//! Experimental Flux recognition adapter. Speech playback remains a separate backend.
use crate::{
    AudioClip, ProviderInfo, VoiceBackend, VoiceError, VoiceErrorKind, VoicePage,
    realtime::{StreamStats, StreamingTranscriber},
};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt, stream::SplitSink};
use serde_json::json;
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{net::TcpStream, sync::mpsc, task::JoinHandle, time::timeout};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{Message, client::IntoClientRequest, protocol::WebSocketConfig},
};
fn err(kind: VoiceErrorKind, s: &str) -> VoiceError {
    VoiceError::new(kind, s)
}
fn unsupported() -> VoiceError {
    err(
        VoiceErrorKind::Unsupported,
        "Deepgram Flux provides streaming recognition only",
    )
}
pub(crate) struct Deepgram {
    pub key: reqwest::header::HeaderValue,
}
#[async_trait]
impl VoiceBackend for Deepgram {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "deepgram",
            name: "Deepgram Flux (experimental)",
            streaming_transcription: true,
        }
    }
    async fn voices(&self, _: &str, _: Option<&str>) -> Result<VoicePage, VoiceError> {
        Err(unsupported())
    }
    async fn synthesize(&self, _: &str, _: &str) -> Result<AudioClip, VoiceError> {
        Err(unsupported())
    }
    async fn preview(&self, _: &str) -> Result<AudioClip, VoiceError> {
        Err(unsupported())
    }
    async fn transcribe(&self, _: AudioClip) -> Result<String, VoiceError> {
        Err(unsupported())
    }
    async fn start_transcription(
        &self,
        rate: u32,
    ) -> Result<Box<dyn StreamingTranscriber>, VoiceError> {
        Ok(Box::new(
            Flux::connect("wss://api.deepgram.com", self.key.clone(), rate).await?,
        ))
    }
}
struct Flux {
    writer: SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
    results: mpsc::Receiver<Result<String, VoiceError>>,
    reader: JoinHandle<()>,
    partials: Arc<AtomicU64>,
    first_partial: Arc<AtomicU64>,
    stats: StreamStats,
    rate: u32,
}
impl Flux {
    async fn connect(
        base: &str,
        key: reqwest::header::HeaderValue,
        rate: u32,
    ) -> Result<Self, VoiceError> {
        if ![8000, 16000, 24000, 44100, 48000].contains(&rate) {
            return Err(err(
                VoiceErrorKind::Unsupported,
                "Deepgram needs an 8, 16, 24, 44.1 or 48 kHz microphone",
            ));
        }
        // Manual endpointing preserves the existing native VAD and cancellation contract.
        // Provider-driven turn detection will require separate live acceptance.
        let mut req=format!("{base}/v2/listen?model=flux-general-en&encoding=linear16&sample_rate={rate}&eot_threshold=1.0&eot_timeout_ms=60000").into_client_request().map_err(|_|err(VoiceErrorKind::InvalidInput,"Invalid recognition endpoint"))?;
        req.headers_mut().insert("authorization", key);
        let config = WebSocketConfig::default()
            .max_message_size(Some(131072))
            .max_frame_size(Some(131072));
        let (socket, _) = timeout(
            Duration::from_secs(5),
            tokio_tungstenite::connect_async_with_config(req, Some(config), false),
        )
        .await
        .map_err(|_| err(VoiceErrorKind::Network, "Deepgram connection timed out"))?
        .map_err(|e| {
            let kind = match e {
                tokio_tungstenite::tungstenite::Error::Http(r) => match r.status().as_u16() {
                    401 => VoiceErrorKind::Authentication,
                    403 => VoiceErrorKind::Permission,
                    402 => VoiceErrorKind::Quota,
                    429 => VoiceErrorKind::RateLimit,
                    _ => VoiceErrorKind::Network,
                },
                _ => VoiceErrorKind::Network,
            };
            err(kind, "Deepgram connection failed")
        })?;
        let (writer, mut input) = socket.split();
        let (tx, results) = mpsc::channel(8);
        let partials = Arc::new(AtomicU64::new(0));
        let seen = partials.clone();
        let first_partial = Arc::new(AtomicU64::new(0));
        let first_seen = first_partial.clone();
        let reader = tokio::spawn(async move {
            let mut previous = None;
            for _ in 0..10000 {
                let Some(Ok(message)) = input.next().await else {
                    break;
                };
                let Message::Text(text) = message else {
                    continue;
                };
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                    let _ =
                        tx.try_send(Err(err(VoiceErrorKind::Provider, "Invalid Deepgram event")));
                    break;
                };
                if value["type"] == "Error" {
                    let _ = tx.try_send(Err(err(
                        VoiceErrorKind::Provider,
                        "Deepgram could not transcribe this request",
                    )));
                    break;
                }
                if value["type"] != "TurnInfo" {
                    continue;
                }
                if value["event"] == "Update" {
                    if value["transcript"]
                        .as_str()
                        .is_none_or(|s| s.trim().is_empty())
                    {
                        continue;
                    }
                    seen.fetch_add(1, Ordering::Relaxed);
                    let _ = first_seen.compare_exchange(
                        0,
                        crate::realtime::timestamp_ms(),
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    );
                    continue;
                }
                if value["event"] != "EndOfTurn" {
                    continue;
                }
                let Some(index) = value["turn_index"].as_u64() else {
                    let _ = tx.try_send(Err(err(
                        VoiceErrorKind::Provider,
                        "Deepgram omitted the turn identity",
                    )));
                    break;
                };
                if previous.is_some_and(|p| index <= p) {
                    continue;
                }
                previous = Some(index);
                let answer = value["transcript"]
                    .as_str()
                    .filter(|s| !s.trim().is_empty() && s.len() <= 65536)
                    .map(|s| s.trim().to_owned())
                    .ok_or_else(|| {
                        err(
                            VoiceErrorKind::Provider,
                            "Deepgram returned no usable speech",
                        )
                    });
                if tx.try_send(answer).is_err() {
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
            stats: StreamStats::default(),
            rate,
        })
    }
    async fn commit_with_timeout(&mut self, final_timeout: Duration) -> Result<String, VoiceError> {
        timeout(
            Duration::from_secs(2),
            self.writer.send(Message::Text(
                json!({"type":"ForceEndTurn"}).to_string().into(),
            )),
        )
        .await
        .map_err(|_| err(VoiceErrorKind::Network, "Deepgram commit timed out"))?
        .map_err(|_| err(VoiceErrorKind::Network, "Deepgram commit failed"))?;
        let answer = timeout(final_timeout, self.results.recv())
            .await
            .map_err(|_| {
                // Report only whether an update was observed, never its content.
                let message = if self.partials.load(Ordering::Relaxed) > 0 {
                    "Deepgram final transcript timed out after receiving speech updates"
                } else {
                    "Deepgram final transcript timed out without receiving speech updates"
                };
                err(VoiceErrorKind::Network, message)
            })?
            .ok_or_else(|| err(VoiceErrorKind::Network, "Deepgram disconnected"))??;
        self.stats.committed_segments += 1;
        Ok(answer)
    }
}
#[async_trait]
impl StreamingTranscriber for Flux {
    fn continuous_turn(&self) -> bool {
        true
    }
    async fn send(&mut self, bytes: &[u8]) -> Result<(), VoiceError> {
        if !bytes.len().is_multiple_of(2) || bytes.len() > self.rate as usize * 2 {
            return Err(err(
                VoiceErrorKind::InvalidInput,
                "Invalid mono PCM16 chunk",
            ));
        }
        // Split existing native packets into Flux's recommended 80 ms frames.
        for chunk in bytes.chunks(self.rate as usize * 2 * 80 / 1000) {
            timeout(
                Duration::from_secs(2),
                self.writer.send(Message::Binary(chunk.to_vec().into())),
            )
            .await
            .map_err(|_| err(VoiceErrorKind::Network, "Deepgram upload timed out"))?
            .map_err(|_| err(VoiceErrorKind::Network, "Deepgram upload failed"))?;
            self.stats.audio_chunks += 1;
            if self.stats.first_audio_ms == 0 {
                self.stats.first_audio_ms = crate::realtime::timestamp_ms();
            }
        }
        Ok(())
    }
    async fn commit(&mut self) -> Result<String, VoiceError> {
        self.commit_with_timeout(Duration::from_secs(5)).await
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
impl Drop for Flux {
    fn drop(&mut self) {
        self.reader.abort();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn binary_upload_final_turn_only_and_sensitive_header() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            #[allow(clippy::result_large_err)]
            let callback = |r: &tokio_tungstenite::tungstenite::handshake::server::Request,
                            response| {
                assert!(r.uri().path().ends_with("/v2/listen"));
                assert!(!r.uri().to_string().contains("synthetic"));
                assert_eq!(r.headers()["authorization"], "Token synthetic");
                Ok(response)
            };
            let mut ws = tokio_tungstenite::accept_hdr_async(tcp, callback)
                .await
                .unwrap();
            let mut count = 0;
            let mut commits = 0;
            while let Some(Ok(m)) = ws.next().await {
                match m {
                    Message::Binary(b) => {
                        assert!(b.len() <= 2560);
                        count += b.len();
                    }
                    Message::Text(t) => {
                        assert_eq!(
                            serde_json::from_str::<serde_json::Value>(&t).unwrap()["type"],
                            "ForceEndTurn"
                        );
                        commits += 1;
                        assert_eq!(count, 6400 * commits);
                        if commits == 1 {
                            for (event, text) in [
                                ("Update", ""),
                                ("Update", "five times seven"),
                                ("EndOfTurn", "five plus seven"),
                                ("EndOfTurn", "duplicate must not replay"),
                            ] {
                                ws.send(Message::Text(json!({"type":"TurnInfo","event":event,"transcript":text,"turn_index":0}).to_string().into())).await.unwrap();
                            }
                        } else {
                            ws.send(Message::Text(json!({"type":"TurnInfo","event":"EndOfTurn","transcript":"next sentence","turn_index":1}).to_string().into())).await.unwrap();
                            break;
                        }
                    }
                    _ => {}
                }
            }
        });
        let mut key = reqwest::header::HeaderValue::from_static("Token synthetic");
        key.set_sensitive(true);
        let mut stream = Flux::connect(&format!("ws://{addr}"), key, 16000)
            .await
            .unwrap();
        stream.send(&vec![0; 6400]).await.unwrap();
        assert_eq!(stream.commit().await.unwrap(), "five plus seven");
        assert_eq!(stream.stats().partial_events, 1);
        stream.send(&vec![0; 6400]).await.unwrap();
        assert_eq!(stream.commit().await.unwrap(), "next sentence");
        assert_eq!(stream.stats().committed_segments, 2);
        stream.close().await;
        server.await.unwrap();
    }
    async fn finalisation_server(
        rate: u32,
        updates: bool,
    ) -> (
        Flux,
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<bool>,
        JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (observed, ready) = tokio::sync::oneshot::channel();
        let (release, gate) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            let mut received = 0;
            loop {
                match ws.next().await.unwrap().unwrap() {
                    Message::Binary(bytes) => received += bytes.len(),
                    Message::Text(text) => {
                        assert_eq!(
                            serde_json::from_str::<serde_json::Value>(&text).unwrap()["type"],
                            "ForceEndTurn"
                        );
                        break;
                    }
                    _ => {}
                }
            }
            assert_eq!(received, rate as usize / 50 * 2);
            if updates {
                ws.send(Message::Text(json!({"type":"TurnInfo","event":"Update","transcript":"synthetic sensitive words"}).to_string().into())).await.unwrap();
            }
            observed.send(()).unwrap();
            if gate.await.unwrap() {
                ws.send(Message::Text(json!({"type":"TurnInfo","event":"EndOfTurn","turn_index":0,"transcript":"confirmed final"}).to_string().into())).await.unwrap();
            }
        });
        let stream = Flux::connect(
            &format!("ws://{addr}"),
            reqwest::header::HeaderValue::from_static("Token synthetic"),
            rate,
        )
        .await
        .unwrap();
        (stream, ready, release, server)
    }
    #[tokio::test]
    async fn missing_final_is_classified_without_leaking_partial_text_at_supported_rates() {
        for rate in [8000, 16000, 24000, 44100, 48000] {
            for updates in [false, true] {
                let (mut stream, ready, release, server) = finalisation_server(rate, updates).await;
                stream.send(&vec![0; rate as usize / 50 * 2]).await.unwrap();
                let request = tokio::spawn(async move {
                    let result = stream.commit_with_timeout(Duration::from_millis(250)).await;
                    (stream, result)
                });
                ready.await.unwrap();
                let (mut stream, result) = request.await.unwrap();
                let error = result.unwrap_err();
                assert_eq!(error.kind, VoiceErrorKind::Network);
                assert_eq!(
                    error.to_string(),
                    if updates {
                        "Deepgram final transcript timed out after receiving speech updates"
                    } else {
                        "Deepgram final transcript timed out without receiving speech updates"
                    }
                );
                assert!(!error.to_string().contains("synthetic sensitive words"));
                assert_eq!(stream.stats().partial_events, u64::from(updates));
                assert_eq!(stream.stats().committed_segments, 0);
                assert_eq!(stream.stats().audio_chunks, 1);
                release.send(false).unwrap();
                stream.close().await;
                server.await.unwrap();
            }
        }
    }
    #[tokio::test]
    async fn delayed_final_waits_for_end_of_turn_and_preserves_counters_at_supported_rates() {
        for rate in [8000, 16000, 24000, 44100, 48000] {
            let (mut stream, ready, release, server) = finalisation_server(rate, true).await;
            stream.send(&vec![0; rate as usize / 50 * 2]).await.unwrap();
            let partials = stream.partials.clone();
            let request = tokio::spawn(async move {
                let result = stream.commit_with_timeout(Duration::from_secs(5)).await;
                (stream, result)
            });
            ready.await.unwrap();
            timeout(Duration::from_secs(1), async {
                while partials.load(Ordering::Relaxed) == 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert!(
                !request.is_finished(),
                "partial text must not finish a request"
            );
            release.send(true).unwrap();
            let (mut stream, result) = request.await.unwrap();
            assert_eq!(result.unwrap(), "confirmed final");
            assert_eq!(stream.stats().partial_events, 1);
            assert_eq!(stream.stats().committed_segments, 1);
            assert_eq!(stream.stats().audio_chunks, 1);
            stream.close().await;
            server.await.unwrap();
        }
    }
    #[test]
    fn recognition_factory_rejects_unsupported_language_and_bad_headers() {
        assert!(
            crate::transcription_provider(
                "deepgram",
                "synthetic".into(),
                crate::http_client().unwrap(),
                None
            )
            .is_err()
        );
        assert!(
            crate::transcription_provider(
                "deepgram",
                "bad\nheader".into(),
                crate::http_client().unwrap(),
                Some("en")
            )
            .is_err()
        );
        let backend = crate::transcription_provider(
            "deepgram",
            "synthetic".into(),
            crate::http_client().unwrap(),
            Some("en"),
        )
        .unwrap();
        assert_eq!(backend.info().id, "deepgram");
        assert!(
            !crate::providers().iter().any(|p| p.id == "deepgram"),
            "recognition must not replace the playback service"
        );
    }
}
