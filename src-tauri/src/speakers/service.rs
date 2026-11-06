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
#[cfg(test)]
use notdot_core::profiles::Outbox;
use notdot_core::profiles::{
    Actor, Admission, Capability, ChatAccess, IdentityMode, LoadOutcome, ProfileId, Refusal,
    Roster, SpeakerOutcome, TurnLock,
};
use notdot_speakers::{
    audio,
    embedding::{CamPlusPlus, Extractor},
    engine::{Engine, IDENTIFY_TIMEOUT_MS, Identification},
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
    receipts: Mutex<VecDeque<(String, Instant, TurnLock)>>,
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
            enrolment: Mutex::new(None),
            enrol_epoch: AtomicU64::new(0),
            #[cfg(test)]
            outbox: Mutex::new(Outbox::new()),
        }
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
        let engine = Arc::new(Engine::new(extractor, prints, Thresholds::default()));
        *slot = Some(engine.clone());
        Ok(engine)
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
                let mut receipts = self.receipts.lock().expect("receipts");
                let now = Instant::now();
                receipts.retain(|(t, at, _)| t != token && now.duration_since(*at) < RECEIPT_TTL);
                while receipts.len() >= MAX_RECEIPTS {
                    receipts.pop_front();
                }
                receipts.push_back((token.to_owned(), now, lock.clone()));
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

    /// The lock recorded for a capture token, re-validated now. Receipts expire
    /// after [`RECEIPT_TTL`]; a stale or revoked one is removed and refused.
    pub fn receipt(&self, token: &str) -> Result<TurnLock, String> {
        let mut receipts = self.receipts.lock().expect("receipts");
        let now = Instant::now();
        receipts.retain(|(_, at, _)| now.duration_since(*at) < RECEIPT_TTL);
        let lock = receipts
            .iter()
            .find(|(t, _, _)| t == token)
            .map(|(_, _, l)| l.clone())
            .ok_or("That request has expired. Please ask again.")?;
        if let Err(e) = self.validate(&lock) {
            receipts.retain(|(t, _, _)| t != token);
            return Err(e);
        }
        Ok(lock)
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

    /// Remove a member. Deletes exactly: their voiceprint, their chat grants,
    /// their roster entry, their receipts and queued announcements, and
    /// `<base>/profiles/<id>/` (their memory ledger and private state). Returns
    /// their companion chat ID: the chat itself lives in Codex and is no longer
    /// reachable from notdot; archiving it there is the caller's step.
    pub fn remove_member(&self, actor: &Actor, id: &ProfileId) -> Result<Option<String>, String> {
        let (dir, thread) = {
            let r = self.roster()?;
            let p = r.profile(id).ok_or("Unknown profile")?;
            (
                self.root.join("profiles").join(id.as_str()),
                p.companion_thread.clone(),
            )
        };
        self.forget_voice(actor, id)?;
        self.mutate(|r| r.remove_member(actor, &self.root, id).map(|_| ()))?;
        #[cfg(test)]
        self.outbox.lock().expect("outbox").forget(id);
        self.receipts
            .lock()
            .expect("receipts")
            .retain(|(_, _, l)| &l.profile != id);
        // App wrapper closes live stores and completes durable profile removal.
        let _ = dir;
        Ok(thread)
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
        if r.profile(id).is_none() {
            return Err("Unknown profile".into());
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
    pub fn enrol_save(&self, actor: &Actor) -> Result<(), String> {
        self.roster()?
            .authorise(actor, Capability::ManageMembers)
            .map_err(|e| e.to_string())?;
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
        if let Err(e) = self.mutate(|r| r.set_voice_enrolled(actor, &profile, true)) {
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
        assert_eq!(
            s.remove_member(&local(), &ada).unwrap().as_deref(),
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
                assert!(s.receipt(&token).is_ok());
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
        assert!(s.enrol_save(&local()).is_err());
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
        s.enrol_save(&local()).unwrap();
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
        assert!(s.enrol_save(&local()).is_err());
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
