//! Interrupting a spoken reply by saying the wake phrase.
//!
//! While a reply plays, the webview captures the microphone with its own
//! echo cancellation, which uses notdot's playback as the reference, and
//! sends 16 kHz PCM here over local IPC. The on-device wake detector runs on
//! it. Nothing leaves the computer until the wake phrase is detected; the
//! request after it is captured and recognised exactly like a click.
//!
//! The feature only runs when the webview has reported a working echo-
//! cancelled track in this session. Without it, overlapping reply audio hides
//! the wake phrase and a reply that says the wake phrase can trigger itself,
//! so the control stays off rather than running unfiltered.
use crate::AppState;
use serde::Serialize;
use std::{
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::State;

/// One chunk is at most 300 ms of 16 kHz audio.
pub const MAX_CHUNK: usize = 4_800;
/// A detection must be followed by its capture within this time.
const CAPTURE_WINDOW: Duration = Duration::from_secs(3);
/// A session that receives no audio for this long has ended.
const IDLE_LIMIT: Duration = Duration::from_secs(5);

pub struct Interrupt {
    inner: Mutex<Inner>,
    /// The detector and the wake phrase it was built for.
    detector: Mutex<Option<(String, crate::wake::standalone::Detector)>>,
    models: PathBuf,
}
#[derive(Default)]
struct Inner {
    /// The webview reported an echo-cancelled track in this session.
    support: Option<Support>,
    session: Option<Session>,
    /// A detected session waiting for its request capture.
    triggered: Option<(String, Owner, Instant)>,
    /// The capture of the request after a detected wake phrase.
    capture: Option<(String, Owner)>,
}
#[derive(Clone, Serialize)]
pub struct Support {
    pub echo_cancellation: bool,
    pub detail: String,
}
/// Whose reply is being interrupted: the person, their permission epoch and
/// the exact spoken turn. Every later step must match it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub profile: notdot_core::profiles::ProfileId,
    pub epoch: u64,
    pub turn: String,
}
struct Session {
    id: String,
    last_audio: Instant,
    owner: Owner,
}

impl Interrupt {
    pub fn new(models: PathBuf) -> Self {
        Self {
            inner: Mutex::default(),
            detector: Mutex::default(),
            models,
        }
    }
    /// Release the detector when interruption is turned off.
    pub fn unload(&self) {
        self.end();
        if let Ok(mut d) = self.detector.lock() {
            *d = None;
        }
    }
    pub fn supported(&self) -> bool {
        self.inner
            .lock()
            .ok()
            .and_then(|i| i.support.clone())
            .is_some_and(|s| s.echo_cancellation)
    }
    /// Revoke listening, a pending detection and a pending capture (settings,
    /// mute, stop, update install, person change).
    pub fn end(&self) {
        if let Ok(mut i) = self.inner.lock() {
            i.session = None;
            i.triggered = None;
            i.capture = None;
        }
    }
    /// The owner of an interruption capture, if `token` is one.
    pub fn capture_owner(&self, token: &str) -> Option<Owner> {
        self.inner.lock().ok().and_then(|i| {
            i.capture
                .as_ref()
                .filter(|(t, _)| t == token)
                .map(|(_, owner)| owner.clone())
        })
    }
    /// Single use: the capture's result is handled once.
    pub fn finish_capture(&self, token: &str) -> Option<Owner> {
        let mut i = self.inner.lock().ok()?;
        match i.capture.take() {
            Some((t, owner)) if t == token => Some(owner),
            other => {
                i.capture = other;
                None
            }
        }
    }
    /// A capture for `session` may start during a reply only just after its
    /// wake phrase was detected. Single use; returns the reply's owner.
    pub fn take_trigger(&self, session: &str, token: &str) -> Option<Owner> {
        let mut i = self.inner.lock().ok()?;
        match i.triggered.take() {
            Some((id, owner, at)) if id == session && at.elapsed() < CAPTURE_WINDOW => {
                i.capture = Some((token.to_owned(), owner.clone()));
                Some(owner)
            }
            _ => None,
        }
    }
    #[cfg(test)]
    pub fn trigger_for_test(&self, owner: Owner) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let mut i = self.inner.lock().unwrap();
        i.support = Some(Support {
            echo_cancellation: true,
            detail: "test".into(),
        });
        i.triggered = Some((id.clone(), owner, Instant::now()));
        id
    }
}

/// Why listening is not allowed right now, checked natively on every call.
async fn refusal(state: &AppState) -> Option<&'static str> {
    let s = state.snapshot.lock().await;
    if !s.settings.interrupt_enabled {
        Some("Interrupting by voice is off")
    } else if !s.settings.wake_enabled {
        Some("Interrupting needs wake listening")
    } else if s.settings.microphone_muted {
        Some("The microphone is muted")
    } else if !s.window_visible || !s.settings.onboarding_complete || !s.key_configured {
        Some("notdot is not ready to listen")
    } else if state.speakers.enrol_capture_active() {
        Some("A voice is being taught")
    } else if matches!(s.updates.phase.as_str(), "downloading" | "installing") {
        Some("notdot is updating")
    } else {
        None
    }
}

/// The current owner of the reply that is speaking, or why there is none.
pub async fn current_owner(state: &AppState) -> Result<Owner, String> {
    let profile = state.context().id.clone();
    let epoch = state
        .speakers
        .roster()?
        .profile(&profile)
        .ok_or("This person is no longer set up")?
        .epoch;
    let turn = state.snapshot.lock().await.speech.turn.clone();
    if turn.is_empty() {
        return Err("Nothing is being said".into());
    }
    Ok(Owner {
        profile,
        epoch,
        turn,
    })
}
/// Checked before a capture's speaker is admitted: an interruption may only
/// come from the person whose reply is held, with unchanged permissions,
/// while that same reply is still current. A refusal never switches person.
pub async fn admit(state: &AppState, token: &str, identity: &TurnLock) -> Result<(), String> {
    let Some(owner) = state.interrupt.capture_owner(token) else {
        return Ok(());
    };
    let current = current_owner(state).await;
    let identification = state.speakers.needs_identification();
    if identity.profile != owner.profile
        || identity.epoch != owner.epoch
        || (identification && identity.open_fallback)
        || current.as_ref() != Ok(&owner)
    {
        return Err("I didn't recognise that voice, so I'll carry on.".into());
    }
    Ok(())
}
use notdot_core::profiles::TurnLock;

#[tauri::command]
pub async fn interrupt_support(
    state: State<'_, std::sync::Arc<AppState>>,
    echo_cancellation: bool,
    detail: String,
) -> Result<(), String> {
    let mut i = state
        .interrupt
        .inner
        .lock()
        .map_err(|_| "interrupt state")?;
    i.support = Some(Support {
        echo_cancellation,
        detail: detail.chars().take(200).collect(),
    });
    if !echo_cancellation {
        i.session = None;
        i.triggered = None;
    }
    Ok(())
}
#[tauri::command]
pub fn interrupt_status(state: State<'_, std::sync::Arc<AppState>>) -> Option<Support> {
    state
        .interrupt
        .inner
        .lock()
        .ok()
        .and_then(|i| i.support.clone())
}

/// Start listening for the wake phrase while `turn`'s reply plays.
#[tauri::command]
pub async fn interrupt_begin(
    state: State<'_, std::sync::Arc<AppState>>,
    turn: String,
) -> Result<String, String> {
    if let Some(why) = refusal(&state).await {
        return Err(why.into());
    }
    if !state.interrupt.supported() {
        return Err("Interrupting needs echo cancellation, which isn't available".into());
    }
    let owner = current_owner(&state).await?;
    if owner.turn != turn {
        return Err("That reply is no longer speaking".into());
    }
    let phrase = {
        let s = state.snapshot.lock().await;
        crate::wake::phrase(&s.settings.name, s.settings.wake_name_only)?
    };
    let interrupt = state.inner().clone();
    let models = interrupt.interrupt.models.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut slot = interrupt
            .interrupt
            .detector
            .lock()
            .map_err(|_| "interrupt detector")?;
        match slot.as_mut() {
            Some((built_for, detector)) if *built_for == phrase => detector.reset(),
            // The name or "name only" changed: build for the new phrase.
            _ => {
                *slot = Some((
                    phrase.clone(),
                    crate::wake::standalone::Detector::new(&models, &phrase)?,
                ))
            }
        }
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| e.to_string())??;
    // Re-check after loading: the reply or person may have changed.
    if current_owner(&state).await.as_ref() != Ok(&owner) {
        return Err("That reply is no longer speaking".into());
    }
    let id = uuid::Uuid::new_v4().to_string();
    let mut i = state
        .interrupt
        .inner
        .lock()
        .map_err(|_| "interrupt state")?;
    i.session = Some(Session {
        id: id.clone(),
        last_audio: Instant::now(),
        owner,
    });
    i.triggered = None;
    i.capture = None;
    Ok(id)
}
/// Feed echo-cancelled 16 kHz audio. Returns true when the wake phrase is
/// heard; the session then ends and waits for its request capture.
#[tauri::command]
pub async fn interrupt_audio(
    state: State<'_, std::sync::Arc<AppState>>,
    session: String,
    samples: Vec<f32>,
    guarded: bool,
) -> Result<bool, String> {
    if samples.len() > MAX_CHUNK || samples.iter().any(|s| !s.is_finite()) {
        return Err("Invalid audio".into());
    }
    if let Some(why) = refusal(&state).await {
        state.interrupt.end();
        return Err(why.into());
    }
    let current = current_owner(&state).await.ok();
    let owner = {
        let mut i = state
            .interrupt
            .inner
            .lock()
            .map_err(|_| "interrupt state")?;
        let Some(open) = i.session.as_mut().filter(|s| s.id == session) else {
            return Err("Interrupt listening ended".into());
        };
        if current.as_ref() != Some(&open.owner) || open.last_audio.elapsed() > IDLE_LIMIT {
            i.session = None;
            return Err("Interrupt listening ended".into());
        }
        open.last_audio = Instant::now();
        open.owner.clone()
    };
    let interrupt = state.inner().clone();
    let heard = tauri::async_runtime::spawn_blocking(move || {
        let mut detector = interrupt.interrupt.detector.lock().ok()?;
        let (_, detector) = detector.as_mut()?;
        let mut heard = false;
        for chunk in samples.chunks(320) {
            heard |= detector.accept(16_000, chunk);
        }
        Some(heard)
    })
    .await
    .map_err(|e| e.to_string())?
    .ok_or("Interrupt detector unavailable")?;
    // While a reply sentence that contains the wake name plays, a detection
    // is treated as notdot hearing itself.
    if !heard || guarded {
        return Ok(false);
    }
    // Revalidate after detection work: the same reply must still be playing.
    if current_owner(&state).await.ok().as_ref() != Some(&owner) {
        state.interrupt.end();
        return Ok(false);
    }
    let mut i = state
        .interrupt
        .inner
        .lock()
        .map_err(|_| "interrupt state")?;
    if i.session.as_ref().is_none_or(|s| s.id != session) {
        return Ok(false);
    }
    i.session = None;
    i.triggered = Some((session, owner, Instant::now()));
    Ok(true)
}
#[tauri::command]
pub fn interrupt_end(state: State<'_, std::sync::Arc<AppState>>) {
    state.interrupt.end();
}

/// What an interruption's recognised request means.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Keep the original reply: silence, notdot's own words, or a refusal.
    Resume(&'static str),
    /// Stop speaking and wait.
    Stop,
    /// A new request or correction.
    Request(String),
}
fn words(text: &str) -> Vec<String> {
    // Normalise curly apostrophes before splitting: "that’s all" is one phrase.
    text.replace(['’', '‘'], "'")
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .map(|w| w.trim_matches('\''))
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}
/// Classify a recognised interruption against the reply that was playing.
pub fn classify(transcript: &str, reply: &str, wake_name: &str) -> Outcome {
    let name = words(wake_name);
    let mut heard = words(transcript);
    // Drop a repeated wake phrase at the start: "hey ava, stop".
    if heard.first().map(String::as_str) == Some("hey") {
        heard.remove(0);
    }
    // An empty or punctuation-only name has no words to drop.
    while !name.is_empty() && heard.len() >= name.len() && heard[..name.len()] == name[..] {
        heard.drain(..name.len());
    }
    if heard.is_empty() {
        return Outcome::Resume("I didn't hear a request, so I'll carry on.");
    }
    let joined = heard.join(" ");
    if matches!(
        joined.as_str(),
        "stop"
            | "stop talking"
            | "stop it"
            | "that's enough"
            | "that's all"
            | "enough"
            | "be quiet"
            | "quiet"
            | "cancel"
            | "never mind"
            | "shush"
            | "hush"
            | "stop please"
            | "please stop"
            | "ok stop"
            | "okay stop"
    ) {
        return Outcome::Stop;
    }
    // notdot's own words picked up despite echo cancellation: most of what
    // was heard is a contiguous run of the reply text.
    let reply = words(reply);
    if heard.len() >= 2 && !reply.is_empty() {
        let longest = (0..reply.len())
            .map(|start| {
                heard
                    .iter()
                    .zip(&reply[start..])
                    .take_while(|(a, b)| a == b)
                    .count()
            })
            .max()
            .unwrap_or(0);
        let within = (0..heard.len())
            .map(|from| {
                (0..reply.len())
                    .map(|start| {
                        heard[from..]
                            .iter()
                            .zip(&reply[start..])
                            .take_while(|(a, b)| a == b)
                            .count()
                    })
                    .max()
                    .unwrap_or(0)
            })
            .max()
            .unwrap_or(0)
            .max(longest);
        if within * 10 >= heard.len() * 7 {
            return Outcome::Resume("That sounded like my own voice, so I'll carry on.");
        }
    }
    Outcome::Request(transcript.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interruptions_are_classified_without_guessing() {
        let reply =
            "The forecast for Leeds is mild with light rain this afternoon, then clearer skies.";
        assert_eq!(
            classify("", reply, "Ava"),
            Outcome::Resume("I didn't hear a request, so I'll carry on.")
        );
        assert_eq!(
            classify("Hey Ava", reply, "Ava"),
            Outcome::Resume("I didn't hear a request, so I'll carry on.")
        );
        assert_eq!(classify("Hey Ava, stop.", reply, "Ava"), Outcome::Stop);
        assert_eq!(classify("stop talking", reply, "Ava"), Outcome::Stop);
        assert!(matches!(
            classify("light rain this afternoon, then clearer", reply, "Ava"),
            Outcome::Resume(_)
        ));
        assert_eq!(
            classify("Actually, I meant Manchester", reply, "Ava"),
            Outcome::Request("Actually, I meant Manchester".into())
        );
        assert_eq!(
            classify("what about tomorrow", reply, "Ava"),
            Outcome::Request("what about tomorrow".into())
        );
        // A single shared word is not treated as echo.
        assert!(matches!(
            classify("Leeds", reply, "Ava"),
            Outcome::Request(_)
        ));
        // Curly apostrophes from recognition are normal speech.
        assert_eq!(classify("That’s all.", reply, "Ava"), Outcome::Stop);
        assert_eq!(
            classify("Hey Ava, that’s enough", reply, "Ava"),
            Outcome::Stop
        );
        // An empty or punctuation-only name cannot loop or drop words.
        assert_eq!(classify("stop", reply, ""), Outcome::Stop);
        assert_eq!(
            classify("what about tomorrow", reply, "..."),
            Outcome::Request("what about tomorrow".into())
        );
    }

    async fn ready(state: &std::sync::Arc<AppState>, turn: &str) {
        let mut s = state.snapshot.lock().await;
        s.settings.interrupt_enabled = true;
        s.settings.wake_enabled = true;
        s.settings.onboarding_complete = true;
        s.key_configured = true;
        s.speech = notdot_core::spoken_reply::SpokenReply::immediate(turn.into(), "Reply".into());
    }
    fn lock(state: &AppState, token: &str, profile: &notdot_core::profiles::ProfileId) -> TurnLock {
        let notdot_core::profiles::Admission::Admit(lock) = state.speakers.roster().unwrap().admit(
            token,
            &notdot_core::profiles::SpeakerOutcome::Recognised {
                profile: profile.clone(),
                score: 0.99,
            },
        ) else {
            panic!("admission")
        };
        lock
    }
    #[tokio::test]
    async fn only_the_reply_owner_can_interrupt_and_a_refusal_never_switches_person() {
        use notdot_core::profiles::{Actor, SpeakerOutcome};
        let dir = tempfile::tempdir().unwrap();
        let state = crate::test_support::state(dir.path());
        let mut roster = state.speakers.roster().unwrap();
        let owner = roster.owner().clone();
        let member = roster.add_member(&Actor::LocalDevice, "Member").unwrap();
        for id in [owner.clone(), member.clone()] {
            roster
                .set_voice_enrolled(&Actor::LocalDevice, &id, true)
                .unwrap();
        }
        state.speakers.save_roster(roster).unwrap();
        crate::test_support::admit(
            &state,
            SpeakerOutcome::Recognised {
                profile: owner.clone(),
                score: 0.99,
            },
        )
        .await
        .unwrap();
        ready(&state, "owner-reply").await;
        let reply_owner = current_owner(&state).await.unwrap();
        assert_eq!(reply_owner.profile, owner);

        // A trusted member says the wake phrase over the owner's reply.
        let session = state.interrupt.trigger_for_test(reply_owner.clone());
        let token = "interrupt-member";
        assert_eq!(
            state.interrupt.take_trigger(&session, token),
            Some(reply_owner.clone())
        );
        let refused = crate::profile_access::bind_capture(
            state.clone(),
            token,
            16_000,
            &[],
            Some(lock(&state, token, &member)),
        )
        .await;
        assert!(refused.unwrap_err().contains("carry on"));
        assert_eq!(
            state.context().id,
            owner,
            "a refusal must not switch person"
        );
        assert_eq!(state.snapshot.lock().await.speech.turn, "owner-reply");

        // An unrecognised voice in Anyone mode is refused once voices exist.
        let session = state.interrupt.trigger_for_test(reply_owner.clone());
        let token = "interrupt-stranger";
        state.interrupt.take_trigger(&session, token).unwrap();
        let notdot_core::profiles::Admission::Admit(stranger) = state
            .speakers
            .roster()
            .unwrap()
            .admit(token, &SpeakerOutcome::Unknown)
        else {
            panic!()
        };
        assert!(stranger.open_fallback);
        assert!(
            crate::profile_access::bind_capture(state.clone(), token, 16_000, &[], Some(stranger))
                .await
                .is_err()
        );
        assert_eq!(state.context().id, owner);

        // A detection for an older reply cannot be used for a newer one.
        let session = state.interrupt.trigger_for_test(reply_owner.clone());
        let token = "interrupt-stale";
        state.interrupt.take_trigger(&session, token).unwrap();
        ready(&state, "newer-reply").await;
        assert!(
            crate::profile_access::bind_capture(
                state.clone(),
                token,
                16_000,
                &[],
                Some(lock(&state, token, &owner))
            )
            .await
            .is_err()
        );

        // The owner, for the same reply, is admitted. Detections are single use.
        let reply_owner = current_owner(&state).await.unwrap();
        let session = state.interrupt.trigger_for_test(reply_owner.clone());
        let token = "interrupt-owner";
        state.interrupt.take_trigger(&session, token).unwrap();
        assert_eq!(state.interrupt.take_trigger(&session, "again"), None);
        crate::profile_access::bind_capture(
            state.clone(),
            token,
            16_000,
            &[],
            Some(lock(&state, token, &owner)),
        )
        .await
        .unwrap();
        assert_eq!(state.interrupt.finish_capture(token), Some(reply_owner));
        assert_eq!(state.interrupt.finish_capture(token), None);
    }
    #[tokio::test]
    async fn listening_is_revoked_by_mute_settings_and_missing_echo_cancellation() {
        let dir = tempfile::tempdir().unwrap();
        let state = crate::test_support::state(dir.path());
        ready(&state, "reply").await;
        let owner = current_owner(&state).await.unwrap();
        let session = state.interrupt.trigger_for_test(owner.clone());
        state.interrupt.end();
        assert_eq!(
            state.interrupt.take_trigger(&session, "t"),
            None,
            "end revokes a detection"
        );
        state.snapshot.lock().await.settings.microphone_muted = true;
        assert_eq!(refusal(&state).await, Some("The microphone is muted"));
        state.snapshot.lock().await.settings.microphone_muted = false;
        state.snapshot.lock().await.settings.interrupt_enabled = false;
        assert!(refusal(&state).await.is_some());
        state.snapshot.lock().await.settings.interrupt_enabled = true;
        state.snapshot.lock().await.updates.phase = "installing".into();
        assert_eq!(refusal(&state).await, Some("notdot is updating"));
        state.snapshot.lock().await.updates.phase = String::new();
        assert_eq!(refusal(&state).await, None);
        // Without a reported echo-cancelled track, nothing starts.
        let fresh = Interrupt::new(dir.path().join("models"));
        assert!(!fresh.supported());
    }
}
