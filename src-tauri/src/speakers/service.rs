//! Native speaker-identity service: roster, encrypted voiceprints, model,
//! identification receipts, guided enrolment and the per-profile outbox.
//! Tauri-independent so it is testable without a window; `speakers.rs`
//! exposes the local UI commands. See docs/guide.md for the profile boundary.
//!
//! Fail-closed rules:
//! - a damaged `profiles.json` refuses every voice until local recovery;
//! - in a locked mode, a missing model, unreadable voiceprints, a missing or
//!   locked key, an inference error or a timeout refuse the capture;
//! - security changes only accept [`Actor::LocalDevice`].
use super::self_removal::{self, Answer, Challenge, Stage};
#[cfg(test)]
use notdot_core::profiles::Outbox;
use notdot_core::profiles::{
    Actor, Admission, Capability, ChatAccess, IdentityMode, LoadOutcome, ProfileId, Refusal, Role,
    Roster, SpeakerOutcome, TurnLock,
};
use notdot_speakers::{
    audio,
    embedding::{CamPlusPlus, Extractor},
    engine::{CONTINUITY_FLOOR, Continuity, Engine, IDENTIFY_TIMEOUT_MS, Identification},
    enrol::{EnrolError, Enrolment, Progress},
    matcher::Thresholds,
    store::{KeyStore, StoreError, VoiceprintStore},
};
use serde::Serialize;
#[cfg(test)]
use std::path::Path;
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use zeroize::Zeroize;

pub const ROSTER_FILE: &str = "profiles.json";
pub const VOICEPRINT_FILE: &str = "speakers/voiceprints.bin";
/// Receipts are consumed by send/confirmation soon after capture.
pub const RECEIPT_TTL: Duration = Duration::from_secs(10 * 60);
pub const MAX_RECEIPTS: usize = 64;
/// Longest audio accepted by `identify` (seconds of s16le PCM).
const MAX_IDENTIFY_SECONDS: usize = 10;
/// Longest single enrolment phrase recording accepted.
const MAX_ENROL_SECONDS: usize = 12;
/// Longest capture checked for speaker continuity (matches the capture limit).
const MAX_CONTINUITY_SECONDS: usize = 60;
/// Spoken when a capture contained another voice after admission.
pub const SPEAKER_CHANGED: &str =
    "I heard more than one voice, so I didn't send that. Could one person ask again?";
pub const SPEAKER_UNCHECKED: &str = "I couldn't check who was speaking. Please try again.";

/// Whether a whole capture has been checked for a change of speaker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContinuityState {
    Pending,
    Verified,
    Rejected,
}

struct Receipt {
    token: String,
    at: Instant,
    lock: TurnLock,
    continuity: ContinuityState,
}

/// Who asked for a removal. Both routes finish with a deliberate click in
/// notdot's window; the self-service route also needs a fresh spoken code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovalRoute {
    /// The principal owner, at this computer, removing a trusted member.
    Owner,
    /// A trusted member removing their own profile.
    SelfService,
}

/// Proof that a removal passed every native check. Only
/// [`Speakers::authorise_removal`] can create one, and it is consumed by
/// [`Speakers::remove_member`].
#[derive(Debug)]
pub struct RemovalTicket {
    profile: ProfileId,
    name: String,
    route: RemovalRoute,
}
impl RemovalTicket {
    pub fn profile(&self) -> &ProfileId {
        &self.profile
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn route(&self) -> RemovalRoute {
        self.route
    }
}

/// What removal changed in the roster and voice store.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RemovalOutcome {
    /// Retired private chat, to archive in Codex.
    pub companion_thread: Option<String>,
    /// False when the voice store could not be rewritten now (for example a
    /// locked keychain); the voice is then pruned the next time voices load.
    pub voice_deleted: bool,
}

/// Normalise a typed confirmation name for comparison.
fn same_name(typed: &str, name: &str) -> bool {
    let norm = |s: &str| {
        s.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    !typed.trim().is_empty() && norm(typed) == norm(name)
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "detail")]
pub enum Health {
    /// Nothing loaded yet (open mode without voices never needs the model).
    Idle,
    Ready,
    /// The bundled model is missing or failed verification.
    ModelUnavailable(String),
    /// Voiceprints exist but cannot be used (key, tamper, model change).
    VoicesUnavailable(String),
    /// profiles.json was damaged; local recovery required.
    RosterDamaged,
}

struct EnrolSession {
    enrolment: Enrolment,
    profile: ProfileId,
    cancelled: Arc<AtomicBool>,
    /// Capture token currently recording a phrase, and its bounded PCM.
    capture: Option<(String, u32, zeroize::Zeroizing<Vec<u8>>)>,
}

pub struct Speakers {
    root: PathBuf,
    roster_path: PathBuf,
    model_path: PathBuf,
    roster: Mutex<Roster>,
    roster_damaged: AtomicBool,
    engine: Mutex<Option<Arc<Engine>>>,
    model_error: Mutex<Option<String>>,
    voices_error: Mutex<Option<StoreError>>,
    store: VoiceprintStore,
    receipts: Mutex<VecDeque<Receipt>>,
    self_removal: Mutex<Option<Challenge>>,
    enrolment: Mutex<Option<EnrolSession>>,
    enrol_epoch: AtomicU64,
    #[cfg(test)]
    outbox: Mutex<Outbox<serde_json::Value>>,
}

fn identification_to_outcome(id: Identification) -> SpeakerOutcome {
    match id {
        Identification::NoVoiceprints => SpeakerOutcome::Disabled,
        Identification::Recognised { profile, score, .. } => match ProfileId::parse(&profile) {
            Ok(profile) => SpeakerOutcome::Recognised { profile, score },
            Err(_) => SpeakerOutcome::Unavailable,
        },
        Identification::Ambiguous | Identification::SuspectedReplay => SpeakerOutcome::Ambiguous,
        Identification::Unknown => SpeakerOutcome::Unknown,
        Identification::Unusable { .. } => SpeakerOutcome::Unusable,
        Identification::Cancelled | Identification::Unavailable => SpeakerOutcome::Unavailable,
    }
}

impl Speakers {
    /// Production constructor: OS keychain key, no keychain UI.
    /// Loads `base/profiles.json`; nothing biometric is read until needed.
    pub fn open(base: PathBuf, model: PathBuf, qa: bool) -> Result<Self, String> {
        Ok(Self::with_keys(
            base,
            model,
            Box::new(super::keychain::Keychain::new(qa)),
            "Me",
        ))
    }

    /// Constructor with an explicit key store (tests, QA harnesses).
    pub fn with_keys(
        base: PathBuf,
        model: PathBuf,
        keys: Box<dyn KeyStore>,
        owner_name: &str,
    ) -> Self {
        let roster_path = base.join(ROSTER_FILE);
        let (roster, damaged) = match Roster::load(&roster_path, owner_name) {
            LoadOutcome::Fresh(r) | LoadOutcome::Loaded(r) => (r, false),
            LoadOutcome::Damaged(r, _) => (r, true),
        };
        Self {
            store: VoiceprintStore::new(base.join(VOICEPRINT_FILE), keys),
            root: base,
            roster_path,
            model_path: model,
            roster: Mutex::new(roster),
            roster_damaged: AtomicBool::new(damaged),
            engine: Mutex::new(None),
            model_error: Mutex::new(None),
            voices_error: Mutex::new(None),
            receipts: Mutex::new(VecDeque::new()),
            self_removal: Mutex::new(None),
            enrolment: Mutex::new(None),
            enrol_epoch: AtomicU64::new(0),
            #[cfg(test)]
            outbox: Mutex::new(Outbox::new()),
        }
    }

    /// The app data directory this service manages.
    pub fn base(&self) -> &std::path::Path {
        &self.root
    }
    #[cfg(test)]
    pub fn roster_path(&self) -> &Path {
        &self.roster_path
    }

    /// Current roster (clone). A damaged roster is returned in its fail-closed
    /// form so ACL checks refuse; `Err` is never used for damage.
    pub fn roster(&self) -> Result<Roster, String> {
        Ok(self
            .roster
            .lock()
            .map_err(|_| "Roster unavailable")?
            .clone())
    }

    /// Persist a roster produced by `Roster` methods (which enforce actors and
    /// invariants) and make it current. Refused while the roster is damaged.
    pub fn save_roster(&self, next: Roster) -> Result<(), String> {
        if self.roster_damaged.load(Ordering::Acquire) {
            return Err("Voice recognition needs recovery first".into());
        }
        let mut guard = self.roster.lock().map_err(|_| "Roster unavailable")?;
        next.save(&self.roster_path)?;
        *guard = next;
        Ok(())
    }

    fn mutate<T>(&self, f: impl FnOnce(&mut Roster) -> Result<T, String>) -> Result<T, String> {
        if self.roster_damaged.load(Ordering::Acquire) {
            return Err("Voice recognition needs recovery first".into());
        }
        let mut guard = self.roster.lock().map_err(|_| "Roster unavailable")?;
        let mut next = guard.clone();
        let result = f(&mut next)?;
        next.save(&self.roster_path)?;
        *guard = next;
        Ok(result)
    }
    pub fn set_companion_thread(
        &self,
        member: &ProfileId,
        thread: &str,
        owner_thread: Option<&str>,
    ) -> Result<(), String> {
        self.mutate(|roster| roster.set_companion_thread(member, thread, owner_thread))
    }

    /// True when captures must be identified before use: someone has a voice,
    /// a locked mode is on, or recovery is required. Open mode with no voices
    /// returns false and behaves exactly as before profiles.
    pub fn needs_identification(&self) -> bool {
        let r = self.roster.lock().expect("roster");
        r.recovery_required()
            || r.mode().locked()
            || r.profiles().iter().any(|p| p.voice_enrolled)
            || self.store.path().exists()
    }

    pub fn health(&self) -> Health {
        if self.roster_damaged.load(Ordering::Acquire) {
            return Health::RosterDamaged;
        }
        if let Some(e) = self.voices_error.lock().expect("voices").as_ref() {
            return Health::VoicesUnavailable(e.to_string());
        }
        if let Some(e) = self.model_error.lock().expect("model").clone() {
            return Health::ModelUnavailable(e);
        }
        if self.engine.lock().expect("engine").is_some() {
            Health::Ready
        } else {
            Health::Idle
        }
    }

    /// Load and verify the model, then decrypt voiceprints, once. Voiceprint
    /// failures are recorded (fail closed) but the model stays usable for a
    /// fresh enrolment after "Forget all voices".
    fn ensure_engine(&self) -> Result<Arc<Engine>, String> {
        let mut slot = self.engine.lock().expect("engine");
        if let Some(e) = slot.as_ref() {
            return Ok(e.clone());
        }
        let extractor: Arc<dyn Extractor> = match CamPlusPlus::load(&self.model_path) {
            Ok(x) => Arc::new(x),
            Err(e) => {
                *self.model_error.lock().expect("model") = Some(e.clone());
                return Err(e);
            }
        };
        *self.model_error.lock().expect("model") = None;
        let prints = match self.store.load(&extractor.model_id()) {
            Ok(p) => p,
            Err(e) => {
                *self.voices_error.lock().expect("voices") = Some(e);
                Vec::new()
            }
        };
        // Never use (and finish deleting) a voice whose person was removed or
        // forgotten, for example after a removal that could not rewrite the
        // store at the time.
        let roster = self.roster.lock().expect("roster").clone();
        let enrolled = |id: &str| {
            ProfileId::parse(id)
                .ok()
                .and_then(|id| roster.profile(&id).map(|p| p.voice_enrolled))
                .unwrap_or(false)
        };
        let (prints, stale): (Vec<_>, Vec<_>) =
            prints.into_iter().partition(|p| enrolled(&p.profile));
        if !stale.is_empty() && !roster.recovery_required() {
            let _ = self.store.retain_profiles(enrolled);
        }
        let engine = Arc::new(Engine::new(extractor, prints, Thresholds::default()));
        *slot = Some(engine.clone());
        Ok(engine)
    }

    /// Test hook: record a receipt as if a capture had been identified.
    #[cfg(test)]
    fn insert_receipt(&self, lock: TurnLock, verified: bool) {
        let token = lock.capture_token.clone();
        self.store_receipt(
            &token,
            lock,
            if verified {
                ContinuityState::Verified
            } else {
                ContinuityState::Pending
            },
        );
    }

    /// Test hook: use a prepared engine instead of loading the model.
    #[cfg(test)]
    fn install_engine(&self, engine: Engine) {
        *self.engine.lock().expect("engine") = Some(Arc::new(engine));
    }

    // ------------------------------------------------------------ identity

    async fn outcome(&self, rate: u32, mut samples: Vec<f32>) -> SpeakerOutcome {
        if !self.needs_identification() {
            samples.zeroize();
            return SpeakerOutcome::Disabled;
        }
        let engine = match self.ensure_engine() {
            Ok(e) if self.voices_error.lock().expect("voices").is_none() => e,
            _ => {
                samples.zeroize();
                return SpeakerOutcome::Unavailable;
            }
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        let work = tokio::task::spawn_blocking(move || {
            let out = engine.identify(rate, &samples, &flag);
            samples.zeroize();
            out
        });
        match tokio::time::timeout(Duration::from_millis(IDENTIFY_TIMEOUT_MS), work).await {
            Ok(Ok(id)) => identification_to_outcome(id),
            _ => {
                cancelled.store(true, Ordering::Release);
                SpeakerOutcome::Unavailable
            }
        }
    }

    /// Identify the speaker of a capture from locally held s16le mono PCM,
    /// apply the roster's mode and store a receipt. Returns the immutable lock,
    /// or the refusal (its `spoken()` line is suitable to say aloud). Raw audio
    /// is wiped after embedding; nothing is stored or sent anywhere.
    pub async fn identify_detailed(
        &self,
        token: &str,
        rate: u32,
        pcm: &[u8],
    ) -> Result<TurnLock, Refusal> {
        if token.is_empty() || token.len() > 128 {
            return Err(Refusal::IdentityUnavailable);
        }
        let max = rate as usize * 2 * MAX_IDENTIFY_SECONDS;
        let samples = audio::decode_s16le(&pcm[pcm.len().saturating_sub(max)..]);
        // Admit against the roster as it was when this capture was judged, so
        // the lock carries the epochs that applied before inference. Then
        // validate against the current roster: any removal, re-enrolment,
        // forgotten voice, revoked share or mode change during inference has
        // bumped an epoch (or removed the profile) and the late result is
        // refused instead of minting a fresh lock.
        let snapshot = self.roster.lock().expect("roster").clone();
        let outcome = self.outcome(rate, samples).await;
        match snapshot.admit(token, &outcome) {
            Admission::Admit(lock) => {
                self.roster
                    .lock()
                    .expect("roster")
                    .validate_lock(&lock)
                    .map_err(|_| Refusal::IdentityUnavailable)?;
                // Open-mode fallbacks keep legacy behaviour; a recognised
                // person's capture must pass the continuity check first.
                let continuity = if lock.open_fallback || !self.needs_identification() {
                    ContinuityState::Verified
                } else {
                    ContinuityState::Pending
                };
                self.store_receipt(token, lock.clone(), continuity);
                Ok(lock)
            }
            Admission::Refuse(r) => Err(r),
        }
    }

    /// Convenience form: `Err` carries the short spoken refusal.
    pub async fn identify(&self, token: &str, rate: u32, pcm: &[u8]) -> Result<TurnLock, String> {
        self.identify_detailed(token, rate, pcm)
            .await
            .map_err(|r| r.spoken().to_owned())
    }

    fn store_receipt(&self, token: &str, lock: TurnLock, continuity: ContinuityState) {
        let mut receipts = self.receipts.lock().expect("receipts");
        let now = Instant::now();
        receipts.retain(|r| r.token != token && now.duration_since(r.at) < RECEIPT_TTL);
        while receipts.len() >= MAX_RECEIPTS {
            receipts.pop_front();
        }
        receipts.push_back(Receipt {
            token: token.to_owned(),
            at: now,
            lock,
            continuity,
        });
    }

    fn receipt_entry(&self, token: &str) -> Result<(TurnLock, ContinuityState), String> {
        let mut receipts = self.receipts.lock().expect("receipts");
        let now = Instant::now();
        receipts.retain(|r| now.duration_since(r.at) < RECEIPT_TTL);
        let (lock, continuity) = receipts
            .iter()
            .find(|r| r.token == token)
            .map(|r| (r.lock.clone(), r.continuity))
            .ok_or("That request has expired. Please ask again.")?;
        if let Err(e) = self.validate(&lock) {
            receipts.retain(|r| r.token != token);
            return Err(e);
        }
        Ok((lock, continuity))
    }

    /// The lock recorded for a capture token, re-validated now. Receipts expire
    /// after [`RECEIPT_TTL`]; a stale or revoked one is removed and refused.
    /// A recognised person's capture is only usable once the whole capture
    /// passed [`Speakers::verify_continuity`].
    pub fn receipt(&self, token: &str) -> Result<TurnLock, String> {
        match self.receipt_entry(token)? {
            (lock, ContinuityState::Verified) => Ok(lock),
            (_, ContinuityState::Rejected) => Err(SPEAKER_CHANGED.into()),
            (_, ContinuityState::Pending) => Err(SPEAKER_UNCHECKED.into()),
        }
    }

    fn set_continuity(&self, token: &str, state: ContinuityState) {
        if let Some(r) = self
            .receipts
            .lock()
            .expect("receipts")
            .iter_mut()
            .find(|r| r.token == token)
        {
            r.continuity = state;
        }
    }

    /// Check the whole capture (s16le mono PCM, held locally) for a change of
    /// speaker after admission. Must run before the transcript is used with
    /// anyone's private context. Fails closed: a switch, an unavailable model
    /// or a timeout marks the receipt rejected and returns the spoken reason.
    pub async fn verify_continuity(
        &self,
        token: &str,
        rate: u32,
        pcm: &[u8],
    ) -> Result<(), String> {
        let (lock, state) = self.receipt_entry(token)?;
        match state {
            ContinuityState::Verified => return Ok(()),
            ContinuityState::Rejected => return Err(SPEAKER_CHANGED.into()),
            ContinuityState::Pending => {}
        }
        let max = rate as usize * 2 * MAX_CONTINUITY_SECONDS;
        let mut samples = audio::decode_s16le(&pcm[pcm.len().saturating_sub(max)..]);
        let engine = match self.ensure_engine() {
            Ok(e) => e,
            Err(_) => {
                samples.zeroize();
                self.set_continuity(token, ContinuityState::Rejected);
                return Err(SPEAKER_UNCHECKED.into());
            }
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        let profile = lock.profile.to_string();
        let windows = samples.len() / (rate.max(1) as usize) / 2 + 1;
        let budget = Duration::from_millis(IDENTIFY_TIMEOUT_MS + 250 * windows as u64)
            .min(Duration::from_secs(15));
        let work = tokio::task::spawn_blocking(move || {
            let out = engine.continuity(rate, &samples, &profile, CONTINUITY_FLOOR, &flag);
            samples.zeroize();
            out
        });
        let result = match tokio::time::timeout(budget, work).await {
            Ok(Ok(Continuity::Consistent { .. })) => Ok(()),
            Ok(Ok(Continuity::Switched { .. })) => Err(SPEAKER_CHANGED),
            _ => {
                cancelled.store(true, Ordering::Release);
                Err(SPEAKER_UNCHECKED)
            }
        };
        // Access may have changed while checking.
        let result = result.and_then(|()| self.validate(&lock).map_err(|_| SPEAKER_UNCHECKED));
        self.set_continuity(
            token,
            if result.is_ok() {
                ContinuityState::Verified
            } else {
                ContinuityState::Rejected
            },
        );
        result.map_err(str::to_owned)
    }

    /// Lock for typed or clicked requests from notdot's own window: the
    /// owner's context (keyboard access stays available in every mode).
    pub fn local_lock(&self, request_id: &str) -> Result<TurnLock, String> {
        let r = self.roster.lock().expect("roster");
        let owner = r.owner().clone();
        let lock = TurnLock {
            capture_token: request_id.to_owned(),
            role: notdot_core::profiles::Role::Owner,
            epoch: r.profile(&owner).map_or(0, |p| p.epoch),
            profile: owner,
            open_fallback: false,
        };
        r.validate_lock(&lock).map_err(|e| e.to_string())?;
        Ok(lock)
    }

    pub fn validate(&self, lock: &TurnLock) -> Result<(), String> {
        self.roster
            .lock()
            .expect("roster")
            .validate_lock(lock)
            .map_err(|e| e.to_string())
    }

    // ------------------------------------------------------------ outbox

    /// Defer an announcement (completion report, reminder) for its requester.
    #[cfg(test)]
    pub fn defer(&self, lock: &TurnLock, item: serde_json::Value) {
        self.outbox.lock().expect("outbox").push(lock, item);
    }
    /// Items for the speaker who was just admitted; stale ones are dropped.
    #[cfg(test)]
    pub fn deliverable(&self, active: &TurnLock) -> Vec<serde_json::Value> {
        let r = self.roster.lock().expect("roster");
        self.outbox.lock().expect("outbox").take_for(&r, active)
    }

    // ------------------------------------------------------------ management (LocalDevice only)

    pub fn add_member(&self, actor: &Actor, name: &str) -> Result<ProfileId, String> {
        self.mutate(|r| r.add_member(actor, name))
    }
    pub fn rename(&self, actor: &Actor, id: &ProfileId, name: &str) -> Result<(), String> {
        self.mutate(|r| r.rename(actor, id, name))
    }
    pub fn set_mode(&self, actor: &Actor, mode: IdentityMode) -> Result<(), String> {
        if mode.locked() {
            self.ensure_engine()?;
            if let Health::VoicesUnavailable(e) = self.health() {
                return Err(e);
            }
        }
        self.mutate(|r| r.set_mode(actor, mode))
    }
    pub fn grant_chat(
        &self,
        actor: &Actor,
        chat: &str,
        member: &ProfileId,
        access: ChatAccess,
    ) -> Result<(), String> {
        self.mutate(|r| r.grant_chat(actor, chat, member, access))
    }
    pub fn revoke_chat(&self, actor: &Actor, chat: &str, member: &ProfileId) -> Result<(), String> {
        self.mutate(|r| r.revoke_chat(actor, chat, member))
    }

    // ------------------------------------------------------------ offboarding

    /// Every native check for removing a trusted member, for both routes.
    ///
    /// - `actor` must be [`Actor::LocalDevice`]: a click in notdot's own
    ///   visible, focused window. A voice (even the owner's) never qualifies,
    ///   and the UI never supplies who is acting.
    /// - The target must be a current trusted member. The principal owner can
    ///   never be removed by either route.
    /// - `confirm_name` must match the target's current name, so the dialog
    ///   that was confirmed named the person being removed.
    /// - [`RemovalRoute::SelfService`] additionally needs the target's own
    ///   pending request whose fresh spoken code was confirmed in time, for
    ///   the target's current epoch. The pending request is consumed.
    pub fn authorise_removal(
        &self,
        actor: &Actor,
        target: &ProfileId,
        confirm_name: &str,
        route: RemovalRoute,
    ) -> Result<RemovalTicket, String> {
        if actor != &Actor::LocalDevice {
            return Err("Removing a person needs a confirmation on notdot's screen".into());
        }
        if self.roster_damaged.load(Ordering::Acquire) {
            return Err("Voice recognition needs recovery first".into());
        }
        let roster = self.roster()?;
        let person = roster
            .profile(target)
            .ok_or("That person is no longer set up")?;
        if person.role != Role::Member || target == roster.owner() {
            return Err(
                "The principal owner can't be removed. To start over, use Reset on this computer."
                    .into(),
            );
        }
        if !same_name(confirm_name, &person.name) {
            return Err(format!("Type {} to confirm", person.name));
        }
        if route == RemovalRoute::SelfService {
            let mut slot = self.self_removal.lock().expect("self removal");
            let now = Instant::now();
            let ok = slot.as_ref().is_some_and(|c| {
                &c.profile == target
                    && c.epoch == person.epoch
                    && c.stage == Stage::AwaitingLocalConfirmation
                    && !c.expired(now)
            });
            if !ok {
                return Err(format!(
                    "{} needs to ask notdot to remove their profile first, then say the code",
                    person.name
                ));
            }
            *slot = None;
        }
        Ok(RemovalTicket {
            profile: target.clone(),
            name: person.name.clone(),
            route,
        })
    }

    /// Remove an authorised member from the roster (revoking access, grants,
    /// receipts and any pending self-removal), retire their private chat and
    /// delete their voice. The caller records the durable offboarding marker
    /// *before* calling this and finishes local data deletion afterwards.
    pub fn remove_member(&self, ticket: RemovalTicket) -> Result<RemovalOutcome, String> {
        self.complete_recorded_removal(&ticket.profile)
    }

    /// Finish a removal that was authorised and durably recorded earlier, for
    /// example after a crash. Idempotent: a person already gone is fine.
    pub fn complete_recorded_removal(&self, id: &ProfileId) -> Result<RemovalOutcome, String> {
        let roster = self.roster()?;
        if id == roster.owner() {
            return Err("Refusing to remove the owner".into());
        }
        let companion_thread = match roster.profile(id) {
            Some(p) => {
                let thread = p.companion_thread.clone();
                self.mutate(|r| r.remove_member_recorded(&self.root, id).map(|_| ()))?;
                thread
            }
            None => None,
        };
        #[cfg(test)]
        self.outbox.lock().expect("outbox").forget(id);
        self.receipts
            .lock()
            .expect("receipts")
            .retain(|r| &r.lock.profile != id);
        {
            let mut slot = self.self_removal.lock().expect("self removal");
            if slot.as_ref().is_some_and(|c| &c.profile == id) {
                *slot = None;
            }
        }
        let voice_deleted = self.store.retain_profiles(|p| p != id.as_str()).is_ok();
        if let Some(engine) = self.engine.lock().expect("engine").as_ref() {
            let prints: Vec<_> = engine
                .prints()
                .iter()
                .filter(|p| p.profile != id.as_str())
                .cloned()
                .collect();
            engine.replace_prints(prints);
        }
        Ok(RemovalOutcome {
            companion_thread,
            voice_deleted,
        })
    }

    /// Record the outcome of archiving a retired chat in Codex.
    pub fn record_archive(&self, chat: &str, archived: bool) -> Result<(), String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or_default();
        self.mutate(|r| {
            r.record_archive(chat, archived, now);
            Ok(())
        })
    }

    // ------------------------------------------------------------ self-service removal

    /// A trusted member asked to remove their own profile. Returns the line to
    /// speak, containing a fresh one-time code. Only a member's own continuity
    /// verified capture qualifies; the owner and open-mode fallbacks are told
    /// how to proceed instead.
    pub fn self_removal_request(&self, token: &str) -> Result<String, String> {
        let lock = self.receipt(token)?;
        if lock.open_fallback {
            return Err("I can't tell who's speaking, so I can't remove a profile. Ask the owner to help on this computer.".into());
        }
        if lock.role == Role::Owner {
            return Err("You're the owner, so your profile can't be removed. To start over, use Reset in Settings on this computer.".into());
        }
        let name = self
            .roster()?
            .profile(&lock.profile)
            .map(|p| p.name.clone())
            .ok_or("That person is no longer set up")?;
        let challenge = Challenge::new(&lock, self_removal::random_code(), Instant::now());
        let line = format!(
            "{name}, this removes your profile, your saved voice and your private memories. To continue, say: remove me, code {}. Then confirm on notdot's screen. The code works for two minutes.",
            challenge.spoken_code()
        );
        *self.self_removal.lock().expect("self removal") = Some(challenge);
        Ok(line)
    }

    /// Offer a transcript to a pending self-removal. `None` means it is not
    /// part of that flow and should be handled normally.
    pub fn self_removal_answer(
        &self,
        token: &str,
        transcript: &str,
    ) -> Option<Result<String, String>> {
        let mut slot = self.self_removal.lock().expect("self removal");
        let challenge = slot.as_mut()?;
        let lock = match self.receipt(token) {
            Ok(lock) => lock,
            Err(_) => return None,
        };
        let name = self
            .roster()
            .ok()?
            .profile(&challenge.profile)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        match challenge.answer(&lock, transcript, Instant::now()) {
            Answer::NotForUs => None,
            Answer::Confirmed => Some(Ok(format!(
                "Thanks, {name}. To finish, select Remove my profile on notdot's screen and type your name."
            ))),
            Answer::WrongCode { remaining } => Some(Err(format!(
                "That code didn't match. You can try {remaining} more {}.",
                if remaining == 1 { "time" } else { "times" }
            ))),
            Answer::Ended => {
                *slot = None;
                Some(Err(
                    "I've stopped the removal. Nothing was deleted. You can ask again any time."
                        .into(),
                ))
            }
        }
    }

    /// Cancel a pending self-removal (from the screen or by voice).
    pub fn self_removal_cancel(&self) {
        *self.self_removal.lock().expect("self removal") = None;
    }

    /// Pending self-removal, for the settings panel.
    pub fn self_removal_status(&self) -> serde_json::Value {
        let slot = self.self_removal.lock().expect("self removal");
        let now = Instant::now();
        match slot.as_ref().filter(|c| !c.expired(now)) {
            Some(c) => serde_json::json!({
                "profile": c.profile,
                "stage": match c.stage {
                    Stage::AwaitingCode { .. } => "awaiting_code",
                    Stage::AwaitingLocalConfirmation => "awaiting_local_confirmation",
                },
                "expires_in_s": c.expires_in(now).as_secs(),
            }),
            None => serde_json::Value::Null,
        }
    }

    /// Remove one person's voiceprint (they can re-teach it later).
    pub fn forget_voice(&self, actor: &Actor, id: &ProfileId) -> Result<(), String> {
        self.roster()?
            .authorise(actor, Capability::ManageMembers)
            .map_err(|e| e.to_string())?;
        if !self.roster()?.profile(id).is_some_and(|p| p.voice_enrolled) {
            return Ok(());
        }
        let engine = self.ensure_engine()?;
        let model = engine.extractor().model_id();
        let prints: Vec<_> = engine
            .prints()
            .iter()
            .filter(|p| p.profile != id.as_str())
            .cloned()
            .collect();
        self.mutate(|r| r.set_voice_enrolled(actor, id, false))?;
        self.store
            .save(&model, &prints)
            .map_err(|e| e.to_string())?;
        engine.replace_prints(prints);
        Ok(())
    }

    /// Forget every voice: switch to open mode, delete the encrypted voiceprint
    /// file and the device key. People, memories, chats and grants stay.
    pub fn forget_all_voices(&self, actor: &Actor) -> Result<(), String> {
        self.roster()?
            .authorise(actor, Capability::ManageMembers)
            .map_err(|e| e.to_string())?;
        if !self.roster_damaged.load(Ordering::Acquire) {
            self.mutate(|r| {
                r.set_mode(actor, IdentityMode::Open)?;
                let ids: Vec<_> = r.profiles().iter().map(|p| p.id.clone()).collect();
                for id in ids {
                    r.set_voice_enrolled(actor, &id, false)?;
                }
                Ok(())
            })?;
        }
        self.store.forget_all().map_err(|e| e.to_string())?;
        *self.voices_error.lock().expect("voices") = None;
        if let Some(engine) = self.engine.lock().expect("engine").as_ref() {
            engine.replace_prints(Vec::new());
        }
        self.receipts.lock().expect("receipts").clear();
        Ok(())
    }

    /// Local recovery from a damaged roster: forget all voices and start a
    /// fresh open-mode roster that keeps the owner's existing memory ledger.
    /// Returns member directories left from the old roster, for a separate,
    /// explicitly confirmed removal (never deleted here).
    pub fn recover(&self, actor: &Actor, owner_name: &str) -> Result<Vec<PathBuf>, String> {
        let fresh = Roster::recover(actor, owner_name).map_err(|e| e.to_string())?;
        self.store.forget_all().map_err(|e| e.to_string())?;
        fresh.save(&self.roster_path)?;
        *self.roster.lock().expect("roster") = fresh.clone();
        self.roster_damaged.store(false, Ordering::Release);
        *self.voices_error.lock().expect("voices") = None;
        if let Some(engine) = self.engine.lock().expect("engine").as_ref() {
            engine.replace_prints(Vec::new());
        }
        self.receipts.lock().expect("receipts").clear();
        Ok(fresh.orphaned_member_dirs(&self.root))
    }

    // ------------------------------------------------------------ enrolment

    /// Begin guided enrolment for `id`. Returns the first phrase to say.
    pub fn enrol_begin(&self, actor: &Actor, id: &ProfileId) -> Result<Progress, String> {
        let r = self.roster()?;
        r.authorise(actor, Capability::ManageMembers)
            .map_err(|e| e.to_string())?;
        let person = r.profile(id).ok_or("Unknown profile")?;
        if !person.named() {
            return Err("Tell notdot what to call you first.".into());
        }
        self.ensure_engine()?;
        if let Health::VoicesUnavailable(e) = self.health() {
            return Err(format!("{e}. Use \"Forget all voices\" to start again."));
        }
        let (enrolment, progress) = Enrolment::start(id.as_str());
        let mut slot = self.enrolment.lock().expect("enrolment");
        self.enrol_epoch.fetch_add(1, Ordering::SeqCst);
        if let Some(old) = slot.take() {
            old.cancelled.store(true, Ordering::Release);
        }
        *slot = Some(EnrolSession {
            enrolment,
            profile: id.clone(),
            cancelled: Arc::new(AtomicBool::new(false)),
            capture: None,
        });
        Ok(progress)
    }

    /// Mark `token` as the capture recording the current phrase. Its PCM must
    /// be routed to [`Speakers::enrol_audio`], never to speech recognition.
    pub fn enrol_capture(&self, token: &str) -> Result<(), String> {
        let mut slot = self.enrolment.lock().expect("enrolment");
        let s = slot.as_mut().ok_or("Enrolment is not active")?;
        s.capture = Some((token.to_owned(), 0, zeroize::Zeroizing::new(Vec::new())));
        Ok(())
    }

    /// Is this capture recording an enrolment phrase? Root's capture path
    /// checks this first and diverts the audio here.
    pub fn is_enrol_capture(&self, token: &str) -> bool {
        self.enrolment
            .lock()
            .expect("enrolment")
            .as_ref()
            .and_then(|s| s.capture.as_ref())
            .is_some_and(|(t, _, _)| t == token)
    }

    /// Buffer enrolment PCM (bounded). Returns false if the capture overflowed
    /// or is not the enrolment capture, in which case it should be stopped.
    pub fn enrol_audio(&self, token: &str, rate: u32, pcm: &[u8]) -> bool {
        let mut slot = self.enrolment.lock().expect("enrolment");
        let Some((t, r, buf)) = slot.as_mut().and_then(|s| s.capture.as_mut()) else {
            return false;
        };
        if t != token {
            return false;
        }
        *r = rate;
        if buf.len() + pcm.len() > rate as usize * 2 * MAX_ENROL_SECONDS {
            return false;
        }
        buf.extend_from_slice(pcm);
        true
    }

    /// The phrase capture ended: embed it and return the next step.
    pub async fn enrol_capture_end(&self, token: &str) -> Result<Progress, String> {
        let (rate, pcm) = {
            let mut slot = self.enrolment.lock().expect("enrolment");
            let s = slot.as_mut().ok_or("Enrolment is not active")?;
            match s.capture.take() {
                Some((t, rate, mut pcm)) if t == token => (rate, std::mem::take(&mut *pcm)),
                other => {
                    s.capture = other;
                    return Err("That recording was not for enrolment".into());
                }
            }
        };
        self.enrol_sample(rate, pcm).await
    }

    /// Submit a phrase recording directly (s16le mono PCM). Audio is reduced
    /// to an embedding and wiped; nothing is written to disk.
    pub async fn enrol_sample(&self, rate: u32, mut pcm: Vec<u8>) -> Result<Progress, String> {
        if pcm.len() > rate as usize * 2 * MAX_ENROL_SECONDS {
            pcm.zeroize();
            return Err("That recording was too long".into());
        }
        let epoch = self.enrol_epoch.load(Ordering::SeqCst);
        let engine = self.ensure_engine()?;
        let mut session = self
            .enrolment
            .lock()
            .expect("enrolment")
            .take()
            .ok_or("Enrolment is not active")?;
        let mut samples = audio::decode_s16le(&pcm);
        pcm.zeroize();
        let cancelled = session.cancelled.clone();
        let (session, progress) = tokio::task::spawn_blocking(move || {
            let p = engine.enrol_sample(&mut session.enrolment, rate, &samples);
            samples.zeroize();
            (session, p)
        })
        .await
        .map_err(|_| "Enrolment stopped")?;
        let mut slot = self.enrolment.lock().expect("enrolment");
        if cancelled.load(Ordering::Acquire)
            || self.enrol_epoch.load(Ordering::SeqCst) != epoch
            || slot.is_some()
        {
            return Err("Enrolment cancelled".into());
        }
        *slot = Some(session);
        Ok(progress)
    }

    /// Cancel enrolment: every collected sample is dropped and zeroised.
    pub fn enrol_cancel(&self) {
        let mut slot = self.enrolment.lock().expect("enrolment");
        self.enrol_epoch.fetch_add(1, Ordering::SeqCst);
        if let Some(s) = slot.take() {
            s.cancelled.store(true, Ordering::Release);
        }
    }

    /// Save the new voiceprint (replacing any previous one for this person).
    /// `consent` is the owner's confirmation, at this computer, that the
    /// person was present and agreed; it is recorded with the voice.
    pub fn enrol_save(&self, actor: &Actor, consent: bool) -> Result<(), String> {
        self.roster()?
            .authorise(actor, Capability::ManageMembers)
            .map_err(|e| e.to_string())?;
        if !consent {
            return Err(
                "Confirm that this person is here and agrees before saving their voice.".into(),
            );
        }
        let session = self
            .enrolment
            .lock()
            .expect("enrolment")
            .take()
            .ok_or("Enrolment is not active")?;
        let profile = session.profile.clone();
        let engine = self.ensure_engine()?;
        let others = engine.prints();
        let EnrolSession { enrolment, .. } = session;
        let print = enrolment
            .finish(&others, engine.thresholds())
            .map_err(|e| match e {
                EnrolError::Incomplete => "Enrolment isn't finished yet".to_owned(),
                EnrolError::TooSimilar { profile, .. } => {
                    let name = ProfileId::parse(&profile)
                        .ok()
                        .and_then(|id| self.roster().ok()?.profile(&id).map(|p| p.name.clone()))
                        .unwrap_or_else(|| "someone already set up".into());
                    format!(
                        "That voice sounds too much like {name} for me to tell you apart reliably."
                    )
                }
            })?;
        let mut prints: Vec<_> = others
            .iter()
            .filter(|p| p.profile != print.profile)
            .cloned()
            .collect();
        prints.push(print);
        let model = engine.extractor().model_id();
        self.store
            .save(&model, &prints)
            .map_err(|e| e.to_string())?;
        if let Err(e) = self.mutate(|r| {
            r.set_voice_enrolled(actor, &profile, true)?;
            r.record_voice_consent(actor, &profile)
        }) {
            let restored: Vec<_> = prints
                .iter()
                .filter(|p| p.profile != profile.as_str())
                .cloned()
                .collect();
            let _ = self.store.save(&model, &restored);
            return Err(e);
        }
        engine.replace_prints(prints);
        Ok(())
    }

    /// Settings-panel view: names, modes, shares and status, never biometrics.
    pub fn summary(&self) -> serde_json::Value {
        let mut v = self.roster().map(|r| r.summary()).unwrap_or_default();
        v["health"] = serde_json::to_value(self.health()).unwrap_or_default();
        v["self_removal"] = self.self_removal_status();
        v["enrolling"] = serde_json::json!(
            self.enrolment
                .lock()
                .expect("enrolment")
                .as_ref()
                .map(|s| s.profile.clone())
        );
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notdot_core::profiles::{ChatOp, GlobalChatMode, Role};
    use notdot_speakers::store::{Key, MemoryKeyStore};

    #[derive(Clone, Default)]
    struct SharedKeys(Arc<MemoryKeyStore>);
    impl KeyStore for SharedKeys {
        fn load(&self) -> Result<Option<Key>, String> {
            self.0.load()
        }
        fn store(&self, key: &Key) -> Result<(), String> {
            self.0.store(key)
        }
        fn delete(&self) -> Result<(), String> {
            self.0.delete()
        }
    }
    fn open(dir: &Path, model: PathBuf, keys: &SharedKeys) -> Speakers {
        Speakers::with_keys(dir.to_owned(), model, Box::new(keys.clone()), "Owner")
    }
    fn silence_pcm(seconds: usize) -> Vec<u8> {
        vec![0u8; 16_000 * 2 * seconds]
    }
    fn local() -> Actor {
        Actor::LocalDevice
    }

    #[tokio::test]
    async fn open_mode_without_voices_is_legacy_and_needs_no_model() {
        let dir = tempfile::tempdir().unwrap();
        let s = open(
            dir.path(),
            dir.path().join("missing.onnx"),
            &SharedKeys::default(),
        );
        assert!(!s.needs_identification());
        let lock = s.identify("cap-1", 16_000, &silence_pcm(1)).await.unwrap();
        assert_eq!(lock.role, Role::Owner);
        assert!(lock.open_fallback);
        assert_eq!(s.receipt("cap-1").unwrap(), lock);
        assert_eq!(s.health(), Health::Idle);
        assert!(!dir.path().join(VOICEPRINT_FILE).exists());
    }

    #[tokio::test]
    async fn locked_mode_fails_closed_without_model_or_voices() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = Roster::new("Owner").unwrap();
        let owner = r.owner().clone();
        r.set_voice_enrolled(&local(), &owner, true).unwrap();
        r.set_mode(&local(), IdentityMode::OwnerOnly).unwrap();
        r.save(&dir.path().join(ROSTER_FILE)).unwrap();
        let s = open(
            dir.path(),
            dir.path().join("missing.onnx"),
            &SharedKeys::default(),
        );
        assert!(s.needs_identification());
        assert_eq!(
            s.identify_detailed("cap", 16_000, &silence_pcm(3)).await,
            Err(Refusal::IdentityUnavailable)
        );
        assert!(s.receipt("cap").is_err());
        assert!(matches!(s.health(), Health::ModelUnavailable(_)));
        // Keyboard requests in the owner's context remain available.
        assert_eq!(s.local_lock("typed-1").unwrap().profile, owner);
        // Cannot switch to another locked mode while unavailable; open is fine.
        assert!(s.set_mode(&local(), IdentityMode::TrustedGroup).is_err());
        s.set_mode(&local(), IdentityMode::Open).unwrap();
    }

    #[tokio::test]
    async fn damaged_roster_refuses_everything_until_local_recovery() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(ROSTER_FILE), b"{not json").unwrap();
        std::fs::create_dir_all(
            dir.path()
                .join("profiles/0b7c1f4e-2d6a-4c1e-9f3a-5b2d8e7a6c10/memory"),
        )
        .unwrap();
        let s = open(
            dir.path(),
            dir.path().join("missing.onnx"),
            &SharedKeys::default(),
        );
        assert_eq!(s.health(), Health::RosterDamaged);
        assert!(s.needs_identification());
        assert_eq!(
            s.identify_detailed("cap", 16_000, &silence_pcm(3)).await,
            Err(Refusal::RecoveryRequired)
        );
        assert!(s.add_member(&local(), "Ada").is_err());
        assert!(s.save_roster(s.roster().unwrap()).is_err());
        assert!(s.local_lock("typed").is_err());
        // The damaged file is not overwritten until recovery.
        assert_eq!(
            std::fs::read(dir.path().join(ROSTER_FILE)).unwrap(),
            b"{not json"
        );
        let fake_voice = Actor::Voice(TurnLock {
            capture_token: "x".into(),
            profile: ProfileId::random(),
            role: Role::Owner,
            epoch: 0,
            open_fallback: false,
        });
        assert!(s.recover(&fake_voice, "Owner").is_err());
        let orphans = s.recover(&local(), "Owner").unwrap();
        assert_eq!(orphans.len(), 1);
        assert!(
            orphans[0].exists(),
            "member data is never deleted implicitly"
        );
        assert_eq!(s.roster().unwrap().mode(), IdentityMode::Open);
        assert!(s.identify("cap2", 16_000, &silence_pcm(1)).await.is_ok());
    }

    #[tokio::test]
    async fn receipts_are_bounded_and_revalidated() {
        let dir = tempfile::tempdir().unwrap();
        let s = open(
            dir.path(),
            dir.path().join("missing.onnx"),
            &SharedKeys::default(),
        );
        for i in 0..(MAX_RECEIPTS + 6) {
            s.identify(&format!("cap-{i}"), 16_000, &[]).await.unwrap();
        }
        assert!(s.receipt("cap-0").is_err());
        assert!(s.receipt(&format!("cap-{}", MAX_RECEIPTS + 5)).is_ok());
        assert!(s.receipt("never").is_err());
        // An access change bumps the owner's epoch: older receipts are revoked.
        let mut r = s.roster().unwrap();
        let owner = r.owner().clone();
        r.set_voice_enrolled(&local(), &owner, true).unwrap();
        s.save_roster(r).unwrap();
        assert!(s.receipt(&format!("cap-{}", MAX_RECEIPTS + 5)).is_err());
        assert!(s.identify("", 16_000, &[]).await.is_err());
    }

    #[tokio::test]
    async fn management_is_local_only_and_removal_is_exact() {
        let dir = tempfile::tempdir().unwrap();
        let s = open(
            dir.path(),
            dir.path().join("missing.onnx"),
            &SharedKeys::default(),
        );
        let owner_lock = s.identify("cap", 16_000, &[]).await.unwrap();
        let voice = Actor::Voice(owner_lock.clone());
        assert!(s.add_member(&voice, "Eve").is_err());
        assert!(s.set_mode(&voice, IdentityMode::Open).is_err());
        assert!(s.forget_all_voices(&voice).is_err());
        assert!(s.enrol_begin(&voice, &owner_lock.profile).is_err());
        let ada = s.add_member(&local(), "Ada").unwrap();
        assert!(
            s.grant_chat(&voice, "chat-1", &ada, ChatAccess::Read)
                .is_err()
        );
        s.grant_chat(&local(), "chat-1", &ada, ChatAccess::Read)
            .unwrap();
        // Root's ChatPolicy reloads profiles.json: it reflects the grant.
        let LoadOutcome::Loaded(on_disk) = Roster::load(s.roster_path(), "x") else {
            panic!()
        };
        assert_eq!(on_disk.grants().len(), 1);
        let ada_dir = dir.path().join("profiles").join(ada.as_str());
        std::fs::create_dir_all(ada_dir.join("memory")).unwrap();
        std::fs::write(ada_dir.join("memory/db"), b"ada's notes").unwrap();
        std::fs::create_dir_all(dir.path().join("memory")).unwrap();
        std::fs::write(dir.path().join("memory/db"), b"owner notes").unwrap();
        let mut r = s.roster().unwrap();
        r.set_companion_thread(&ada, "ada-thread", Some("owner-thread"))
            .unwrap();
        s.save_roster(r).unwrap();
        let lock = TurnLock {
            capture_token: "c".into(),
            profile: ada.clone(),
            role: Role::Member,
            epoch: s.roster().unwrap().profile(&ada).unwrap().epoch,
            open_fallback: false,
        };
        assert!(s.roster().unwrap().chat_allowed(
            &lock,
            "chat-1",
            ChatOp::Read,
            GlobalChatMode::Confirm
        ));
        s.defer(&lock, serde_json::json!("ada's reminder"));
        let ticket = s
            .authorise_removal(&local(), &ada, "ada", RemovalRoute::Owner)
            .unwrap();
        assert_eq!(
            s.remove_member(ticket).unwrap().companion_thread.as_deref(),
            Some("ada-thread")
        );
        assert!(ada_dir.exists()); // App wrapper performs removal after closing stores.
        assert_eq!(
            std::fs::read(dir.path().join("memory/db")).unwrap(),
            b"owner notes"
        );
        assert!(s.validate(&lock).is_err());
        assert!(s.roster().unwrap().grants().is_empty());
        assert!(s.deliverable(&lock).is_empty());
    }

    /// Extractor that blocks mid-inference until released, so a test can
    /// change the roster while a capture is being judged.
    struct Blocking {
        started: std::sync::Mutex<std::sync::mpsc::Sender<()>>,
        release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    }
    impl Extractor for Blocking {
        fn embed(&self, _: u32, _: &[f32]) -> Result<Vec<f32>, String> {
            let _ = self.started.lock().unwrap().send(());
            let _ = self
                .release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_millis(1500));
            Ok(vec![1.0, 0.0, 0.0])
        }
        fn dim(&self) -> usize {
            3
        }
        fn model_id(&self) -> [u8; 32] {
            [0; 32]
        }
    }
    fn speech_pcm() -> Vec<u8> {
        (0..16_000 * 2)
            .flat_map(|i| ((0.3 * (i as f32 * 0.05).sin() * 32767.0) as i16).to_le_bytes())
            .collect()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn late_recognition_after_access_change_never_mints_a_lock() {
        use notdot_speakers::matcher::Voiceprint;
        let dir = tempfile::tempdir().unwrap();
        let s = Arc::new(open(
            dir.path(),
            dir.path().join("m.onnx"),
            &SharedKeys::default(),
        ));
        let mut r = s.roster().unwrap();
        let owner = r.owner().clone();
        let ada = r.add_member(&local(), "Ada").unwrap();
        r.set_voice_enrolled(&local(), &owner, true).unwrap();
        r.set_voice_enrolled(&local(), &ada, true).unwrap();
        r.set_mode(&local(), IdentityMode::TrustedGroup).unwrap();
        s.save_roster(r).unwrap();
        let near = |y: f32, z: f32| notdot_speakers::embedding::normalise(vec![1.0, y, z]).unwrap();
        let ada_print = Voiceprint {
            profile: ada.to_string(),
            samples: vec![near(0.3, 0.0), near(-0.3, 0.0), near(0.0, 0.3)],
        };
        let (started_tx, started) = std::sync::mpsc::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        s.install_engine(Engine::new(
            Arc::new(Blocking {
                started: std::sync::Mutex::new(started_tx),
                release: std::sync::Mutex::new(release_rx),
            }),
            vec![ada_print],
            Thresholds::default(),
        ));
        let changes: [&dyn Fn(&mut Roster); 3] = [
            // Control: nothing changes, so Ada is admitted.
            &|_| {},
            // Re-enrolment (or forgetting) bumps Ada's epoch.
            &|r| r.set_voice_enrolled(&local(), &ada, true).unwrap(),
            // Removal.
            &|r| {
                r.remove_member(&local(), Path::new("/unused"), &ada)
                    .unwrap();
            },
        ];
        for (i, change) in changes.iter().enumerate() {
            let token = format!("race-{i}");
            let task = {
                let (s, token) = (s.clone(), token.clone());
                tokio::spawn(
                    async move { s.identify_detailed(&token, 16_000, &speech_pcm()).await },
                )
            };
            started
                .recv_timeout(Duration::from_secs(2))
                .expect("inference started");
            let mut r = s.roster().unwrap();
            change(&mut r);
            s.save_roster(r).unwrap();
            release.send(()).unwrap();
            let result = task.await.unwrap();
            if i == 0 {
                assert_eq!(result.unwrap().profile, ada);
                // Admitted, but unusable until the whole capture is checked.
                assert!(s.receipt_entry(&token).is_ok());
                assert_eq!(s.receipt(&token), Err(SPEAKER_UNCHECKED.into()));
            } else {
                assert_eq!(result, Err(Refusal::IdentityUnavailable), "change {i}");
                assert!(s.receipt(&token).is_err(), "change {i}");
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancel_or_new_enrolment_during_inference_never_restores_old_session() {
        let dir = tempfile::tempdir().unwrap();
        let s = Arc::new(open(
            dir.path(),
            dir.path().join("m.onnx"),
            &SharedKeys::default(),
        ));
        let owner = s.roster().unwrap().owner().clone();
        let ada = s.add_member(&local(), "Ada").unwrap();
        let (started_tx, started) = std::sync::mpsc::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        s.install_engine(Engine::new(
            Arc::new(Blocking {
                started: std::sync::Mutex::new(started_tx),
                release: std::sync::Mutex::new(release_rx),
            }),
            vec![],
            Thresholds::default(),
        ));
        let in_flight = |s: &Arc<Speakers>| {
            let s = s.clone();
            tokio::spawn(async move { s.enrol_sample(16_000, speech_pcm()).await })
        };
        let current = |s: &Speakers| {
            s.enrolment
                .lock()
                .unwrap()
                .as_ref()
                .map(|x| (x.profile.clone(), x.enrolment.current()))
        };

        // Control: an uninterrupted sample advances the same session.
        s.enrol_begin(&local(), &owner).unwrap();
        let task = in_flight(&s);
        started.recv_timeout(Duration::from_secs(2)).unwrap();
        release.send(()).unwrap();
        assert!(matches!(
            task.await.unwrap(),
            Ok(Progress::Prompt { index: 2, .. })
        ));
        assert_eq!(current(&s).unwrap().0, owner);

        // Cancel while the sample is being embedded: nothing comes back.
        let task = in_flight(&s);
        started.recv_timeout(Duration::from_secs(2)).unwrap();
        s.enrol_cancel();
        release.send(()).unwrap();
        assert_eq!(task.await.unwrap(), Err("Enrolment cancelled".into()));
        assert!(current(&s).is_none(), "cancelled session was restored");
        assert!(s.enrol_save(&local(), true).is_err());
        assert!(s.summary()["enrolling"].is_null());

        // A new enrolment started mid-inference wins; the old one is void.
        s.enrol_begin(&local(), &owner).unwrap();
        let task = in_flight(&s);
        started.recv_timeout(Duration::from_secs(2)).unwrap();
        s.enrol_begin(&local(), &ada).unwrap();
        release.send(()).unwrap();
        assert_eq!(task.await.unwrap(), Err("Enrolment cancelled".into()));
        let (who, progress) = current(&s).unwrap();
        assert_eq!(who, ada);
        assert!(
            matches!(progress, Progress::Prompt { index: 1, .. }),
            "{progress:?}"
        );

        // Cancel then immediately begin again mid-inference: still the new one.
        let task = in_flight(&s);
        started.recv_timeout(Duration::from_secs(2)).unwrap();
        s.enrol_cancel();
        s.enrol_begin(&local(), &owner).unwrap();
        release.send(()).unwrap();
        assert_eq!(task.await.unwrap(), Err("Enrolment cancelled".into()));
        let (who, progress) = current(&s).unwrap();
        assert_eq!(who, owner);
        assert!(
            matches!(progress, Progress::Prompt { index: 1, .. }),
            "{progress:?}"
        );
        assert!(!dir.path().join(VOICEPRINT_FILE).exists());
    }

    // ---------------------------------------------------------------- offboarding

    fn member_lock(s: &Speakers, id: &ProfileId, token: &str) -> TurnLock {
        let r = s.roster().unwrap();
        let p = r.profile(id).unwrap();
        TurnLock {
            capture_token: token.into(),
            profile: id.clone(),
            role: p.role,
            epoch: p.epoch,
            open_fallback: false,
        }
    }
    /// Owner with a voice, trusted mode, members Ada and Grace with voices.
    fn household(dir: &Path) -> (Speakers, ProfileId, ProfileId, ProfileId) {
        let s = open(dir, dir.join("m.onnx"), &SharedKeys::default());
        let mut r = s.roster().unwrap();
        let owner = r.owner().clone();
        let ada = r.add_member(&local(), "Ada").unwrap();
        let grace = r.add_member(&local(), "Grace").unwrap();
        for id in [&owner, &ada, &grace] {
            r.set_voice_enrolled(&local(), id, true).unwrap();
        }
        r.set_mode(&local(), IdentityMode::TrustedGroup).unwrap();
        r.set_companion_thread(&ada, "ada-private", Some("owner-thread"))
            .unwrap();
        s.save_roster(r).unwrap();
        (s, owner, ada, grace)
    }
    fn code_from(line: &str) -> String {
        self_removal::spoken_digits(line.split("code").nth(1).unwrap(), false)
    }

    #[test]
    fn owner_removal_needs_local_actor_named_member_target_and_never_the_owner() {
        let dir = tempfile::tempdir().unwrap();
        let (s, owner, ada, grace) = household(dir.path());
        let owner_voice = Actor::Voice(member_lock(&s, &owner, "o"));
        let ada_voice = Actor::Voice(member_lock(&s, &ada, "a"));
        // Forged actors: no voice, not even the owner's, can remove anyone.
        for actor in [&owner_voice, &ada_voice] {
            for target in [&ada, &grace, &owner] {
                assert!(
                    s.authorise_removal(actor, target, "Ada", RemovalRoute::Owner)
                        .is_err()
                );
                assert!(
                    s.authorise_removal(actor, target, "Ada", RemovalRoute::SelfService)
                        .is_err()
                );
            }
        }
        // The principal can never be removed, by either route.
        for route in [RemovalRoute::Owner, RemovalRoute::SelfService] {
            let name = s.roster().unwrap().profile(&owner).unwrap().name.clone();
            assert!(
                s.authorise_removal(&local(), &owner, &name, route)
                    .unwrap_err()
                    .contains("principal owner")
            );
        }
        // Named confirmation must match the target, not another person.
        assert!(
            s.authorise_removal(&local(), &ada, "Grace", RemovalRoute::Owner)
                .is_err()
        );
        assert!(
            s.authorise_removal(&local(), &ada, "", RemovalRoute::Owner)
                .is_err()
        );
        assert!(
            s.authorise_removal(&local(), &ProfileId::random(), "Ada", RemovalRoute::Owner)
                .is_err()
        );
        let ticket = s
            .authorise_removal(&local(), &ada, "  ada ", RemovalRoute::Owner)
            .unwrap();
        assert_eq!((ticket.profile(), ticket.name()), (&ada, "Ada"));
        let out = s.remove_member(ticket).unwrap();
        assert_eq!(out.companion_thread.as_deref(), Some("ada-private"));
        let r = s.roster().unwrap();
        assert!(
            r.profile(&ada).is_none() && r.profile(&grace).is_some() && r.profile(&owner).is_some()
        );
        assert!(r.is_retired("ada-private"));
    }

    #[tokio::test]
    async fn self_removal_needs_own_voice_fresh_code_and_local_click() {
        let dir = tempfile::tempdir().unwrap();
        let (s, owner, ada, grace) = household(dir.path());
        // Without a confirmed request, self-service removal is refused.
        assert!(
            s.authorise_removal(&local(), &ada, "Ada", RemovalRoute::SelfService)
                .is_err()
        );
        // The owner and open-mode fallbacks cannot start one.
        s.insert_receipt(member_lock(&s, &owner, "o1"), true);
        assert!(s.self_removal_request("o1").unwrap_err().contains("owner"));
        let mut fallback = member_lock(&s, &owner, "f1");
        fallback.open_fallback = true;
        s.insert_receipt(fallback, true);
        assert!(s.self_removal_request("f1").is_err());
        // A capture not yet checked for a speaker switch cannot start one.
        s.insert_receipt(member_lock(&s, &ada, "a0"), false);
        assert!(s.self_removal_request("a0").is_err());
        // Ada asks.
        s.insert_receipt(member_lock(&s, &ada, "a1"), true);
        let line = s.self_removal_request("a1").unwrap();
        let code = code_from(&line);
        assert_eq!(code.len(), 4);
        assert_eq!(s.self_removal_status()["stage"], "awaiting_code");
        // Replaying the request capture, Grace's voice, or the owner's voice
        // saying the code does nothing.
        assert!(s.self_removal_answer("a1", &code).is_none());
        s.insert_receipt(member_lock(&s, &grace, "g1"), true);
        assert!(
            s.self_removal_answer("g1", &format!("code {code}"))
                .is_none()
        );
        s.insert_receipt(member_lock(&s, &owner, "o2"), true);
        assert!(
            s.self_removal_answer("o2", &format!("code {code}"))
                .is_none()
        );
        // Spoken code confirmed, but a click is still required...
        assert!(
            s.authorise_removal(&local(), &ada, "Ada", RemovalRoute::SelfService)
                .is_err()
        );
        s.insert_receipt(member_lock(&s, &ada, "a2"), true);
        assert!(
            s.self_removal_answer("a2", &format!("remove me, code {code}"))
                .unwrap()
                .is_ok()
        );
        assert_eq!(
            s.self_removal_status()["stage"],
            "awaiting_local_confirmation"
        );
        // ...for Ada only, with her name, and never by a voice.
        assert!(
            s.authorise_removal(&local(), &grace, "Grace", RemovalRoute::SelfService)
                .is_err()
        );
        assert!(
            s.authorise_removal(&local(), &ada, "Grace", RemovalRoute::SelfService)
                .is_err()
        );
        let ada_voice = Actor::Voice(member_lock(&s, &ada, "a3"));
        assert!(
            s.authorise_removal(&ada_voice, &ada, "Ada", RemovalRoute::SelfService)
                .is_err()
        );
        let ticket = s
            .authorise_removal(&local(), &ada, "Ada", RemovalRoute::SelfService)
            .unwrap();
        assert_eq!(ticket.route(), RemovalRoute::SelfService);
        // The confirmation is single use.
        assert!(
            s.authorise_removal(&local(), &ada, "Ada", RemovalRoute::SelfService)
                .is_err()
        );
        s.remove_member(ticket).unwrap();
        assert!(s.roster().unwrap().profile(&ada).is_none());
        assert!(s.roster().unwrap().profile(&grace).is_some());
        assert!(s.self_removal_status().is_null());
    }

    #[test]
    fn stale_locks_and_wrong_codes_cannot_complete_self_removal() {
        let dir = tempfile::tempdir().unwrap();
        let (s, _, ada, _) = household(dir.path());
        s.insert_receipt(member_lock(&s, &ada, "a1"), true);
        let code = code_from(&s.self_removal_request("a1").unwrap());
        // Re-teaching Ada's voice bumps her epoch: the old receipt and the
        // pending request are both stale.
        let mut r = s.roster().unwrap();
        r.set_voice_enrolled(&local(), &ada, true).unwrap();
        s.save_roster(r).unwrap();
        assert!(s.self_removal_answer("a1", &code).is_none());
        s.insert_receipt(member_lock(&s, &ada, "a2"), true);
        assert!(
            s.self_removal_answer("a2", &format!("code {code}"))
                .unwrap()
                .is_err()
        );
        assert!(s.self_removal_status().is_null());
        assert!(
            s.authorise_removal(&local(), &ada, "Ada", RemovalRoute::SelfService)
                .is_err()
        );
        // Three wrong codes end the request; nothing is deleted.
        s.insert_receipt(member_lock(&s, &ada, "b1"), true);
        let code = code_from(&s.self_removal_request("b1").unwrap());
        let wrong = if code == "0000" { "1111" } else { "0000" };
        for (i, t) in ["b2", "b3", "b4"].iter().enumerate() {
            s.insert_receipt(member_lock(&s, &ada, t), true);
            let answer = s.self_removal_answer(t, &format!("code {wrong}")).unwrap();
            assert!(answer.is_err(), "{i}");
        }
        assert!(s.self_removal_status().is_null());
        assert!(s.roster().unwrap().profile(&ada).is_some());
        // A local cancel also ends it.
        s.insert_receipt(member_lock(&s, &ada, "c1"), true);
        s.self_removal_request("c1").unwrap();
        s.self_removal_cancel();
        assert!(s.self_removal_status().is_null());
    }

    #[test]
    fn removal_deletes_the_voice_even_without_the_model_and_readding_starts_fresh() {
        use notdot_speakers::matcher::Voiceprint;
        let dir = tempfile::tempdir().unwrap();
        let keys = SharedKeys::default();
        let s = open(dir.path(), dir.path().join("missing.onnx"), &keys);
        let mut r = s.roster().unwrap();
        let owner = r.owner().clone();
        let ada = r.add_member(&local(), "Ada").unwrap();
        r.set_voice_enrolled(&local(), &owner, true).unwrap();
        r.set_voice_enrolled(&local(), &ada, true).unwrap();
        s.save_roster(r).unwrap();
        let store = VoiceprintStore::new(dir.path().join(VOICEPRINT_FILE), Box::new(keys.clone()));
        let print = |id: &ProfileId| Voiceprint {
            profile: id.to_string(),
            samples: vec![vec![0.5; 8]; 3],
        };
        store.save(&[1; 32], &[print(&owner), print(&ada)]).unwrap();
        let ticket = s
            .authorise_removal(&local(), &ada, "Ada", RemovalRoute::Owner)
            .unwrap();
        assert!(s.remove_member(ticket).unwrap().voice_deleted);
        let left = store.load(&[1; 32]).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].profile, owner.to_string());
        let again = s.add_member(&local(), "Ada").unwrap();
        assert_ne!(again, ada);
        let p = s.roster().unwrap().profile(&again).unwrap().clone();
        assert!(
            !p.voice_enrolled && p.voice_consent_unix_ms.is_none() && p.companion_thread.is_none()
        );
        assert!(!dir.path().join("profiles").join(again.as_str()).exists());
    }

    #[test]
    fn owner_must_be_named_and_consent_recorded_before_saving_a_voice() {
        let dir = tempfile::tempdir().unwrap();
        let s = Speakers::with_keys(
            dir.path().into(),
            dir.path().join("m.onnx"),
            Box::new(SharedKeys::default()),
            notdot_core::profiles::OWNER_PLACEHOLDER_NAME,
        );
        let owner = s.roster().unwrap().owner().clone();
        assert_eq!(
            s.enrol_begin(&local(), &owner).unwrap_err(),
            "Tell notdot what to call you first."
        );
        assert_eq!(s.summary()["people"][0]["needs_name"], true);
        s.rename(&local(), &owner, "Kamba").unwrap();
        assert_eq!(s.summary()["people"][0]["needs_name"], false);
        assert!(
            s.enrol_save(&local(), false)
                .unwrap_err()
                .contains("agrees")
        );
    }

    /// Extractor whose "voice" is the loudness of the audio: quiet = one
    /// person, loud = another. Exercises continuity wiring, not acoustics.
    struct Loudness;
    impl Extractor for Loudness {
        fn embed(&self, _: u32, v: &[f32]) -> Result<Vec<f32>, String> {
            let rms = (v.iter().map(|x| x * x).sum::<f32>() / v.len().max(1) as f32).sqrt();
            Ok(if rms < 0.25 {
                vec![1.0, 0.0, 0.0]
            } else {
                vec![0.0, 1.0, 0.0]
            })
        }
        fn dim(&self) -> usize {
            3
        }
        fn model_id(&self) -> [u8; 32] {
            [0; 32]
        }
    }
    fn tone(seconds: usize, amp: f32) -> Vec<u8> {
        (0..16_000 * seconds)
            .flat_map(|i| ((amp * (i as f32 * 0.05).sin() * 32767.0) as i16).to_le_bytes())
            .collect()
    }

    #[tokio::test]
    async fn a_change_of_speaker_after_admission_blocks_the_capture() {
        use notdot_speakers::matcher::Voiceprint;
        let dir = tempfile::tempdir().unwrap();
        let (s, owner, ada, _) = household(dir.path());
        // Enrolment samples near (not identical to) each "voice", so a match
        // is not mistaken for a replayed enrolment clip.
        let n = |v: Vec<f32>| notdot_speakers::embedding::normalise(v).unwrap();
        let near = |x: f32, y: f32| {
            vec![
                n(vec![x, y, 0.3]),
                n(vec![x, y, -0.3]),
                n(vec![x + 0.3 * y, y + 0.3 * x, 0.0]),
            ]
        };
        s.install_engine(Engine::new(
            Arc::new(Loudness),
            vec![
                Voiceprint {
                    profile: owner.to_string(),
                    samples: near(1.0, 0.0),
                },
                Voiceprint {
                    profile: ada.to_string(),
                    samples: near(0.0, 1.0),
                },
            ],
            Thresholds::default(),
        ));
        // The owner alone: admitted and usable after the check.
        let alone = tone(6, 0.2);
        assert_eq!(
            s.identify("one", 16_000, &alone).await.unwrap().profile,
            owner
        );
        assert_eq!(s.receipt("one"), Err(SPEAKER_UNCHECKED.into()));
        s.verify_continuity("one", 16_000, &alone).await.unwrap();
        assert_eq!(s.receipt("one").unwrap().profile, owner);
        // The owner starts, Ada finishes: admitted as the owner on the first
        // seconds, then refused before anything is sent.
        let mut handover = tone(3, 0.2);
        handover.extend(tone(4, 0.6));
        assert_eq!(
            s.identify("two", 16_000, &handover[..16_000 * 2 * 2])
                .await
                .unwrap()
                .profile,
            owner
        );
        assert_eq!(
            s.verify_continuity("two", 16_000, &handover).await,
            Err(SPEAKER_CHANGED.into())
        );
        assert_eq!(s.receipt("two"), Err(SPEAKER_CHANGED.into()));
        // A rejected capture stays rejected.
        assert_eq!(
            s.verify_continuity("two", 16_000, &alone).await,
            Err(SPEAKER_CHANGED.into())
        );
    }

    #[tokio::test]
    async fn continuity_fails_closed_without_the_model() {
        let dir = tempfile::tempdir().unwrap();
        let (s, _, ada, _) = household(dir.path());
        s.insert_receipt(member_lock(&s, &ada, "a1"), false);
        assert_eq!(
            s.verify_continuity("a1", 16_000, &tone(3, 0.2)).await,
            Err(SPEAKER_UNCHECKED.into())
        );
        assert!(s.receipt("a1").is_err());
        assert!(
            s.verify_continuity("unknown", 16_000, &tone(3, 0.2))
                .await
                .is_err()
        );
    }

    // ------------------------------------------------------------------
    // Full flow with the real model and synthetic voices (see
    // crates/notdot-speakers/tests/acoustic.rs for fixture layout).

    fn wav_pcm(path: &Path) -> (u32, Vec<u8>) {
        let bytes = std::fs::read(path).unwrap();
        let rate = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
        let data = bytes.windows(4).position(|w| w == b"data").unwrap();
        (rate, bytes[data + 8..].to_vec())
    }
    async fn enrol(s: &Speakers, who: &ProfileId, voice: &Path) {
        let p = s.enrol_begin(&local(), who).unwrap();
        assert!(matches!(
            p,
            Progress::Prompt {
                index: 1,
                of: 4,
                ..
            }
        ));
        for i in 0..4 {
            let token = format!("enrol-{who}-{i}");
            s.enrol_capture(&token).unwrap();
            assert!(s.is_enrol_capture(&token));
            let (rate, pcm) = wav_pcm(&voice.join(format!("enrol-{i}.wav")));
            for chunk in pcm.chunks(3200) {
                assert!(s.enrol_audio(&token, rate, chunk));
            }
            let p = s.enrol_capture_end(&token).await.unwrap();
            assert!(
                !matches!(p, Progress::Retry { .. } | Progress::Failed { .. }),
                "{p:?}"
            );
        }
        s.enrol_save(&local(), true).unwrap();
    }

    #[tokio::test]
    #[ignore = "needs NOTDOT_SPEAKER_MODEL and NOTDOT_SPEAKER_FIXTURES"]
    async fn end_to_end_identity_privacy_and_recovery() {
        let model = PathBuf::from(std::env::var_os("NOTDOT_SPEAKER_MODEL").unwrap());
        let fx = PathBuf::from(std::env::var_os("NOTDOT_SPEAKER_FIXTURES").unwrap());
        let dir = tempfile::tempdir().unwrap();
        let keys = SharedKeys::default();
        let s = open(dir.path(), model.clone(), &keys);
        let owner = s.roster().unwrap().owner().clone();
        let ada = s.add_member(&local(), "Ada").unwrap();

        // Cancelled enrolment leaves nothing behind.
        s.enrol_begin(&local(), &owner).unwrap();
        s.enrol_capture("t").unwrap();
        let (rate, pcm) = wav_pcm(&fx.join("daniel/enrol-0.wav"));
        s.enrol_audio("t", rate, &pcm);
        s.enrol_cancel();
        assert!(s.enrol_capture_end("t").await.is_err());
        assert!(s.enrol_save(&local(), true).is_err());
        assert!(!dir.path().join(VOICEPRINT_FILE).exists());

        enrol(&s, &owner, &fx.join("daniel")).await;
        enrol(&s, &ada, &fx.join("karen")).await;
        let raw = std::fs::read(dir.path().join(VOICEPRINT_FILE)).unwrap();
        assert!(!raw.windows(8).any(|w| w == &owner.as_str().as_bytes()[..8]));
        assert!(!raw.windows(3).any(|w| w == b"Ada"));
        let roster_text = std::fs::read_to_string(dir.path().join(ROSTER_FILE)).unwrap();
        assert!(roster_text.contains("Ada") && !roster_text.contains("samples"));

        let say = |who: &str, i: usize| wav_pcm(&fx.join(who).join(format!("test-{i}.wav")));
        s.set_mode(&local(), IdentityMode::TrustedGroup).unwrap();
        let (r, p) = say("daniel", 1);
        assert_eq!(s.identify("o1", r, &p).await.unwrap().profile, owner);
        let (r, p) = say("karen", 2);
        let ada_lock = s.identify("a1", r, &p).await.unwrap();
        s.verify_continuity("a1", r, &p).await.unwrap();
        assert!(s.receipt("a1").is_ok());
        // The owner starts and Ada takes over: refused before anything is sent.
        let (r2, mut handover) = say("daniel", 3);
        handover.truncate(r2 as usize * 2 * 5 / 2);
        handover.extend(vec![0u8; r2 as usize / 2]);
        handover.extend(say("karen", 5).1);
        // Admission sees only the first two seconds, as in the capture path.
        let prefix = &handover[..r2 as usize * 2 * 2];
        assert_eq!(s.identify("h1", r2, prefix).await.unwrap().profile, owner);
        assert_eq!(
            s.verify_continuity("h1", r2, &handover).await,
            Err(SPEAKER_CHANGED.into())
        );
        assert!(s.receipt("h1").is_err());
        assert_eq!(
            (ada_lock.profile.clone(), ada_lock.role),
            (ada.clone(), Role::Member)
        );
        let (r, p) = say("fred", 3);
        assert_eq!(
            s.identify_detailed("u1", r, &p).await,
            Err(Refusal::UnrecognisedVoice)
        );
        assert!(s.receipt("u1").is_err());
        let (r, p) = wav_pcm(&fx.join("daniel/enrol-2.wav"));
        assert_eq!(
            s.identify_detailed("replay", r, &p).await,
            Err(Refusal::UncertainVoice)
        );

        // Owner-only: the member is refused and their in-flight turn revoked.
        s.set_mode(&local(), IdentityMode::OwnerOnly).unwrap();
        assert!(s.receipt("a1").is_err());
        let (r, p) = say("karen", 4);
        assert_eq!(
            s.identify_detailed("a2", r, &p).await,
            Err(Refusal::OwnerOnly)
        );

        // Restart with the same keychain: voices survive, still encrypted.
        drop(s);
        let s = open(dir.path(), model.clone(), &keys);
        let (r, p) = say("daniel", 5);
        assert_eq!(s.identify("o2", r, &p).await.unwrap().profile, owner);

        // Key lost: locked mode fails closed, then local recovery.
        drop(s);
        let s = open(dir.path(), model.clone(), &SharedKeys::default());
        let (r, p) = say("daniel", 6);
        assert_eq!(
            s.identify_detailed("o3", r, &p).await,
            Err(Refusal::IdentityUnavailable)
        );
        assert!(matches!(s.health(), Health::VoicesUnavailable(_)));
        assert!(s.enrol_begin(&local(), &owner).is_err());
        s.forget_all_voices(&local()).unwrap();
        assert!(!dir.path().join(VOICEPRINT_FILE).exists());
        assert_eq!(s.roster().unwrap().mode(), IdentityMode::Open);
        // People and grants remain; only voices were forgotten.
        assert!(s.roster().unwrap().profile(&ada).is_some());
        enrol(&s, &owner, &fx.join("daniel")).await;
        let (r, p) = say("daniel", 7);
        assert_eq!(s.identify("o4", r, &p).await.unwrap().profile, owner);
    }
}
