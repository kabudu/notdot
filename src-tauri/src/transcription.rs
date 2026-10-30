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
                        VoiceErrorKind::Authentication
                            | VoiceErrorKind::Permission
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
async fn run(state: Arc<AppState>, rate: u32, input: mpsc::Receiver<Input>) -> Completion {
    let backend = match recognition_backend(&state).await {
        Ok(backend) => backend,
        Err(_) => {
            return Completion::failed(VoiceError::new(
                VoiceErrorKind::Authentication,
                "Saved voice key is unavailable",
            ));
        }
    };
    let stream = match backend.start_transcription(rate).await {
        Ok(stream) => stream,
        Err(error) => return Completion::failed(error),
    };
    run_stream(stream, rate, input).await
}
async fn run_stream(
    mut stream: Box<dyn notdot_voice::realtime::StreamingTranscriber>,
    rate: u32,
    input: mpsc::Receiver<Input>,
) -> Completion {
    let result = process_stream(stream.as_mut(), rate, input).await;
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
) -> Result<String, VoiceError> {
    let mut expected = 0u64;
    let mut since_commit = 0usize;
    let mut text = String::new();
    loop {
        let next = tokio::time::timeout(Duration::from_secs(8), input.recv())
            .await
            .map_err(|_| {
                VoiceError::new(VoiceErrorKind::Network, "Live microphone audio timed out")
            })?
            .ok_or_else(|| {
                VoiceError::new(
                    VoiceErrorKind::Network,
                    "Live microphone audio was interrupted",
                )
            })?;
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
                        VoiceErrorKind::Provider,
                        "No usable speech was transcribed",
                    ));
                }
                return Ok(text);
            }
        }
    }
}
fn append(text: &mut String, segment: &str) -> Result<(), VoiceError> {
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
                    let mut cancellations = state.streaming.cancelled.lock().await;
                    if cancellations.is_cancelled(&token) {
                        continue;
                    }
                    let _=app.emit("voice-capture",serde_json::json!({"phase":"capturing","id":id,"token":token,"manual":manual}));
                    let (sender, input) = mpsc::channel(64);
                    let (answer, result) = oneshot::channel();
                    let owner = state.clone();
                    let task = tokio::spawn(async move {
                        let _ = answer.send(run(owner, rate, input).await);
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
    #[tokio::test]
    async fn streaming_only_errors_survive_and_retry_never_bypasses_restrictions() {
        for allow_retry in [false, true] {
            for kind in [
                VoiceErrorKind::Network,
                VoiceErrorKind::Provider,
                VoiceErrorKind::Authentication,
                VoiceErrorKind::Permission,
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
