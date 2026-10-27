//! Streaming STT boundary. Only committed segments are used for model input;
//! partials are counted for diagnostics, never exposed as conversational text.
use crate::{VoiceError, VoiceErrorKind};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::{SinkExt, StreamExt, stream::SplitSink};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{net::TcpStream, sync::mpsc, task::JoinHandle, time::timeout};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{Message, client::IntoClientRequest, protocol::WebSocketConfig},
};
#[derive(Clone, Copy, Default, Serialize, Deserialize)]
pub struct StreamStats {
    pub audio_chunks: u64,
    pub partial_events: u64,
    pub committed_segments: u64,
    pub first_audio_ms: u64,
    pub first_partial_ms: u64,
}
#[async_trait]
pub trait StreamingTranscriber: Send {
    async fn send(&mut self, pcm16_le: &[u8]) -> Result<(), VoiceError>;
    async fn commit(&mut self) -> Result<String, VoiceError>;
    async fn close(&mut self);
    fn stats(&self) -> StreamStats;
}
fn timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}
fn error(kind: VoiceErrorKind, message: &str) -> VoiceError {
    VoiceError::new(kind, message)
}
fn protocol_error(kind: &str) -> Option<VoiceError> {
    let (category, message) = match kind {
        "auth_error" => (
            VoiceErrorKind::Authentication,
            "Voice service rejected the streaming key",
        ),
        "quota_exceeded" => (VoiceErrorKind::Quota, "Voice service credits are exhausted"),
        "rate_limited" | "queue_overflow" | "resource_exhausted" => (
            VoiceErrorKind::RateLimit,
            "Live transcription is temporarily busy",
        ),
        "unaccepted_terms" => (
            VoiceErrorKind::Unsupported,
            "Live transcription needs setup in the provider account",
        ),
        "input_error" | "invalid_request" | "chunk_size_exceeded" => (
            VoiceErrorKind::InvalidInput,
            "Live transcription rejected the audio format",
        ),
        "error"
        | "transcriber_error"
        | "commit_throttled"
        | "session_time_limit_exceeded"
        | "insufficient_audio_activity" => (
            VoiceErrorKind::Provider,
            "Live transcription could not finish",
        ),
        _ => return None,
    };
    Some(error(category, message))
}
pub(crate) struct ElevenStream {
    writer: SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
    results: mpsc::Receiver<Result<String, VoiceError>>,
    reader: JoinHandle<()>,
    partials: Arc<AtomicU64>,
    first_partial: Arc<AtomicU64>,
    rate: u32,
    samples_since_commit: usize,
    stats: StreamStats,
}
impl ElevenStream {
    pub(crate) async fn connect(
        endpoint: &str,
        key: reqwest::header::HeaderValue,
        rate: u32,
    ) -> Result<Self, VoiceError> {
        if ![8000, 16000, 22050, 24000, 44100, 48000].contains(&rate) {
            return Err(error(
                VoiceErrorKind::Unsupported,
                "This microphone rate uses batch transcription",
            ));
        }
        let base = endpoint
            .replacen("https://", "wss://", 1)
            .replacen("http://", "ws://", 1);
        let url = format!(
            "{base}/v1/speech-to-text/realtime?model_id=scribe_v2_realtime&audio_format=pcm_{rate}&commit_strategy=manual"
        );
        let mut request = url.into_client_request().map_err(|_| {
            error(
                VoiceErrorKind::InvalidInput,
                "Invalid voice service endpoint",
            )
        })?;
        request.headers_mut().insert("xi-api-key", key);
        let config = WebSocketConfig::default()
            .max_message_size(Some(131072))
            .max_frame_size(Some(131072));
        let (mut socket, _) = timeout(
            Duration::from_secs(5),
            connect_async_with_config(request, Some(config), false),
        )
        .await
        .map_err(|_| {
            error(
                VoiceErrorKind::Network,
                "Live transcription connection timed out",
            )
        })?
        .map_err(|failure| {
            let kind = match failure {
                tokio_tungstenite::tungstenite::Error::Http(response) => {
                    match response.status().as_u16() {
                        401 => VoiceErrorKind::Authentication,
                        402 => VoiceErrorKind::Quota,
                        403 => VoiceErrorKind::Permission,
                        429 => VoiceErrorKind::RateLimit,
                        _ => VoiceErrorKind::Network,
                    }
                }
                _ => VoiceErrorKind::Network,
            };
            error(kind, "Live transcription connection failed")
        })?;
        timeout(Duration::from_secs(5), async {
            for _ in 0..16 {
                let msg = socket
                    .next()
                    .await
                    .ok_or_else(|| {
                        error(
                            VoiceErrorKind::Network,
                            "Live transcription closed before starting",
                        )
                    })?
                    .map_err(|_| {
                        error(VoiceErrorKind::Network, "Live transcription did not start")
                    })?;
                if let Message::Text(text) = msg {
                    let value: Value = serde_json::from_str(&text).map_err(|_| {
                        error(VoiceErrorKind::Provider, "Invalid live transcription event")
                    })?;
                    let kind = value["message_type"].as_str().unwrap_or("");
                    if let Some(e) = protocol_error(kind) {
                        return Err(e);
                    }
                    if kind == "session_started" {
                        return Ok(());
                    }
                }
            }
            Err(error(
                VoiceErrorKind::Provider,
                "Live transcription handshake exceeded its limit",
            ))
        })
        .await
        .map_err(|_| {
            error(
                VoiceErrorKind::Network,
                "Live transcription handshake timed out",
            )
        })??;
        let (writer, mut input) = socket.split();
        let (sender, results) = mpsc::channel(8);
        let partials = Arc::new(AtomicU64::new(0));
        let observed = partials.clone();
        let first_partial = Arc::new(AtomicU64::new(0));
        let partial_at = first_partial.clone();
        let reader = tokio::spawn(async move {
            for _ in 0..10000 {
                let Some(message) = input.next().await else {
                    break;
                };
                let result = match message {
                    Ok(Message::Text(text)) => {
                        let Ok(value) = serde_json::from_str::<Value>(&text) else {
                            let _ = sender
                                .send(Err(error(
                                    VoiceErrorKind::Provider,
                                    "Invalid live transcription event",
                                )))
                                .await;
                            break;
                        };
                        match value["message_type"].as_str().unwrap_or("") {
                            "partial_transcript" => {
                                if value["text"].as_str().is_some_and(|s| !s.is_empty()) {
                                    observed.fetch_add(1, Ordering::Relaxed);
                                    let _ = partial_at.compare_exchange(
                                        0,
                                        timestamp_ms(),
                                        Ordering::Relaxed,
                                        Ordering::Relaxed,
                                    );
                                }
                                continue;
                            }
                            "committed_transcript" => {
                                match value["text"]
                                    .as_str()
                                    .filter(|s| !s.trim().is_empty() && s.len() <= 65536)
                                {
                                    Some(text) => Ok(text.trim().to_owned()),
                                    None => Err(error(
                                        VoiceErrorKind::Provider,
                                        "Live transcription returned no usable speech",
                                    )),
                                }
                            }
                            kind => {
                                if let Some(e) = protocol_error(kind) {
                                    Err(e)
                                } else {
                                    continue;
                                }
                            }
                        }
                    }
                    Ok(Message::Close(_)) | Err(_) => break,
                    _ => continue,
                };
                let failed = result.is_err();
                if sender.try_send(result).is_err() || failed {
                    break;
                }
            }
        });
        Ok(Self {
            writer,
            results,
            reader,
            partials,
            first_partial,
            rate,
            samples_since_commit: 0,
            stats: StreamStats::default(),
        })
    }
    async fn chunk(&mut self, bytes: &[u8], commit: bool) -> Result<(), VoiceError> {
        if !bytes.len().is_multiple_of(2) || bytes.len() > self.rate as usize * 2 {
            return Err(error(
                VoiceErrorKind::InvalidInput,
                "Live audio chunks must be mono PCM16 and at most one second",
            ));
        }
        let value = json!({"message_type":"input_audio_chunk","audio_base_64":STANDARD.encode(bytes),"sample_rate":self.rate,"commit":commit});
        timeout(
            Duration::from_secs(2),
            self.writer.send(Message::Text(value.to_string().into())),
        )
        .await
        .map_err(|_| error(VoiceErrorKind::Network, "Live audio upload timed out"))?
        .map_err(|_| error(VoiceErrorKind::Network, "Live audio upload failed"))?;
        self.samples_since_commit += bytes.len() / 2;
        if !bytes.is_empty() {
            self.stats.audio_chunks += 1;
            if self.stats.first_audio_ms == 0 {
                self.stats.first_audio_ms = timestamp_ms();
            }
        }
        Ok(())
    }
}
#[async_trait]
impl StreamingTranscriber for ElevenStream {
    async fn send(&mut self, pcm16_le: &[u8]) -> Result<(), VoiceError> {
        self.chunk(pcm16_le, false).await
    }
    async fn commit(&mut self) -> Result<String, VoiceError> {
        // The service begins decoding after two seconds. Pad a short utterance
        // with generated silence; do not keep recording the user's environment.
        while self.samples_since_commit < (self.rate as usize * 2) {
            let remaining =
                (self.rate as usize * 2 - self.samples_since_commit).min(self.rate as usize / 2);
            self.chunk(&vec![0; remaining * 2], false).await?;
        }
        self.chunk(&[], true).await?;
        let text = timeout(Duration::from_secs(5), self.results.recv())
            .await
            .map_err(|_| {
                error(
                    VoiceErrorKind::Network,
                    "Live transcription commit timed out",
                )
            })?
            .ok_or_else(|| error(VoiceErrorKind::Network, "Live transcription disconnected"))??;
        self.samples_since_commit = 0;
        self.stats.committed_segments += 1;
        Ok(text)
    }
    async fn close(&mut self) {
        let _ = timeout(Duration::from_secs(1), self.writer.close()).await;
        self.reader.abort();
    }
    fn stats(&self) -> StreamStats {
        StreamStats {
            partial_events: self.partials.load(Ordering::Relaxed),
            first_partial_ms: self.first_partial.load(Ordering::Relaxed),
            ..self.stats
        }
    }
}
impl Drop for ElevenStream {
    fn drop(&mut self) {
        self.reader.abort();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn partial_rewrites_are_not_submitted_and_short_audio_is_committed_once() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            ws.send(Message::Text(
                json!({"message_type":"session_started"}).to_string().into(),
            ))
            .await
            .unwrap();
            let mut bytes = 0;
            while let Some(Ok(Message::Text(text))) = ws.next().await {
                let v: Value = serde_json::from_str(&text).unwrap();
                bytes += STANDARD
                    .decode(v["audio_base_64"].as_str().unwrap())
                    .unwrap()
                    .len();
                if v["commit"] == true {
                    assert_eq!(bytes, 64000);
                    for (kind, text) in [
                        ("partial_transcript", "seven times five"),
                        ("partial_transcript", "seven plus five"),
                        ("committed_transcript", "seven plus five"),
                    ] {
                        ws.send(Message::Text(
                            json!({"message_type":kind,"text":text}).to_string().into(),
                        ))
                        .await
                        .unwrap();
                    }
                    break;
                }
            }
        });
        let mut stream = ElevenStream::connect(
            &format!("http://{addr}"),
            crate::sensitive_key("test-key").unwrap(),
            16000,
        )
        .await
        .unwrap();
        stream.send(&vec![0; 3200]).await.unwrap();
        assert_eq!(stream.commit().await.unwrap(), "seven plus five");
        assert_eq!(stream.stats().partial_events, 2);
        assert_eq!(stream.stats().committed_segments, 1);
        stream.close().await;
        server.await.unwrap();
    }
    #[test]
    fn provider_error_bodies_are_never_copied_into_messages() {
        assert_eq!(
            protocol_error("quota_exceeded").unwrap().kind,
            VoiceErrorKind::Quota
        );
        assert_eq!(
            protocol_error("auth_error").unwrap().kind,
            VoiceErrorKind::Authentication
        );
        assert!(protocol_error("partial_transcript").is_none());
    }
}
