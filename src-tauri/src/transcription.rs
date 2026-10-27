//! One bounded streaming session for the current activated recording. Failures
//! preserve the local WAV for a batch retry; cancellation never triggers a retry.
use crate::{AppState, backend, wake::CaptureEvent};
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
struct Session {
    token: String,
    input: mpsc::Sender<Input>,
    result: Option<oneshot::Receiver<Result<(String, StreamStats), VoiceError>>>,
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
    pub async fn take(&self, token: &str) -> Result<Option<String>, VoiceError> {
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
            _ => Err(VoiceError::new(
                VoiceErrorKind::Network,
                "Live transcription did not complete",
            )),
        };
        let mut report = self.report.lock().await;
        report.completed_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        match result {
            Ok((text, stats)) => {
                report.mode = "streaming".into();
                report.audio_chunks = stats.audio_chunks;
                report.partial_events = stats.partial_events;
                report.committed_segments = stats.committed_segments;
                report.first_audio_ms = stats.first_audio_ms;
                report.first_partial_ms = stats.first_partial_ms;
                report.error_kind = None;
                Ok(Some(text))
            }
            Err(error) => {
                report.mode = "batch_fallback".into();
                report.error_kind = Some(error.kind);
                // No retry can bypass exhausted credits or invalid authentication.
                if matches!(
                    error.kind,
                    VoiceErrorKind::Authentication | VoiceErrorKind::Quota
                ) {
                    Err(error)
                } else {
                    Ok(None)
                }
            }
        }
    }
}
async fn run(
    state: Arc<AppState>,
    rate: u32,
    input: mpsc::Receiver<Input>,
) -> Result<(String, StreamStats), VoiceError> {
    let backend = backend(&state, None).await.map_err(|_| {
        VoiceError::new(
            VoiceErrorKind::Authentication,
            "Saved voice key is unavailable",
        )
    })?;
    let stream = backend.start_transcription(rate).await?;
    run_stream(stream, rate, input).await
}
async fn run_stream(
    mut stream: Box<dyn notdot_voice::realtime::StreamingTranscriber>,
    rate: u32,
    mut input: mpsc::Receiver<Input>,
) -> Result<(String, StreamStats), VoiceError> {
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
                if since_commit >= rate as usize * 20 {
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
                let stats = stream.stats();
                stream.close().await;
                return Ok((text, stats));
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
        commits: u64,
        bytes: usize,
    }
    #[async_trait::async_trait]
    impl notdot_voice::realtime::StreamingTranscriber for MockStream {
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
                    commits: 0,
                    bytes: 0,
                }),
                16000,
                rx,
            )
            .await
            .err()
            .unwrap();
            assert_eq!(err.kind, VoiceErrorKind::Network);
        }
    }
    #[tokio::test]
    async fn long_turn_preserves_committed_segment_order() {
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
        let (text, stats) = run_stream(
            Box::new(MockStream {
                commits: 0,
                bytes: 0,
            }),
            16000,
            rx,
        )
        .await
        .unwrap();
        assert_eq!(text, "segment1 segment2");
        assert_eq!(stats.committed_segments, 2);
    }
    #[tokio::test]
    async fn cancelled_session_never_requests_batch_fallback() {
        let manager = Streaming::default();
        manager.cancel("capture-1").await;
        assert!(manager.take("capture-1").await.is_err());
        // A cancellation for another turn cannot block this one.
        assert!(manager.take("capture-2").await.unwrap().is_none());
        let (tx, input) = mpsc::channel(8);
        let (answer, result) = oneshot::channel();
        let task = tokio::spawn(async move {
            let value = run_stream(
                Box::new(MockStream {
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
        assert!(manager.take("capture-3").await.is_err());
        assert!(manager.inner.lock().await.is_none());
    }
}
