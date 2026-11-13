//! Google's Android attestation revocation list: fetched, checked, cached.
//!
//! Only from [`STATUS_URL`], over HTTPS with certificate checks, no
//! redirects, a time limit and a size limit. A response is used only if it
//! parses as the documented schema. The last good list is kept in the app
//! data folder (replaced atomically) with the time *notdot* fetched it, so a
//! restart does not need the network. The verifier refuses Android keys when
//! the list is older than 48 hours, so being offline fails closed, and the
//! owner sees why.
use notdot_link::attest::android::StatusList;
use std::{
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// The only address fetched. Documented at
/// <https://developer.android.com/privacy-and-security/security-key-attestation>.
pub const STATUS_URL: &str = "https://android.googleapis.com/attestation/status";
/// The list is about 180 KB today; anything far bigger is refused.
pub const MAX_BYTES: usize = 4 * 1024 * 1024;
pub const TIMEOUT: Duration = Duration::from_secs(30);
/// Google serves it with a one-day cache lifetime; refresh twice a day.
pub const REFRESH_AFTER_MS: u64 = 12 * 3_600_000;
pub const RETRY_FIRST_MS: u64 = 5 * 60_000;
pub const RETRY_MAX_MS: u64 = 6 * 3_600_000;
const CACHE_FILE: &str = "attestation-status.json";

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Cached {
    fetched_ms: u64,
    body: String,
}

/// What the owner sees about the list.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct ListState {
    /// When the list in use was fetched.
    pub fetched_ms: Option<u64>,
    pub last_attempt_ms: Option<u64>,
    /// Plain words for the last failure, if the last attempt failed.
    pub last_error: Option<String>,
    /// When the next attempt is due.
    pub next_ms: Option<u64>,
    failures: u32,
}
impl ListState {
    /// Fresh enough for the verifier (48 hours).
    pub fn fresh(&self, now: u64) -> bool {
        self.fetched_ms.is_some_and(|f| {
            f <= now + 300_000
                && now.saturating_sub(f) <= notdot_link::attest::android::MAX_STATUS_AGE_MS
        })
    }
    pub fn due(&self, now: u64) -> bool {
        self.next_ms.is_none_or(|next| now >= next)
    }
    pub fn restore(&mut self, fetched: u64) {
        self.fetched_ms = Some(fetched);
        self.next_ms = Some(if fetched > now_ms().saturating_add(300_000) {
            0
        } else {
            fetched.saturating_add(REFRESH_AFTER_MS)
        });
    }
    fn succeeded(&mut self, fetched: u64) {
        self.fetched_ms = Some(fetched);
        self.last_attempt_ms = Some(fetched);
        self.last_error = None;
        self.failures = 0;
        self.next_ms = Some(fetched + REFRESH_AFTER_MS);
    }
    fn failed(&mut self, now: u64, error: String) {
        self.last_attempt_ms = Some(now);
        self.last_error = Some(error);
        self.failures = self.failures.saturating_add(1);
        let backoff = RETRY_FIRST_MS
            .saturating_mul(1u64 << self.failures.saturating_sub(1).min(16))
            .min(RETRY_MAX_MS);
        self.next_ms = Some(now + backoff);
    }
}

/// The last good list from disk, if it still parses.
pub fn load_cached(dir: &Path) -> Option<(StatusList, u64)> {
    let (_, cached) = notdot_link::store::JsonFile::open::<Cached>(dir, CACHE_FILE).ok()?;
    let cached = cached?;
    let list = StatusList::parse(cached.body.as_bytes(), cached.fetched_ms).ok()?;
    Some((list, cached.fetched_ms))
}

fn save_cached(dir: &Path, body: &str, fetched_ms: u64) -> Result<(), String> {
    let (file, _) = notdot_link::store::JsonFile::open::<serde_json::Value>(dir, CACHE_FILE)
        .or_else(|_| {
            // An unreadable old cache is replaced: it holds no state of its own.
            let _ = std::fs::remove_file(dir.join(CACHE_FILE));
            notdot_link::store::JsonFile::open::<serde_json::Value>(dir, CACHE_FILE)
        })
        .map_err(|e| e.to_string())?;
    file.save(&Cached {
        fetched_ms,
        body: body.to_owned(),
    })
    .map_err(|e| e.to_string())
}

/// A client that only speaks HTTPS, never follows redirects and gives up
/// after [`TIMEOUT`].
pub fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(TIMEOUT)
        .connect_timeout(Duration::from_secs(10))
        .user_agent("notdot")
        .build()
        .map_err(|e| e.to_string())
}

/// Fetch and check the list from `url` (always [`STATUS_URL`] outside
/// tests), keeping at most [`MAX_BYTES`].
async fn fetch(client: &reqwest::Client, url: &str) -> Result<String, String> {
    let mut response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                "Google didn't answer in time".to_owned()
            } else if e.is_connect() {
                "Couldn't reach Google (offline?)".to_owned()
            } else {
                "Couldn't fetch the list from Google".to_owned()
            }
        })?;
    if !response.status().is_success() {
        return Err(format!("Google answered {}", response.status().as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BYTES as u64)
    {
        return Err("The list was unexpectedly large".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "The download stopped part-way".to_owned())?
    {
        if body.len() + chunk.len() > MAX_BYTES {
            return Err("The list was unexpectedly large".into());
        }
        body.extend_from_slice(&chunk);
    }
    let text = String::from_utf8(body).map_err(|_| "The list wasn't text".to_owned())?;
    StatusList::parse(text.as_bytes(), 0)
        .map_err(|_| "The list wasn't in Google's format".to_owned())?;
    Ok(text)
}

/// One refresh: fetch, check, save, and return the list to install.
pub async fn refresh(
    client: &reqwest::Client,
    dir: &Path,
    state: &mut ListState,
) -> Option<StatusList> {
    refresh_from(client, STATUS_URL, dir, state).await
}

async fn refresh_from(
    client: &reqwest::Client,
    url: &str,
    dir: &Path,
    state: &mut ListState,
) -> Option<StatusList> {
    let started = now_ms();
    match fetch(client, url).await {
        Ok(body) => {
            let fetched = now_ms();
            if let Err(error) = save_cached(dir, &body, fetched) {
                // Still usable now; it just won't survive a restart.
                eprintln!("notdot: couldn't keep the revocation list: {error}");
            }
            state.succeeded(fetched);
            StatusList::parse(body.as_bytes(), fetched).ok()
        }
        Err(error) => {
            state.failed(started, error);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    /// A one-shot local HTTP server: tests only (the real client refuses
    /// plain HTTP).
    fn serve(response: Vec<u8>, delay: Duration) -> (String, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/status", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let handle = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            let accepted = loop {
                match listener.accept() {
                    Ok(pair) => break Some(pair),
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            && std::time::Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    _ => break None,
                }
            };
            if let Some((mut stream, _)) = accepted {
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                std::thread::sleep(delay);
                let _ = stream.write_all(&response);
            }
        });
        (url, handle)
    }

    fn http(status: &str, body: &[u8]) -> Vec<u8> {
        let mut out = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        out.extend_from_slice(body);
        out
    }

    fn test_client(timeout: Duration) -> reqwest::Client {
        reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(timeout)
            .build()
            .unwrap()
    }

    const GOOD: &[u8] = br#"{"entries":{"1a2b":{"status":"REVOKED","reason":"KEY_COMPROMISE"}}}"#;

    #[tokio::test]
    async fn good_lists_are_cached_and_bad_ones_never_replace_them() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = ListState::default();
        let (url, server) = serve(http("200 OK", GOOD), Duration::ZERO);
        let list = refresh_from(&test_client(TIMEOUT), &url, dir.path(), &mut state).await;
        server.join().unwrap();
        assert!(list.is_some());
        assert!(state.fresh(now_ms()));
        assert!(!state.due(now_ms()));
        let (cached, fetched) = load_cached(dir.path()).unwrap();
        assert_eq!(Some(fetched), state.fetched_ms);
        assert_eq!(cached.fetched_ms(), fetched);

        // Every bad answer is refused and leaves the cache as it was.
        let big = vec![b' '; MAX_BYTES + 1];
        for (response, why) in [
            (http("500 Internal Server Error", b"{}"), "Google answered 500"),
            (http("200 OK", b"not json"), "format"),
            (http("200 OK", br#"{"entries":{"0x":{"status":"FINE"}}}"#), "format"),
            (http("200 OK", &big), "large"),
            (
                b"HTTP/1.1 302 Found\r\nlocation: https://example.com/\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_vec(),
                "302",
            ),
        ] {
            let (url, server) = serve(response, Duration::ZERO);
            let before = state.fetched_ms;
            let list = refresh_from(&test_client(TIMEOUT), &url, dir.path(), &mut state).await;
            server.join().unwrap();
            assert!(list.is_none(), "{why}");
            assert!(state.last_error.as_deref().unwrap().contains(why) || why == "302", "{:?}", state.last_error);
            assert_eq!(state.fetched_ms, before, "a failure keeps the good list");
            assert!(load_cached(dir.path()).is_some());
        }
        // Retries back off, up to six hours.
        assert!(state.failures >= 5);
        let wait = state.next_ms.unwrap() - state.last_attempt_ms.unwrap();
        assert!(wait > RETRY_FIRST_MS && wait <= RETRY_MAX_MS, "{wait}");
    }

    #[tokio::test]
    async fn a_slow_server_times_out_and_the_real_client_refuses_plain_http() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = ListState::default();
        let (url, _server) = serve(http("200 OK", GOOD), Duration::from_secs(3));
        let list = refresh_from(
            &test_client(Duration::from_millis(300)),
            &url,
            dir.path(),
            &mut state,
        )
        .await;
        assert!(list.is_none());
        assert!(
            state.last_error.as_deref().unwrap().contains("in time"),
            "{:?}",
            state.last_error
        );
        assert!(!state.fresh(now_ms()));
        // The production client never fetches over plain HTTP.
        let (url, _server) = serve(http("200 OK", GOOD), Duration::ZERO);
        assert!(fetch(&client().unwrap(), &url).await.is_err());
    }

    #[test]
    fn freshness_follows_the_verifier() {
        let now = 10 * notdot_link::attest::android::MAX_STATUS_AGE_MS;
        let mut state = ListState::default();
        assert!(!state.fresh(now) && state.due(now));
        state.succeeded(now - notdot_link::attest::android::MAX_STATUS_AGE_MS);
        assert!(state.fresh(now));
        state.succeeded(now - notdot_link::attest::android::MAX_STATUS_AGE_MS - 1);
        assert!(!state.fresh(now));
        // A clock far behind the list is not fresh either.
        state.succeeded(now + 3_600_000);
        assert!(!state.fresh(now));
    }
    #[tokio::test]
    #[ignore = "explicit online check against Google's public status endpoint"]
    async fn live_google_list_uses_the_production_tls_client_and_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = ListState::default();
        assert!(
            refresh(&client().unwrap(), dir.path(), &mut state)
                .await
                .is_some(),
            "{:?}",
            state.last_error
        );
        assert!(state.fresh(now_ms()));
        let (list, fetched) = load_cached(dir.path()).unwrap();
        assert_eq!(list.fetched_ms(), fetched);
        let mut restored = ListState::default();
        restored.restore(fetched);
        assert!(restored.fresh(now_ms()));
        assert!(!restored.due(now_ms()));
    }
}
