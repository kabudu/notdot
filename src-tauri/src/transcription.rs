//! One bounded streaming session for the current activated recording. Recoverable failures
//! preserve the local WAV for a retry only when the backend supports batch STT.
//! Cancellation, authentication, quota and permission failures never trigger a retry.
use crate::{AppState, recognition_backend, wake::CaptureEvent};
use notdot_voice::{VoiceError, VoiceErrorKind, realtime::StreamStats};
use serde::Serialize;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter};
use tokio::{
    sync::{Mutex, mpsc, oneshot},
    task::JoinHandle,
};
#[derive(Clone, Default, Serialize)]
pub struct Report {
    pub mode: String,
    pub sample_rate: u32,
    pub stats_available: bool,
    pub audio_chunks: u64,
    pub partial_events: u64,
    pub committed_segments: u64,
    pub completed_ms: u64,
    pub first_audio_ms: u64,
    pub first_partial_ms: u64,
    pub capture_end_ms: u64,
    pub error_kind: Option<VoiceErrorKind>,
}
enum Input {
    Audio { sequence: u64, pcm: Vec<u8> },
    Finish { sequence: u64 },
}
struct Completion {
    result: Result<String, VoiceError>,
    stats: Option<StreamStats>,
}
impl Completion {
    fn failed(error: VoiceError) -> Self {
        Self {
            result: Err(error),
            stats: None,
        }
    }
}
struct Session {
    token: String,
    input: mpsc::Sender<Input>,
    result: Option<oneshot::Receiver<Completion>>,
    task: JoinHandle<()>,
    cancelled: Arc<AtomicBool>,
    started: Instant,
}
#[derive(Default)]
pub struct Streaming {
    inner: Mutex<Option<Session>>,
    report: Mutex<Report>,
    cancelled: Mutex<notdot_core::request_cancel::Requests>,
}
impl Streaming {
    #[cfg(debug_assertions)]
    pub async fn report(&self) -> Report {
        self.report.lock().await.clone()
    }
    pub async fn cancel(&self, token: &str) {
        let mut cancellations = self.cancelled.lock().await;
        let _ = cancellations.cancel(token);
        let mut slot = self.inner.lock().await;
        if slot.as_ref().is_some_and(|s| s.token == token) {
            let s = slot.take().expect("matched session");
            s.cancelled.store(true, Ordering::Relaxed);
            s.task.abort();
        }
    }
    pub async fn take(
        &self,
        token: &str,
        allow_batch_retry: bool,
    ) -> Result<Option<String>, VoiceError> {
        // Match cancellation's lock order so a concurrent cancel cannot remove
        // a session between the tombstone check and taking its receiver.
        let mut cancellations = self.cancelled.lock().await;
        if cancellations.is_cancelled(token) {
            return Err(VoiceError::new(
                VoiceErrorKind::InvalidInput,
                "Voice capture cancelled",
            ));
        }
        let (result, cancelled, started) = {
            let mut slot = self.inner.lock().await;
            let Some(s) = slot.as_mut().filter(|s| s.token == token) else {
                return Ok(None);
            };
            let Some(result) = s.result.take() else {
                return Err(VoiceError::new(
                    VoiceErrorKind::InvalidInput,
                    "Transcription was already consumed",
                ));
            };
            (result, s.cancelled.clone(), s.started)
        };
        drop(cancellations);
        let result = tokio::time::timeout(Duration::from_secs(12), result).await;
        if cancelled.load(Ordering::Relaxed) {
            return Err(VoiceError::new(
                VoiceErrorKind::InvalidInput,
                "Voice capture cancelled",
            ));
        }
        let mut slot = self.inner.lock().await;
        if let Some(s) = slot.as_ref().filter(|s| s.token == token) {
            s.task.abort();
        }
        if slot.as_ref().is_some_and(|s| s.token == token) {
            slot.take();
        }
        drop(slot);
        let result = match result {
            Ok(Ok(result)) => result,
            _ => Completion::failed(VoiceError::new(
                VoiceErrorKind::Network,
                "Live transcription did not complete",
            )),
        };
        let mut report = self.report.lock().await;
        report.completed_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        report.stats_available = result.stats.is_some();
        if let Some(stats) = result.stats {
            report.audio_chunks = stats.audio_chunks;
            report.partial_events = stats.partial_events;
            report.committed_segments = stats.committed_segments;
            report.first_audio_ms = stats.first_audio_ms;
            report.first_partial_ms = stats.first_partial_ms;
        }
        match result.result {
            Ok(text) => {
                report.mode = "streaming".into();
                report.error_kind = None;
                Ok(Some(text))
            }
            Err(error) => {
                report.error_kind = Some(error.kind);
                let retry = allow_batch_retry
                    && !matches!(
                        error.kind,
                        VoiceErrorKind::NoSpeech
                            | VoiceErrorKind::Authentication
                            | VoiceErrorKind::Permission
                            | VoiceErrorKind::Refused
                            | VoiceErrorKind::Quota
                            | VoiceErrorKind::RateLimit
                            | VoiceErrorKind::InvalidInput
                    );
                report.mode = if retry { "batch_fallback" } else { "failed" }.into();
                if retry { Ok(None) } else { Err(error) }
            }
        }
    }
}
/// Recognise one bounded echo-cancelled interruption through the configured
/// streaming backend and the same native speaker-admission path as captures.
pub async fn interruption(
    state: Arc<AppState>,
    token: String,
    samples: &[f32],
) -> Result<String, VoiceError> {
    if samples.len() > 192_000 || samples.iter().any(|s| !s.is_finite()) {
        return Err(VoiceError::new(
            VoiceErrorKind::InvalidInput,
            "Invalid interruption audio",
        ));
    }
    let (send, input) = mpsc::channel(64);
    let mut sequence = 0;
    for chunk in samples.chunks(3_200) {
        let pcm = chunk
            .iter()
            .flat_map(|s| ((s.clamp(-1., 1.) * 32767.) as i16).to_le_bytes())
            .collect();
        send.try_send(Input::Audio { sequence, pcm }).map_err(|_| {
            VoiceError::new(VoiceErrorKind::InvalidInput, "Interruption is too long")
        })?;
        sequence += 1;
    }
    send.try_send(Input::Finish { sequence })
        .map_err(|_| VoiceError::new(VoiceErrorKind::InvalidInput, "Interruption is too long"))?;
    drop(send);
    tokio::time::timeout(Duration::from_secs(15), run(state, token, 16_000, input))
        .await
        .map_err(|_| {
            VoiceError::new(
                VoiceErrorKind::Network,
                "Interruption recognition timed out",
            )
        })?
        .result
}

async fn run(
    state: Arc<AppState>,
    token: String,
    rate: u32,
    mut input: mpsc::Receiver<Input>,
) -> Completion {
    let mut prelude = std::collections::VecDeque::new();
    let mut pcm = zeroize::Zeroizing::new(Vec::new());
    // Identification runs locally before opening the provider connection. Hold
    // only a bounded prefix; the rest remains in the bounded capture channel.
    let identified = if state.speakers.needs_identification() {
        if !notdot_speakers::audio::supported_rate(rate) {
            return Completion::failed(VoiceError::new(
                VoiceErrorKind::Refused,
                "The microphone format cannot be checked for trusted voices",
            ));
        }
        let mut target = rate as usize * 4; // first two seconds
        let max_bytes = rate as usize * 12; // six seconds, still on device
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let mut ended = false;
        loop {
            while !ended && pcm.len() < target {
                let packet = match tokio::time::timeout_at(deadline, input.recv()).await {
                    Ok(Some(packet)) => packet,
                    _ => {
                        return Completion::failed(VoiceError::new(
                            VoiceErrorKind::Refused,
                            "Voice identity could not be checked",
                        ));
                    }
                };
                ended = matches!(packet, Input::Finish { .. });
                if let Input::Audio { pcm: bytes, .. } = &packet {
                    if pcm.len() + bytes.len() > max_bytes {
                        return Completion::failed(VoiceError::new(
                            VoiceErrorKind::Refused,
                            "Voice identity buffer exceeded its limit",
                        ));
                    }
                    pcm.extend_from_slice(bytes);
                }
                prelude.push_back(packet);
            }
            match state.speakers.identify_detailed(&token, rate, &pcm).await {
                Ok(identity) => break Some(identity),
                Err(why)
                    if !ended
                        && target < max_bytes
                        && matches!(
                            why,
                            notdot_core::profiles::Refusal::UnclearAudio
                                | notdot_core::profiles::Refusal::UnrecognisedVoice
                                | notdot_core::profiles::Refusal::UncertainVoice
                        ) =>
                {
                    target = (target + rate as usize * 3).min(max_bytes);
                }
                Err(why) => {
                    return Completion::failed(VoiceError::new(
                        VoiceErrorKind::Refused,
                        why.spoken(),
                    ));
                }
            }
        }
    } else {
        None
    };
    let identity =
        match crate::profile_access::bind_capture(state.clone(), &token, rate, &pcm, identified)
            .await
        {
            Ok(identity) => identity,
            Err(error) => {
                return Completion::failed(VoiceError::new(VoiceErrorKind::Refused, error));
            }
        };

    let backend = match recognition_backend(&state).await {
        Ok(backend) => backend,
        Err(_) => {
            return Completion::failed(VoiceError::new(
                VoiceErrorKind::Authentication,
                "Saved voice key is unavailable",
            ));
        }
    };
    if state.speakers.validate(&identity).is_err() {
        return Completion::failed(VoiceError::new(
            VoiceErrorKind::Refused,
            "Voice access changed before transcription",
        ));
    }
    let stream = match backend.start_transcription(rate).await {
        Ok(stream) => stream,
        Err(error) => return Completion::failed(error),
    };
    let valid = || {
        state.speakers.validate(&identity).is_ok()
            && state.context().id == identity.profile
            && (!token.starts_with("interrupt-") || state.interrupt.capture_owner(&token).is_some())
    };
    run_stream_with_prelude(stream, rate, input, prelude, &valid).await
}
#[cfg(test)]
async fn run_stream(
    stream: Box<dyn notdot_voice::realtime::StreamingTranscriber>,
    rate: u32,
    input: mpsc::Receiver<Input>,
) -> Completion {
    run_stream_with_prelude(
        stream,
        rate,
        input,
        std::collections::VecDeque::new(),
        &|| true,
    )
    .await
}
async fn run_stream_with_prelude(
    mut stream: Box<dyn notdot_voice::realtime::StreamingTranscriber>,
    rate: u32,
    input: mpsc::Receiver<Input>,
    prelude: std::collections::VecDeque<Input>,
    valid: &(dyn Fn() -> bool + Send + Sync),
) -> Completion {
    let result = process_stream(stream.as_mut(), rate, input, prelude, valid).await;
    // Counters contain no transcript or audio. Retain them even when finalisation
    // fails, so diagnostics can distinguish uploaded audio and observed updates.
    let stats = Some(stream.stats());
    stream.close().await;
    Completion { result, stats }
}
async fn process_stream(
    stream: &mut dyn notdot_voice::realtime::StreamingTranscriber,
    rate: u32,
    mut input: mpsc::Receiver<Input>,
    mut prelude: std::collections::VecDeque<Input>,
    valid: &(dyn Fn() -> bool + Send + Sync),
) -> Result<String, VoiceError> {
    let mut expected = 0u64;
    let mut since_commit = 0usize;
    let mut text = String::new();
    loop {
        let next = if let Some(packet) = prelude.pop_front() {
            Ok(Some(packet))
        } else {
            tokio::time::timeout(Duration::from_secs(8), input.recv()).await
        }
        .map_err(|_| VoiceError::new(VoiceErrorKind::Network, "Live microphone audio timed out"))?
        .ok_or_else(|| {
            VoiceError::new(
                VoiceErrorKind::Network,
                "Live microphone audio was interrupted",
            )
        })?;
        if !valid() {
            return Err(VoiceError::new(
                VoiceErrorKind::Refused,
                "Voice access was revoked",
            ));
        }
        match next {
            Input::Audio { sequence, pcm } => {
                if sequence != expected {
                    return Err(VoiceError::new(
                        VoiceErrorKind::Network,
                        "Live microphone audio fell behind",
                    ));
                }
                expected = expected.wrapping_add(1);
                since_commit += pcm.len() / 2;
                stream.send(&pcm).await?;
                if !stream.continuous_turn() && since_commit >= rate as usize * 20 {
                    let segment = stream.commit().await?;
                    append(&mut text, &segment)?;
                    since_commit = 0;
                }
            }
            Input::Finish { sequence } => {
                if sequence != expected {
                    return Err(VoiceError::new(
                        VoiceErrorKind::Network,
                        "Live microphone audio was incomplete",
                    ));
                }
                if since_commit > 0 {
                    append(&mut text, &stream.commit().await?)?;
                }
                if text.trim().is_empty() {
                    return Err(VoiceError::new(
                        VoiceErrorKind::NoSpeech,
                        "No speech was transcribed. Please try again.",
                    ));
                }
                return Ok(text);
            }
        }
    }
}
fn append(text: &mut String, segment: &str) -> Result<(), VoiceError> {
    if segment.trim().is_empty() {
        return Ok(());
    }
    if text.len() + segment.len() + 1 > 65536 {
        return Err(VoiceError::new(
            VoiceErrorKind::InvalidInput,
            "Transcript exceeded its limit",
        ));
    }
    if !text.is_empty() {
        text.push(' ');
    }
    text.push_str(segment);
    Ok(())
}
fn enrol_progress_payload(
    token: &str,
    result: Result<notdot_speakers::enrol::Progress, String>,
) -> serde_json::Value {
    match result {
        Ok(progress) => serde_json::json!({"token":token,"progress":progress}),
        Err(error) => serde_json::json!({"token":token,"error":error}),
    }
}
pub fn spawn(app: AppHandle, state: Arc<AppState>, mut events: mpsc::Receiver<CaptureEvent>) {
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events.recv().await {
            match event {
                CaptureEvent::Start {
                    id,
                    token,
                    rate,
                    manual,
                } => {
                    if state.speakers.is_enrol_capture(&token) {
                        state.streaming.report.lock().await.sample_rate = rate;
                        state.speakers.enrol_audio(&token, rate, &[]);
                        continue;
                    }
                    let mut cancellations = state.streaming.cancelled.lock().await;
                    if cancellations.is_cancelled(&token) {
                        continue;
                    }
                    let _=app.emit("voice-capture",serde_json::json!({"phase":"capturing","id":id,"token":token,"manual":manual}));
                    let (sender, input) = mpsc::channel(64);
                    let (answer, result) = oneshot::channel();
                    let owner = state.clone();
                    let capture_token = token.clone();
                    let task = tokio::spawn(async move {
                        let _ = answer.send(run(owner, capture_token, rate, input).await);
                    });
                    let session = Session {
                        token,
                        input: sender,
                        result: Some(result),
                        task,
                        cancelled: Arc::new(AtomicBool::new(false)),
                        started: Instant::now(),
                    };
                    if let Some(old) = state.streaming.inner.lock().await.replace(session) {
                        old.cancelled.store(true, Ordering::Relaxed);
                        old.task.abort();
                    }
                    drop(cancellations);
                    *state.streaming.report.lock().await = Report {
                        mode: "streaming_pending".into(),
                        sample_rate: rate,
                        ..Default::default()
                    };
                }
                CaptureEvent::Audio {
                    token,
                    sequence,
                    pcm,
                } => {
                    if state.speakers.is_enrol_capture(&token) {
                        let rate = state.streaming.report.lock().await.sample_rate;
                        // Capture start sets this independently of a cloud session.
                        let rate = if rate == 0 { 16_000 } else { rate };
                        if !state.speakers.enrol_audio(&token, rate, &pcm) {
                            state.speakers.enrol_cancel();
                            let _ = state.wake.cancel_capture(&token);
                            let _=app.emit("speaker-enrol-progress",serde_json::json!({"token":token,"error":"The recording exceeded its limit. Please try again."}));
                        }
                        continue;
                    }
                    let slot = state.streaming.inner.lock().await;
                    if let Some(s) = slot.as_ref().filter(|s| s.token == token)
                        && s.input.try_send(Input::Audio { sequence, pcm }).is_err()
                    {
                        s.task.abort();
                    }
                }
                CaptureEvent::End {
                    token,
                    valid,
                    sequence,
                } => {
                    if state.speakers.is_enrol_capture(&token) {
                        let result = state.speakers.enrol_capture_end(&token).await;
                        let payload = enrol_progress_payload(&token, result);
                        let _ = app.emit("speaker-enrol-progress", payload);
                        // Discard the separate bounded WAV held by wake as well.
                        if let Some(id) = state.wake.status().request_id {
                            let _ = state.wake.take(id);
                        }
                        continue;
                    }
                    let _ = app.emit(
                        "voice-capture",
                        serde_json::json!({"phase":if valid{"ready"}else{"empty"},"token":token}),
                    );
                    state.streaming.report.lock().await.capture_end_ms =
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis()
                            .min(u128::from(u64::MAX)) as u64;
                    if valid {
                        let slot = state.streaming.inner.lock().await;
                        if let Some(s) = slot.as_ref().filter(|s| s.token == token)
                            && s.input.try_send(Input::Finish { sequence }).is_err()
                        {
                            s.task.abort();
                        }
                    } else {
                        state.streaming.cancel(&token).await;
                    }
                }
                CaptureEvent::Cancel { token } => {
                    if state.speakers.is_enrol_capture(&token) {
                        state.speakers.enrol_cancel();
                        let _=app.emit("speaker-enrol-progress",serde_json::json!({"token":token,"error":"Voice setup cancelled. Nothing was saved."}));
                        continue;
                    }
                    let _ = app.emit(
                        "voice-capture",
                        serde_json::json!({"phase":"cancelled","token":token}),
                    );
                    state.streaming.cancel(&token).await;
                }
            }
        }
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn segments_are_bounded_and_do_not_replace_stable_text() {
        let mut t = "first".to_string();
        append(&mut t, "second").unwrap();
        assert_eq!(t, "first second");
        assert!(append(&mut t, &"x".repeat(65536)).is_err());
    }
    #[test]
    fn enrol_progress_always_identifies_the_recording_for_stale_event_rejection() {
        let ready = enrol_progress_payload(
            "new-capture",
            Ok(notdot_speakers::enrol::Progress::Ready { of: 4 }),
        );
        assert_eq!(ready["token"], "new-capture");
        assert_eq!(ready["progress"]["step"], "ready");
        let failed = enrol_progress_payload("old-capture", Err("Cancelled".into()));
        assert_eq!(failed["token"], "old-capture");
        assert_eq!(failed["error"], "Cancelled");
    }
    #[tokio::test]
    async fn held_audio_keeps_order_and_revocation_stops_the_next_upload() {
        use std::collections::VecDeque;
        for allowed in [0, 1, 3] {
            let (tx, rx) = mpsc::channel(4);
            tx.send(Input::Audio {
                sequence: 1,
                pcm: vec![0; 32000],
            })
            .await
            .unwrap();
            tx.send(Input::Finish { sequence: 2 }).await.unwrap();
            let prefix = VecDeque::from([Input::Audio {
                sequence: 0,
                pcm: vec![0; 32000],
            }]);
            let checks = std::sync::atomic::AtomicUsize::new(0);
            let valid = || checks.fetch_add(1, Ordering::SeqCst) < allowed;
            let result = run_stream_with_prelude(
                Box::new(MockStream {
                    continuous: false,
                    commits: 0,
                    bytes: 0,
                }),
                16000,
                rx,
                prefix,
                &valid,
            )
            .await;
            if allowed < 3 {
                assert_eq!(result.result.unwrap_err().kind, VoiceErrorKind::Refused);
                assert_eq!(result.stats.unwrap().audio_chunks, allowed as u64 * 32000);
            } else {
                assert_eq!(result.result.unwrap(), "segment1");
                assert_eq!(result.stats.unwrap().audio_chunks, 64000);
            }
        }
    }
    struct MockStream {
        continuous: bool,
        commits: u64,
        bytes: usize,
    }
    #[async_trait::async_trait]
    impl notdot_voice::realtime::StreamingTranscriber for MockStream {
        fn continuous_turn(&self) -> bool {
            self.continuous
        }
        async fn send(&mut self, bytes: &[u8]) -> Result<(), VoiceError> {
            self.bytes += bytes.len();
            Ok(())
        }
        async fn commit(&mut self) -> Result<String, VoiceError> {
            self.commits += 1;
            Ok(format!("segment{}", self.commits))
        }
        async fn close(&mut self) {}
        fn stats(&self) -> StreamStats {
            StreamStats {
                audio_chunks: self.bytes as u64,
                committed_segments: self.commits,
                ..Default::default()
            }
        }
    }
    #[tokio::test]
    async fn missing_middle_or_final_audio_is_rejected_instead_of_submitted() {
        for (sent, end) in [(vec![0, 2], 3), (vec![0, 1], 3)] {
            let (tx, rx) = mpsc::channel(8);
            for sequence in sent {
                tx.send(Input::Audio {
                    sequence,
                    pcm: vec![0; 32],
                })
                .await
                .unwrap();
            }
            tx.send(Input::Finish { sequence: end }).await.unwrap();
            let err = run_stream(
                Box::new(MockStream {
                    continuous: false,
                    commits: 0,
                    bytes: 0,
                }),
                16000,
                rx,
            )
            .await
            .result
            .err()
            .unwrap();
            assert_eq!(err.kind, VoiceErrorKind::Network);
        }
    }
    #[tokio::test]
    async fn long_turn_preserves_committed_segment_order_without_forcing_continuous_recognizers() {
        for continuous in [false, true] {
            let (tx, rx) = mpsc::channel(32);
            for sequence in 0..21 {
                tx.send(Input::Audio {
                    sequence,
                    pcm: vec![0; 32000],
                })
                .await
                .unwrap();
            }
            tx.send(Input::Finish { sequence: 21 }).await.unwrap();
            let completion = run_stream(
                Box::new(MockStream {
                    continuous,
                    commits: 0,
                    bytes: 0,
                }),
                16000,
                rx,
            )
            .await;
            let text = completion.result.unwrap();
            let stats = completion.stats.unwrap();
            assert_eq!(
                text,
                if continuous {
                    "segment1"
                } else {
                    "segment1 segment2"
                }
            );
            assert_eq!(stats.committed_segments, if continuous { 1 } else { 2 });
        }
    }
    struct TextStream(std::collections::VecDeque<String>);
    #[async_trait::async_trait]
    impl notdot_voice::realtime::StreamingTranscriber for TextStream {
        async fn send(&mut self, _: &[u8]) -> Result<(), VoiceError> {
            Ok(())
        }
        async fn commit(&mut self) -> Result<String, VoiceError> {
            Ok(self.0.pop_front().expect("expected commit"))
        }
        async fn close(&mut self) {}
        fn stats(&self) -> StreamStats {
            StreamStats::default()
        }
    }
    #[tokio::test]
    async fn empty_final_segment_keeps_earlier_speech_but_empty_utterance_is_no_speech() {
        for first in ["five plus seven", ""] {
            let (tx, rx) = mpsc::channel(32);
            for sequence in 0..21 {
                tx.send(Input::Audio {
                    sequence,
                    pcm: vec![0; 32000],
                })
                .await
                .unwrap();
            }
            tx.send(Input::Finish { sequence: 21 }).await.unwrap();
            let result = run_stream(
                Box::new(TextStream([first.into(), "".into()].into())),
                16000,
                rx,
            )
            .await
            .result;
            if first.is_empty() {
                assert_eq!(result.unwrap_err().kind, VoiceErrorKind::NoSpeech);
            } else {
                assert_eq!(result.unwrap(), first);
            }
        }
    }
    #[tokio::test]
    async fn streaming_only_errors_survive_and_retry_never_bypasses_restrictions() {
        for allow_retry in [false, true] {
            for kind in [
                VoiceErrorKind::Network,
                VoiceErrorKind::Provider,
                VoiceErrorKind::Authentication,
                VoiceErrorKind::Permission,
                VoiceErrorKind::Refused,
                VoiceErrorKind::Quota,
                VoiceErrorKind::RateLimit,
                VoiceErrorKind::InvalidInput,
            ] {
                let manager = Streaming::default();
                let (input, _receiver) = mpsc::channel(8);
                let (answer, result) = oneshot::channel();
                assert!(
                    answer
                        .send(Completion::failed(VoiceError::new(
                            kind,
                            "Original recognition failure"
                        )))
                        .is_ok()
                );
                *manager.inner.lock().await = Some(Session {
                    token: "failure".into(),
                    input,
                    result: Some(result),
                    task: tokio::spawn(async {}),
                    cancelled: Arc::new(AtomicBool::new(false)),
                    started: Instant::now(),
                });
                let result = manager.take("failure", allow_retry).await;
                let retry = allow_retry
                    && matches!(kind, VoiceErrorKind::Network | VoiceErrorKind::Provider);
                if retry {
                    assert!(result.unwrap().is_none());
                    assert_eq!(manager.report.lock().await.mode, "batch_fallback");
                } else {
                    let error = result.unwrap_err();
                    assert_eq!(error.kind, kind);
                    assert_eq!(error.to_string(), "Original recognition failure");
                    assert_eq!(manager.report.lock().await.mode, "failed");
                }
                assert!(
                    !manager.report.lock().await.stats_available,
                    "setup failure has no decoder counters"
                );
                assert!(manager.inner.lock().await.is_none());
            }
        }
    }
    struct FinalFailure {
        chunks: u64,
        closed: Arc<AtomicBool>,
    }
    #[async_trait::async_trait]
    impl notdot_voice::realtime::StreamingTranscriber for FinalFailure {
        async fn send(&mut self, _: &[u8]) -> Result<(), VoiceError> {
            self.chunks += 1;
            Ok(())
        }
        async fn commit(&mut self) -> Result<String, VoiceError> {
            Err(VoiceError::new(
                VoiceErrorKind::Network,
                "Final transcript timed out",
            ))
        }
        async fn close(&mut self) {
            self.closed.store(true, Ordering::Relaxed);
        }
        fn stats(&self) -> StreamStats {
            StreamStats {
                audio_chunks: self.chunks,
                partial_events: 3,
                first_audio_ms: 1000,
                first_partial_ms: 1010,
                ..Default::default()
            }
        }
    }
    #[tokio::test]
    async fn finalisation_failure_preserves_observed_counters_and_closes_stream() {
        for rate in [16000, 44100, 48000] {
            let manager = Streaming::default();
            let closed = Arc::new(AtomicBool::new(false));
            let (tx, input) = mpsc::channel(8);
            tx.send(Input::Audio {
                sequence: 0,
                pcm: vec![0; 32],
            })
            .await
            .unwrap();
            tx.send(Input::Finish { sequence: 1 }).await.unwrap();
            let (answer, result) = oneshot::channel();
            let stream = FinalFailure {
                chunks: 0,
                closed: closed.clone(),
            };
            let task = tokio::spawn(async move {
                let _ = answer.send(run_stream(Box::new(stream), rate, input).await);
            });
            *manager.inner.lock().await = Some(Session {
                token: "failure".into(),
                input: tx,
                result: Some(result),
                task,
                cancelled: Arc::new(AtomicBool::new(false)),
                started: Instant::now(),
            });
            *manager.report.lock().await = Report {
                sample_rate: rate,
                ..Default::default()
            };
            let error = manager.take("failure", false).await.unwrap_err();
            assert_eq!(error.to_string(), "Final transcript timed out");
            let report = manager.report.lock().await;
            assert!(report.stats_available);
            assert_eq!(report.sample_rate, rate);
            assert_eq!(report.mode, "failed");
            assert_eq!(report.audio_chunks, 1);
            assert_eq!(report.partial_events, 3);
            assert_eq!(report.committed_segments, 0);
            assert_eq!(report.first_audio_ms, 1000);
            assert_eq!(report.first_partial_ms, 1010);
            assert!(closed.load(Ordering::Relaxed));
            assert!(manager.inner.lock().await.is_none());
        }
    }
    #[tokio::test]
    async fn explicit_no_speech_never_falls_back_to_a_billable_batch_request() {
        let manager = Streaming::default();
        let (input, _) = mpsc::channel(1);
        let (answer, result) = oneshot::channel();
        answer
            .send(Completion::failed(VoiceError::new(
                VoiceErrorKind::NoSpeech,
                "No speech heard",
            )))
            .ok()
            .unwrap();
        *manager.inner.lock().await = Some(Session {
            token: "empty-followup".into(),
            input,
            result: Some(result),
            task: tokio::spawn(async {}),
            cancelled: Arc::new(AtomicBool::new(false)),
            started: Instant::now(),
        });
        assert_eq!(
            manager.take("empty-followup", true).await.unwrap_err().kind,
            VoiceErrorKind::NoSpeech
        );
        assert!(manager.inner.lock().await.is_none());
        assert_eq!(manager.report.lock().await.mode, "failed");
    }
    #[tokio::test]
    async fn cancelled_session_never_requests_batch_fallback() {
        let manager = Streaming::default();
        manager.cancel("capture-1").await;
        assert!(manager.take("capture-1", true).await.is_err());
        // A cancellation for another turn cannot block this one.
        assert!(manager.take("capture-2", true).await.unwrap().is_none());
        let (tx, input) = mpsc::channel(8);
        let (answer, result) = oneshot::channel();
        let task = tokio::spawn(async move {
            let value = run_stream(
                Box::new(MockStream {
                    continuous: false,
                    commits: 0,
                    bytes: 0,
                }),
                16000,
                input,
            )
            .await;
            let _ = answer.send(value);
        });
        *manager.inner.lock().await = Some(Session {
            token: "capture-3".into(),
            input: tx,
            result: Some(result),
            task,
            cancelled: Arc::new(AtomicBool::new(false)),
            started: Instant::now(),
        });
        manager.cancel("capture-3").await;
        assert!(manager.take("capture-3", true).await.is_err());
        assert!(manager.inner.lock().await.is_none());
    }
}
