//! Optional interruption without a wake phrase. The webview supplies locally
//! echo-cancelled PCM; Silero detects speech onset and preserves a bounded
//! pre-roll. The same stream continues until silence, with no mic handoff.
//! Only the active reply owner may supply a replacement request. Recognition
//! uses the selected voice backend, and cancellation revokes admission.
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
    detector: Mutex<Option<(String, crate::wake::speech::SpeechGate)>>,
    models: PathBuf,
}
#[derive(Default)]
struct Inner {
    /// The webview reported an echo-cancelled track in this session.
    support: Option<Support>,
    #[cfg(target_os = "linux")]
    permission_until: Option<Instant>,
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
    utterance: Utterance,
}

#[derive(Default)]
struct Utterance {
    audio: zeroize::Zeroizing<Vec<f32>>,
    speaking: bool,
    quiet: usize,
    complete: bool,
}
impl Utterance {
    fn push(&mut self, samples: &[f32], speech: bool) -> bool {
        if self.complete {
            return false;
        }
        let onset = speech && !self.speaking;
        self.speaking |= speech;
        self.audio.extend_from_slice(samples);
        if !self.speaking {
            let excess = self.audio.len().saturating_sub(8_000);
            self.audio.drain(..excess);
        } else {
            self.quiet = if speech {
                0
            } else {
                self.quiet + samples.len()
            };
            self.complete = self.quiet >= 9_600 || self.audio.len() >= 192_000;
            self.audio.truncate(192_000);
        }
        onset
    }
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
            #[cfg(target_os = "linux")]
            {
                i.permission_until = None;
            }
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
        return if token.starts_with("interrupt-") {
            Err("That interruption was cancelled".into())
        } else {
            Ok(())
        };
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

/// A one-use, short-lived browser permission request. This never changes OS
/// microphone permissions. The trusted UI calls it immediately before capture.
#[tauri::command]
pub async fn interrupt_permission(
    window: tauri::WebviewWindow,
    state: State<'_, std::sync::Arc<AppState>>,
) -> Result<(), String> {
    if window.label() != "main" || !window.is_visible().unwrap_or(false) {
        return Err("Show notdot before using the microphone".into());
    }
    if state.snapshot.lock().await.settings.microphone_muted {
        return Err("The microphone is muted".into());
    }
    #[cfg(target_os = "linux")]
    {
        state
            .interrupt
            .inner
            .lock()
            .map_err(|_| "interrupt state")?
            .permission_until = Some(Instant::now() + Duration::from_secs(6));
    }
    Ok(())
}
#[cfg(any(target_os = "linux", test))]
fn trusted_media_page(label: &str, url: &tauri::Url, development: bool) -> bool {
    if label != "main" || !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    match (url.scheme(), url.host_str(), url.port()) {
        ("tauri", Some("localhost"), None) => true,
        ("http" | "https", Some("tauri.localhost"), None) => true,
        ("http", Some("localhost" | "127.0.0.1"), Some(1420)) if development => true,
        _ => false,
    }
}
#[cfg(any(target_os = "linux", test))]
fn consume_permission(until: &mut Option<Instant>) -> bool {
    until.take().is_some_and(|at| Instant::now() < at)
}
/// WebKitGTK has no browser chrome to ask for media permission. Grant only
/// the app's own main page, following its explicit bounded request above.
#[cfg(target_os = "linux")]
pub fn webview_permission(
    view: tauri::Webview,
    kind: tauri::webview::PermissionKind,
) -> tauri::webview::PermissionResponse {
    use tauri::{
        Manager,
        webview::{PermissionKind, PermissionResponse},
    };
    if !matches!(kind, PermissionKind::Microphone) {
        return PermissionResponse::Default;
    }
    let Ok(url) = view.url() else {
        return PermissionResponse::Deny;
    };
    if !trusted_media_page(view.label(), &url, cfg!(debug_assertions)) {
        return PermissionResponse::Deny;
    }
    let Some(state) = view.try_state::<std::sync::Arc<AppState>>() else {
        return PermissionResponse::Deny;
    };
    let Ok(snapshot) = state.snapshot.try_lock() else {
        return PermissionResponse::Deny;
    };
    if !snapshot.window_visible || snapshot.settings.microphone_muted {
        return PermissionResponse::Deny;
    }
    let Ok(mut inner) = state.interrupt.inner.lock() else {
        return PermissionResponse::Deny;
    };
    if consume_permission(&mut inner.permission_until) {
        PermissionResponse::Allow
    } else {
        PermissionResponse::Deny
    }
}

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
        i.capture = None;
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
    let id = uuid::Uuid::new_v4().to_string();
    let detector_id = id.clone();
    let interrupt = state.inner().clone();
    let models = interrupt.interrupt.models.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut slot = interrupt
            .interrupt
            .detector
            .lock()
            .map_err(|_| "interrupt detector")?;
        if let Some((owner, detector)) = slot.as_mut() {
            detector.reset();
            *owner = detector_id;
        } else {
            *slot = Some((detector_id, crate::wake::speech::SpeechGate::new(&models)?));
        }
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| e.to_string())??;
    // Re-check after loading: the reply or person may have changed.
    if current_owner(&state).await.as_ref() != Ok(&owner) {
        return Err("That reply is no longer speaking".into());
    }
    let mut i = state
        .interrupt
        .inner
        .lock()
        .map_err(|_| "interrupt state")?;
    i.session = Some(Session {
        id: id.clone(),
        last_audio: Instant::now(),
        owner,
        utterance: Utterance::default(),
    });
    i.triggered = None;
    i.capture = None;
    Ok(id)
}
/// Feed echo-cancelled 16 kHz audio. Returns true once at speech onset.
/// Keep feeding the same stream until interrupt_collect finishes.
#[tauri::command]
pub async fn interrupt_audio(
    state: State<'_, std::sync::Arc<AppState>>,
    session: String,
    samples: Vec<f32>,
    guarded: bool,
) -> Result<serde_json::Value, String> {
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
    let packet = samples.clone();
    let detector_session = session.clone();
    let voiced = tauri::async_runtime::spawn_blocking(move || {
        let mut detector = interrupt
            .interrupt
            .detector
            .lock()
            .map_err(|_| "interrupt detector")?;
        let (id, gate) = detector
            .as_mut()
            .ok_or("Interrupt detector unavailable".to_string())?;
        if *id != detector_session {
            return Err("Interrupt listening ended".into());
        }
        gate.accept(&packet, 16_000)
    })
    .await
    .map_err(|e| e.to_string())??;
    // The echo-cancelled stream is classified for speech, not a keyword. Keep
    // the pre-roll so short commands such as Stop are never clipped at onset.
    let _ = guarded; // Compatibility with older webviews; no wake-name guard.
    // Revalidate after detection work: the same reply must still be playing.
    if current_owner(&state).await.ok().as_ref() != Some(&owner) {
        state.interrupt.end();
        return Ok(serde_json::json!(false));
    }
    let mut i = state
        .interrupt
        .inner
        .lock()
        .map_err(|_| "interrupt state")?;
    if i.session.as_ref().is_none_or(|s| s.id != session) {
        return Ok(serde_json::json!(false));
    }
    let onset = i
        .session
        .as_mut()
        .expect("checked session")
        .utterance
        .push(&samples, voiced);
    if onset {
        i.triggered = Some((session, owner, Instant::now()));
    }
    let complete = i.session.as_ref().is_some_and(|s| s.utterance.complete);
    Ok(if complete {
        serde_json::json!("complete")
    } else {
        serde_json::json!(onset)
    })
}
/// Collect the same echo-cancelled stream that detected speech. No microphone
/// handoff, lost first word, temporary WAV or second transcription request.
#[tauri::command]
pub async fn interrupt_collect(
    state: State<'_, std::sync::Arc<AppState>>,
    session: String,
    token: String,
) -> Result<serde_json::Value, String> {
    notdot_core::request_cancel::Requests::validate(&token)?;
    let owner = state
        .interrupt
        .take_trigger(&session, &token)
        .ok_or("Interruption expired")?;
    let started = Instant::now();
    let audio = loop {
        if refusal(&state).await.is_some()
            || current_owner(&state).await.as_ref() != Ok(&owner)
            || state.interrupt.capture_owner(&token).as_ref() != Some(&owner)
        {
            return Err("Interruption cancelled".into());
        }
        let ready = {
            let mut i = state
                .interrupt
                .inner
                .lock()
                .map_err(|_| "interrupt state")?;
            let capture = i
                .session
                .as_mut()
                .filter(|s| s.id == session)
                .ok_or("Interruption cancelled")?;
            if capture.last_audio.elapsed() > IDLE_LIMIT {
                return Err("Microphone stopped".into());
            }
            if capture.utterance.complete {
                Some(std::mem::take(&mut capture.utterance.audio))
            } else {
                None
            }
        };
        if let Some(audio) = ready {
            break audio;
        }
        if started.elapsed() > Duration::from_secs(14) {
            return Err("Interruption timed out".into());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let continuity = if state.speakers.needs_identification() {
        Some((
            16_000,
            zeroize::Zeroizing::new(
                audio
                    .iter()
                    .flat_map(|s| ((s.clamp(-1., 1.) * 32767.) as i16).to_le_bytes())
                    .collect(),
            ),
        ))
    } else {
        None
    };
    let generation = state.quota.generation();
    let provider = state
        .snapshot
        .lock()
        .await
        .settings
        .transcription_id()
        .to_string();
    let heard =
        crate::transcription::interruption(state.inner().clone(), token.clone(), &audio).await;
    let heard = match heard {
        Err(error) if error.kind == notdot_voice::VoiceErrorKind::Refused => {
            Err(crate::Heard::Refused(error.to_string()))
        }
        result => crate::quota::finish(&state, &provider, generation, "stt", result)
            .await
            .map_err(crate::Heard::Failed),
    };
    Ok(crate::interrupt_result(&state, &token, heard, continuity).await)
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
    #[test]
    fn media_permission_is_one_use_expiring_and_restricted_to_owned_pages() {
        let owned = tauri::Url::parse("tauri://localhost/index.html").unwrap();
        assert!(super::trusted_media_page("main", &owned, false));
        assert!(!super::trusted_media_page("orb-input", &owned, false));
        for value in [
            "https://example.com",
            "https://tauri.localhost.evil.test",
            "http://localhost:8109",
            "file:///tmp/page.html",
            "http://user@tauri.localhost",
        ] {
            assert!(
                !super::trusted_media_page("main", &tauri::Url::parse(value).unwrap(), false),
                "{value}"
            );
        }
        let development = tauri::Url::parse("http://localhost:1420").unwrap();
        assert!(!super::trusted_media_page("main", &development, false));
        assert!(super::trusted_media_page("main", &development, true));
        let mut lease = Some(std::time::Instant::now() + std::time::Duration::from_secs(6));
        assert!(super::consume_permission(&mut lease));
        assert!(!super::consume_permission(&mut lease));
        lease = Some(std::time::Instant::now() - std::time::Duration::from_secs(1));
        assert!(!super::consume_permission(&mut lease));
    }

    #[test]
    fn speech_onset_keeps_short_commands_and_bounds_silence_and_audio() {
        let mut capture = super::Utterance::default();
        for _ in 0..100 {
            assert!(!capture.push(&[0.; 1600], false));
        }
        assert_eq!(
            capture.audio.len(),
            8000,
            "idle audio is only a half-second pre-roll"
        );
        assert!(
            capture.push(&[0.2; 1600], true),
            "speech starts without a wake phrase"
        );
        assert!(
            !capture.push(&[0.2; 1600], true),
            "only one onset per utterance"
        );
        for _ in 0..6 {
            capture.push(&[0.; 1600], false);
        }
        assert!(capture.complete, "600 ms silence submits even a short Stop");
        assert!(capture.audio.contains(&0.2), "first word is retained");
        let mut long = super::Utterance::default();
        for _ in 0..200 {
            long.push(&[0.2; 1600], true);
        }
        assert_eq!(
            long.audio.len(),
            192000,
            "all buffers are capped at twelve seconds"
        );
        assert!(long.complete);
    }
    #[test]
    fn shipped_speech_model_detects_speech_without_a_wake_phrase() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut gate = crate::wake::speech::SpeechGate::new(&root.join("resources/wake")).unwrap();
        let mut wav = hound::WavReader::open(root.join("tests/fixtures/negative.wav")).unwrap();
        let rate = wav.spec().sample_rate;
        let pcm: Vec<f32> = wav
            .samples::<i16>()
            .map(|s| f32::from(s.unwrap()) / 32768.)
            .collect();
        let mut heard = false;
        for chunk in pcm.chunks((rate / 10) as usize) {
            heard |= gate.accept(chunk, rate).unwrap();
        }
        assert!(
            heard,
            "ordinary speech with no keyword must open interruption capture"
        );
        gate.reset();
        assert!(
            !gate.accept(&[0.; 16000], 16000).unwrap(),
            "silence is not an interruption"
        );
    }

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

        state.interrupt.end();
        let cancelled_token = "interrupt-cancelled";
        assert!(
            crate::profile_access::bind_capture(
                state.clone(),
                cancelled_token,
                16_000,
                &[],
                Some(lock(&state, cancelled_token, &member))
            )
            .await
            .is_err()
        );
        assert_eq!(
            state.context().id,
            owner,
            "cancelled audio cannot switch profile"
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
