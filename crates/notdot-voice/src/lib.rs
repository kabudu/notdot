#![forbid(unsafe_code)]
//! Voice-provider boundary. Providers own HTTP protocols and audio formats; the app owns credentials.
pub mod quota;
pub mod usage;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;
pub use usage::{UsageReport, VoiceError, VoiceErrorKind};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Voice {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(skip_serializing)]
    pub preview_url: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VoicePage {
    pub voices: Vec<Voice>,
    pub next_cursor: Option<String>,
}
#[derive(Clone, Debug)]
pub struct AudioClip {
    pub bytes: Vec<u8>,
    pub mime: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct ProviderInfo {
    pub id: &'static str,
    pub name: &'static str,
}
/// Implement this interface and register a factory to add a backend. No frontend protocol changes required.
#[async_trait]
pub trait VoiceBackend: Send + Sync {
    fn info(&self) -> ProviderInfo;
    async fn usage(&self) -> Result<UsageReport, VoiceError> {
        Err(VoiceError::new(
            VoiceErrorKind::Unsupported,
            "This backend does not expose usage",
        ))
    }
    async fn voices(&self, search: &str, cursor: Option<&str>) -> Result<VoicePage, VoiceError>;
    async fn transcribe(&self, clip: AudioClip) -> Result<String, VoiceError>;
    async fn synthesize(&self, text: &str, voice: &str) -> Result<AudioClip, VoiceError>;
    async fn preview(&self, voice: &str) -> Result<AudioClip, VoiceError>;
}
pub fn providers() -> Vec<ProviderInfo> {
    vec![ProviderInfo {
        id: "elevenlabs",
        name: "ElevenLabs",
    }]
}
pub fn provider(
    id: &str,
    key: String,
    client: reqwest::Client,
) -> Result<Box<dyn VoiceBackend>, String> {
    match id {
        "elevenlabs" => Ok(Box::new(ElevenLabs {
            key,
            client,
            endpoint: "https://api.elevenlabs.io".into(),
        })),
        _ => Err(format!("Voice backend {id:?} is not installed")),
    }
}
pub fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(45))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())
}
struct ElevenLabs {
    key: String,
    client: reqwest::Client,
    endpoint: String,
}
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}
async fn body(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, VoiceError> {
    let status = response.status();
    let limit = if status.is_success() {
        limit
    } else {
        64 * 1024
    };
    if response.content_length().is_some_and(|n| n > limit as u64) {
        return Err(if status.is_success() {
            VoiceError::from("Voice service response exceeded the size limit")
        } else {
            usage::response_error(status.as_u16(), &[])
        });
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| {
        VoiceError::new(
            VoiceErrorKind::Network,
            "Voice service response interrupted",
        )
    })? {
        if bytes.len() + chunk.len() > limit {
            return Err(if status.is_success() {
                VoiceError::from("Voice service response exceeded the size limit")
            } else {
                usage::response_error(status.as_u16(), &[])
            });
        }
        bytes.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        return Err(usage::response_error(status.as_u16(), &bytes));
    }
    Ok(bytes)
}
impl ElevenLabs {
    async fn page(
        &self,
        search: &str,
        cursor: Option<&str>,
        voice: Option<&str>,
    ) -> Result<VoicePage, VoiceError> {
        if search.len() > 200 || cursor.is_some_and(|c| c.len() > 2048) {
            return Err("Voice search is too long".into());
        }
        let mut request = self
            .client
            .get(format!("{}/v2/voices", self.endpoint))
            .header("xi-api-key", &self.key)
            .query(&[
                ("page_size", "100"),
                ("include_total_count", "false"),
                ("sort", "name"),
                ("sort_direction", "asc"),
            ]);
        if !search.trim().is_empty() {
            request = request.query(&[("search", search)]);
        }
        if let Some(cursor) = cursor {
            request = request.query(&[("next_page_token", cursor)]);
        }
        if let Some(voice) = voice {
            request = request.query(&[("voice_ids", voice)]);
        }
        let response = request.send().await.map_err(|_| {
            VoiceError::new(
                VoiceErrorKind::Network,
                "Could not connect to the voice service",
            )
        })?;
        let value: Value = serde_json::from_slice(&body(response, 2_000_000).await?)
            .map_err(|_| "Voice service returned invalid JSON")?;
        let items = value["voices"]
            .as_array()
            .ok_or("Voice service returned no voice list")?;
        if items.len() > 200 {
            return Err("Voice service returned too many voices in one page".into());
        }
        let mut voices = Vec::new();
        for v in items {
            let Some(id) = v["voice_id"].as_str().filter(|id| valid_id(id)) else {
                continue;
            };
            let name = v["name"]
                .as_str()
                .unwrap_or("Unnamed voice")
                .chars()
                .take(100)
                .collect();
            let labels = v["labels"]
                .as_object()
                .map(|labels| {
                    labels
                        .values()
                        .filter_map(Value::as_str)
                        .take(4)
                        .collect::<Vec<_>>()
                        .join(" · ")
                })
                .unwrap_or_default();
            let description = v["description"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or(&labels)
                .chars()
                .take(240)
                .collect();
            voices.push(Voice {
                id: id.into(),
                name,
                description,
                preview_url: v["preview_url"].as_str().map(str::to_owned),
            });
        }
        let next_cursor = if value["has_more"].as_bool().unwrap_or(false) {
            Some(
                value["next_page_token"]
                    .as_str()
                    .filter(|c| !c.is_empty() && Some(*c) != cursor)
                    .ok_or("Voice service pagination did not advance")?
                    .to_string(),
            )
        } else {
            None
        };
        Ok(VoicePage {
            voices,
            next_cursor,
        })
    }
}
#[async_trait]
impl VoiceBackend for ElevenLabs {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "elevenlabs",
            name: "ElevenLabs",
        }
    }
    async fn usage(&self) -> Result<UsageReport, VoiceError> {
        let response = self
            .client
            .get(format!("{}/v1/user", self.endpoint))
            .timeout(Duration::from_secs(15))
            .header("xi-api-key", &self.key)
            .send()
            .await
            .map_err(|_| {
                VoiceError::new(
                    VoiceErrorKind::Network,
                    "Could not connect to usage service",
                )
            })?;
        let value: Value =
            serde_json::from_slice(&body(response, 256 * 1024).await.map_err(|error| {
                if error.kind == VoiceErrorKind::Permission {
                    VoiceError::new(VoiceErrorKind::Permission,
                        "This key cannot read account usage. Voice permissions are separate, so speech may still work. Allow read access to User in ElevenLabs key settings to show usage, or keep using voice without balance monitoring.")
                } else {
                    error
                }
            })?).map_err(|_| {
                VoiceError::new(
                    VoiceErrorKind::Provider,
                    "Usage service returned invalid JSON",
                )
            })?;
        usage::elevenlabs_usage(&value)
    }
    async fn voices(&self, search: &str, cursor: Option<&str>) -> Result<VoicePage, VoiceError> {
        self.page(search, cursor, None).await
    }
    async fn transcribe(&self, clip: AudioClip) -> Result<String, VoiceError> {
        if clip.bytes.is_empty() || clip.bytes.len() > 10_000_000 {
            return Err("Recording must contain audio and be under 10 MB".into());
        }
        let (mime, file) = match clip.mime.split(';').next().unwrap_or("") {
            "audio/mp4" => ("audio/mp4", "recording.mp4"),
            "audio/webm" => ("audio/webm", "recording.webm"),
            "audio/mpeg" => ("audio/mpeg", "recording.mp3"),
            "audio/wav" => ("audio/wav", "recording.wav"),
            _ => return Err("Unsupported recording format".into()),
        };
        let part = reqwest::multipart::Part::bytes(clip.bytes)
            .file_name(file)
            .mime_str(mime)
            .map_err(|e| e.to_string())?;
        let response = self
            .client
            .post(format!("{}/v1/speech-to-text", self.endpoint))
            .header("xi-api-key", &self.key)
            .multipart(
                reqwest::multipart::Form::new()
                    .text("model_id", "scribe_v2")
                    .text("timestamps_granularity", "none")
                    .part("file", part),
            )
            .send()
            .await
            .map_err(|_| {
                VoiceError::new(
                    VoiceErrorKind::Network,
                    "Could not connect to transcription service",
                )
            })?;
        let value: Value = serde_json::from_slice(&body(response, 1_000_000).await?)
            .map_err(|_| "Transcription returned invalid JSON")?;
        value["text"]
            .as_str()
            .filter(|s| !s.trim().is_empty() && s.len() <= 24_000)
            .map(str::to_owned)
            .ok_or_else(|| VoiceError::from("No usable speech was transcribed"))
    }
    async fn synthesize(&self, text: &str, voice: &str) -> Result<AudioClip, VoiceError> {
        if !valid_id(voice) {
            return Err("Choose a voice in Settings".into());
        }
        if text.trim().is_empty() || text.len() > 12_000 {
            return Err("Speech text must contain 1–12,000 bytes".into());
        }
        let response = self
            .client
            .post(format!("{}/v1/text-to-speech/{voice}", self.endpoint))
            .header("xi-api-key", &self.key)
            .json(&serde_json::json!({"text":text,"model_id":"eleven_flash_v2_5"}))
            .send()
            .await
            .map_err(|_| {
                VoiceError::new(
                    VoiceErrorKind::Network,
                    "Could not connect to speech service",
                )
            })?;
        Ok(AudioClip {
            bytes: body(response, 20_000_000).await?,
            mime: "audio/mpeg".into(),
        })
    }
    async fn preview(&self, voice: &str) -> Result<AudioClip, VoiceError> {
        if !valid_id(voice) {
            return Err("Choose a voice to preview".into());
        }
        let page = self.page("", None, Some(voice)).await?;
        let selected = page
            .voices
            .into_iter()
            .find(|v| v.id == voice)
            .ok_or("This voice is no longer available")?;
        if let Some(url) = selected.preview_url.filter(|url| safe_preview_url(url)) {
            let response = self.client.get(url).send().await.map_err(|_| {
                VoiceError::new(
                    VoiceErrorKind::Network,
                    "Voice preview could not be downloaded",
                )
            })?;
            return Ok(AudioClip {
                bytes: body(response, 10_000_000).await?,
                mime: "audio/mpeg".into(),
            });
        }
        self.synthesize(
            "Hello. I'm your companion. How can I help you today?",
            voice,
        )
        .await
    }
}
fn safe_preview_url(value: &str) -> bool {
    reqwest::Url::parse(value).ok().is_some_and(|u| {
        u.scheme() == "https"
            && u.username().is_empty()
            && u.password().is_none()
            && u.port().is_none()
            && u.host_str().is_some_and(|h| {
                h == "storage.googleapis.com"
                    || h == "elevenlabs.io"
                    || h.ends_with(".elevenlabs.io")
            })
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preview_urls_cannot_reach_local_services() {
        for url in [
            "http://localhost/a",
            "https://127.0.0.1/a",
            "https://elevenlabs.io.evil.test/a",
            "https://user:password@storage.googleapis.com/a",
        ] {
            assert!(!safe_preview_url(url));
        }
        assert!(safe_preview_url(
            "https://storage.googleapis.com/eleven-public-prod/preview.mp3"
        ));
    }
    #[tokio::test]
    async fn invalid_inputs_fail_before_network() {
        let p = provider("elevenlabs", "fake".into(), http_client().unwrap()).unwrap();
        assert!(p.synthesize("hello", "../bad").await.is_err());
        assert!(
            p.transcribe(AudioClip {
                bytes: vec![],
                mime: "audio/mp4".into()
            })
            .await
            .is_err()
        );
        assert!(p.voices(&"x".repeat(201), None).await.is_err());
        assert!(provider("missing", "".into(), http_client().unwrap()).is_err());
    }
    #[tokio::test]
    async fn voice_search_decodes_pagination_and_sends_search() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 8192];
            let n = socket.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..n]);
            assert!(request.contains("search=warm"));
            assert!(request.contains("xi-api-key: fake"));
            let json = r#"{"voices":[{"voice_id":"abc","name":"Warm voice","labels":{"accent":"British"}}],"has_more":true,"next_page_token":"next"}"#;
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",json.len()).as_bytes()).await.unwrap();
        });
        let p = ElevenLabs {
            key: "fake".into(),
            client: http_client().unwrap(),
            endpoint: format!("http://{addr}"),
        };
        let page = p.voices("warm", None).await.unwrap();
        assert_eq!(page.voices[0].name, "Warm voice");
        assert_eq!(page.next_cursor.as_deref(), Some("next"));
        task.await.unwrap();
    }
}
