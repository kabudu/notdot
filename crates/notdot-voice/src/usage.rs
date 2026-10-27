use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceErrorKind {
    Authentication,
    Permission,
    Quota,
    RateLimit,
    Network,
    InvalidInput,
    Provider,
    Unsupported,
}
#[derive(Clone, Debug)]
pub struct VoiceError {
    pub kind: VoiceErrorKind,
    message: String,
}
impl VoiceError {
    pub fn new(kind: VoiceErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}
impl std::fmt::Display for VoiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for VoiceError {}
impl From<&str> for VoiceError {
    fn from(s: &str) -> Self {
        Self::new(VoiceErrorKind::Provider, s)
    }
}
impl From<String> for VoiceError {
    fn from(s: String) -> Self {
        Self::new(VoiceErrorKind::Provider, s)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UsageReport {
    /// Provider account/workspace identity, never an API credential.
    pub account: String,
    pub meters: Vec<UsageMeter>,
    pub note: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UsageMeter {
    pub id: String,
    pub label: String,
    pub unit: String,
    pub used: u64,
    pub limit: u64,
    pub reset_unix: Option<u64>,
    /// Whether existing usage-based billing is enabled; never authorizes changing it.
    pub overage: Overage,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Overage {
    Enabled,
    Disabled,
    Unknown,
}
impl UsageMeter {
    pub fn threshold(&self) -> Option<u8> {
        if self.limit == 0 {
            return None;
        }
        [100u8, 90, 80]
            .into_iter()
            .find(|p| u128::from(self.used) * 100 >= u128::from(self.limit) * u128::from(*p))
    }
}
pub(crate) fn elevenlabs_usage(value: &Value) -> Result<UsageReport, VoiceError> {
    let account = value["user_id"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 128)
        .ok_or("Usage unavailable: account identity was not reported")?;
    let workspace = value["workspace_id"]
        .as_str()
        .filter(|s| s.len() <= 128)
        .unwrap_or("");
    let subscription = &value["subscription"];
    let used = subscription["character_count"]
        .as_u64()
        .ok_or("Usage unavailable: used allowance was not reported")?;
    let limit = subscription["character_limit"]
        .as_u64()
        .ok_or("Usage unavailable: allowance limit was not reported")?;
    let cap = &subscription["max_credit_limit_extension"];
    let overage = match (subscription["can_extend_character_limit"].as_bool(), cap) {
        (Some(false), _) => Overage::Disabled,
        (_, Value::Number(n)) if n.as_u64() == Some(0) => Overage::Disabled,
        (Some(true), Value::Number(n)) if n.as_u64().is_some_and(|n| n > 0) => Overage::Enabled,
        (Some(true), Value::String(s)) if s == "unlimited" => Overage::Enabled,
        _ => Overage::Unknown,
    };
    Ok(UsageReport { account: format!("{account}:{workspace}"), meters: vec![UsageMeter { id: "subscription".into(), label: "Subscription allowance".into(), unit: "provider credits".into(), used, limit, reset_unix: subscription["next_character_count_reset_unix"].as_u64().filter(|n| *n > 0), overage }], note: "This is the provider’s subscription allowance. It does not show how many recording minutes or spoken replies remain. Separate speech-to-text charges, pay-as-you-go balances and key limits may apply.".into() })
}
pub(crate) fn response_error(status: u16, bytes: &[u8]) -> VoiceError {
    let body: Value = serde_json::from_slice(bytes).unwrap_or(Value::Null);
    let detail = &body["detail"];
    let code = detail["code"]
        .as_str()
        .or_else(|| detail["status"].as_str())
        .unwrap_or("");
    let kind = detail["type"].as_str().unwrap_or("");
    // Legacy providers sometimes report exhausted quota as HTTP 401. Semantic codes win.
    let (kind, message) = if matches!(
        code,
        "insufficient_credits" | "quota_exceeded" | "insufficient_quota"
    ) || kind == "payment_required"
        || status == 402
    {
        (
            VoiceErrorKind::Quota,
            "Voice service has insufficient credits or requires payment. Check the provider account; notdot will not enable extra spending.",
        )
    } else if matches!(
        code,
        "missing_permissions" | "insufficient_permissions" | "permission_denied"
    ) || kind == "authorization_error"
        || status == 403
    {
        (
            VoiceErrorKind::Permission,
            "The API key lacks permission for this voice service operation.",
        )
    } else if matches!(code, "invalid_api_key" | "missing_api_key" | "unauthorized")
        || kind == "authentication_error"
        || status == 401
    {
        (
            VoiceErrorKind::Authentication,
            "Voice service rejected the API key. Check your saved key in Settings.",
        )
    } else if kind == "rate_limit_error" || status == 429 {
        (
            VoiceErrorKind::RateLimit,
            "Voice service is temporarily rate limited or busy. Try again shortly.",
        )
    } else {
        (
            VoiceErrorKind::Provider,
            "Voice service could not complete the request. Try again later.",
        )
    };
    VoiceError::new(kind, message)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{ElevenLabs, VoiceBackend, http_client};
    use serde_json::json;
    pub(crate) fn report(used: u64, reset: Option<u64>) -> UsageReport {
        elevenlabs_usage(&json!({"user_id":"account", "workspace_id":"workspace", "subscription":{"character_count":used,"character_limit":1000,"next_character_count_reset_unix":reset,"can_extend_character_limit":true,"max_credit_limit_extension":1000}})).unwrap()
    }
    #[test]
    fn accounting_and_unknown_fields_are_not_invented() {
        let mut value = json!({"user_id":"account","workspace_id":"ws","subscription":{"character_count":800,"character_limit":1000,"max_credit_limit_extension":"unlimited","can_extend_character_limit":true}});
        let r = elevenlabs_usage(&value).unwrap();
        assert_eq!(r.meters.len(), 1);
        assert_eq!(r.account, "account:ws");
        assert_eq!(r.meters[0].reset_unix, None);
        assert_eq!(r.meters[0].overage, Overage::Enabled);
        assert_eq!(r.meters[0].threshold(), Some(80));
        value["subscription"]["max_credit_limit_extension"] = json!(0);
        assert_eq!(
            elevenlabs_usage(&value).unwrap().meters[0].overage,
            Overage::Disabled
        );
        value["subscription"]["max_credit_limit_extension"] = Value::Null;
        assert_eq!(
            elevenlabs_usage(&value).unwrap().meters[0].overage,
            Overage::Unknown
        );
        value["subscription"]["character_limit"] = Value::Null;
        assert!(elevenlabs_usage(&value).is_err());
        value["subscription"]["character_limit"] = json!(0);
        assert_eq!(
            elevenlabs_usage(&value).unwrap().meters[0].threshold(),
            None
        );
        let mut m = report(u64::MAX, Some(9999)).meters.remove(0);
        m.limit = u64::MAX;
        assert_eq!(m.threshold(), Some(100));
    }
    async fn fixture(
        status: u16,
        body: String,
        route: &'static str,
    ) -> (ElevenLabs, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut bytes = [0; 4096];
                let n = socket.read(&mut bytes).await.unwrap();
                assert!(n > 0);
                request.extend_from_slice(&bytes[..n]);
                assert!(request.len() < 16 * 1024);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|n| n.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            let text = String::from_utf8_lossy(&request);
            assert!(text.starts_with(route));
            assert!(text.contains("xi-api-key: fake"));
            socket.write_all(format!("HTTP/1.1 {status} Result\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        });
        (
            ElevenLabs {
                key: "fake".into(),
                client: http_client().unwrap(),
                endpoint: format!("http://{address}"),
            },
            server,
        )
    }
    #[tokio::test]
    async fn subscription_usage_uses_authenticated_provider_boundary() {
        let (provider, server) = fixture(200, json!({"user_id":"a","workspace_id":"w","subscription":{"character_count":950,"character_limit":1000,"next_character_count_reset_unix":9999}}).to_string(), "GET /v1/user ").await;
        let report = provider.usage().await.unwrap();
        assert_eq!(report.account, "a:w");
        assert_eq!(report.meters[0].threshold(), Some(90));
        assert!(report.note.contains("recording minutes"));
        server.await.unwrap();
    }
    #[tokio::test]
    async fn provider_http_errors_preserve_categories_without_echoing_body() {
        for (status, code, expected) in [
            (402, "insufficient_credits", VoiceErrorKind::Quota),
            (401, "quota_exceeded", VoiceErrorKind::Quota),
            (401, "invalid_api_key", VoiceErrorKind::Authentication),
            (401, "missing_permissions", VoiceErrorKind::Permission),
            (401, "insufficient_permissions", VoiceErrorKind::Permission),
            (403, "insufficient_permissions", VoiceErrorKind::Permission),
            (429, "rate_limit_exceeded", VoiceErrorKind::RateLimit),
        ] {
            let (provider, server) = fixture(
                status,
                json!({"detail":{"code":code,"message":"SECRET_FIXTURE"}}).to_string(),
                "POST /v1/text-to-speech/testvoice ",
            )
            .await;
            let error = provider.synthesize("Hello", "testvoice").await.unwrap_err();
            assert_eq!(error.kind, expected);
            assert!(!error.to_string().contains("SECRET_FIXTURE"));
            server.await.unwrap();
        }
    }
    #[tokio::test]
    async fn usage_permission_failure_is_explicitly_unavailable() {
        for status in [401, 403] {
            let (provider, server) = fixture(
                status,
                json!({"detail":{"status":"missing_permissions", "message":"PRIVATE_FIXTURE"}})
                    .to_string(),
                "GET /v1/user ",
            )
            .await;
            let error = provider.usage().await.unwrap_err();
            assert_eq!(error.kind, VoiceErrorKind::Permission);
            assert!(error.to_string().contains("cannot read account usage"));
            assert!(!error.to_string().contains("rejected"));
            assert!(!error.to_string().contains("PRIVATE_FIXTURE"));
            server.await.unwrap();
        }
    }
}
