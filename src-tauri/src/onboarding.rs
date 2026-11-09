//! Setup owns a bounded, temporary auth runtime. It never reads auth files or tokens.
use super::{AppState, Bridge, startup};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tauri::{Manager, State};
use tokio::sync::Mutex;
#[derive(Default)]
pub struct Setup {
    auth: Mutex<Auth>,
    pub trial: AtomicBool,
    pub spoken: AtomicBool,
    pub received: AtomicBool,
    cancellation: AtomicU64,
}
#[derive(Default)]
struct Auth {
    bridge: Option<Bridge>,
    login: Option<String>,
    started: Option<Instant>,
}
impl Auth {
    async fn expire(&mut self) -> bool {
        if self
            .started
            .is_some_and(|time| time.elapsed() > Duration::from_secs(300))
        {
            self.close().await;
            true
        } else {
            false
        }
    }

    async fn close(&mut self) {
        if let Some(bridge) = self.bridge.take() {
            if let Some(id) = self.login.take() {
                let _ = tokio::time::timeout(
                    Duration::from_secs(2),
                    bridge.call("account/login/cancel", json!({"loginId":id})),
                )
                .await;
            }
            bridge.shutdown().await;
        }
        self.login = None;
        self.started = None;
    }
}
fn account_connected(account: &Value) -> bool {
    account["account"].as_object().is_some_and(|a| {
        a.get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| matches!(kind, "chatgpt" | "apiKey" | "amazonBedrock"))
    }) || (account["account"].is_null() && account["requiresOpenaiAuth"] == false)
}
fn browser_url(raw: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(raw).map_err(|_| "Invalid sign-in address")?;
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some("chatgpt.com" | "auth.openai.com" | "learn.chatgpt.com")
        )
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return Err("Unexpected sign-in address. Please retry setup.".into());
    }
    Ok(url)
}
async fn open_browser(raw: &str) -> Result<(), String> {
    let url = browser_url(raw)?;
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = tokio::process::Command::new("/usr/bin/open");
        c.arg(url.as_str());
        c
    };
    #[cfg(target_os = "linux")]
    let mut command = {
        let mut c = tokio::process::Command::new("xdg-open");
        c.arg(url.as_str());
        c
    };
    #[cfg(windows)]
    let mut command = {
        let mut c = tokio::process::Command::new("rundll32.exe");
        c.args(["url.dll,FileProtocolHandler", url.as_str()])
            .creation_flags(0x08000000);
        c
    };
    command
        .kill_on_drop(true)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let status = tokio::time::timeout(Duration::from_secs(10), command.status())
        .await
        .map_err(|_| "Opening your browser took too long")?
        .map_err(|_| "Could not open your default browser")?;
    if status.success() {
        Ok(())
    } else {
        Err("Could not open your browser. Check your default browser and retry.".into())
    }
}
#[tauri::command]
pub async fn setup_codex(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    action: String,
) -> Result<Value, String> {
    if action != "cancel" {
        state.require_owner_voice()?;
    }
    if !matches!(
        action.as_str(),
        "check" | "install" | "signin" | "cancel" | "trial" | "finish"
    ) {
        return Err("Unknown setup action".into());
    }
    let window = app
        .get_webview_window("main")
        .ok_or("Show notdot to continue setup")?;
    if !window.is_visible().unwrap_or(false) {
        return Err("Show notdot to continue setup".into());
    }
    if action == "trial" {
        let s = state.snapshot.lock().await;
        if s.phase != "idle"
            || s.active_turn.is_some()
            || !state.credentials.cached(&s.settings.voice_provider)
            || !state.credentials.cached(s.settings.transcription_id())
            || s.settings.voice_id.is_empty()
            || s.settings.microphone_muted
        {
            return Err(
                "Connect Codex, save your voice settings and unmute the microphone first.".into(),
            );
        }
        state.setup.spoken.store(false, Ordering::Relaxed);
        state.setup.received.store(false, Ordering::Relaxed);
        state.setup.trial.store(true, Ordering::Relaxed);
        return Ok(
            json!({"stage":"trial","message":"Say: What is five plus seven? Then pause and listen for the answer."}),
        );
    }
    if action == "finish" {
        let mut s = state.snapshot.lock().await;
        if !state.setup.spoken.load(Ordering::Relaxed)
            || !state.setup.trial.load(Ordering::Relaxed)
            || s.active_turn.is_some()
            || s.phase != "idle"
        {
            return Err("Try a spoken conversation before finishing setup.".into());
        }
        let mut settings = s.settings.clone();
        settings.onboarding_complete = true;
        settings.save(&state.path)?;
        s.settings = settings;
        state.setup.trial.store(false, Ordering::Relaxed);
        return Ok(
            json!({"stage":"complete","message":"You’re ready. Your choices can be changed in Settings."}),
        );
    }
    if action == "cancel" {
        state.setup.cancellation.fetch_add(1, Ordering::Relaxed);
    }
    let generation = state.setup.cancellation.load(Ordering::Relaxed);
    let mut auth = if action == "cancel" {
        state.setup.auth.lock().await
    } else {
        state
            .setup
            .auth
            .try_lock()
            .map_err(|_| "A setup check is already running")?
    };
    if action == "cancel" {
        auth.close().await;
        state.setup.trial.store(false, Ordering::Relaxed);
        return Ok(
            json!({"stage":"signin","message":"Sign-in cancelled. You can continue later."}),
        );
    }
    if action == "install" {
        open_browser("https://learn.chatgpt.com/docs/quickstart").await?;
        return Ok(
            json!({"stage":"install","message":"Install Codex from the official page in your browser, then choose Check again. You can use the desktop app or native command-line tool."}),
        );
    }
    if auth.expire().await {
        return Ok(
            json!({"stage":"signin","message":"Sign-in expired. Choose Sign in to try again."}),
        );
    }
    // Reuse the live transport, but recheck account state: a running process
    // does not prove that its credentials are still valid.
    if action == "check" && auth.bridge.is_none() {
        let live = state.context().bridge.lock().await.clone();
        if let Some(live) = live {
            let account = tokio::time::timeout(
                Duration::from_secs(15),
                live.call("account/read", json!({"refreshToken":false})),
            )
            .await;
            match account {
                Ok(Ok(value)) if account_connected(&value) => {
                    return Ok(
                        json!({"stage":"ready","message":"Codex is connected. Its desktop window can stay closed.","trial_passed":state.setup.trial.load(Ordering::Relaxed) && state.setup.spoken.load(Ordering::Relaxed)}),
                    );
                }
                Ok(Ok(_)) => {}
                _ => {
                    return Err(
                        "Could not check Codex sign-in. Check your connection and try again."
                            .into(),
                    );
                }
            }
        }
    }
    if auth.bridge.is_none() {
        match tokio::time::timeout(
            Duration::from_secs(15),
            Bridge::connect(&startup::runtime()),
        )
        .await
        {
            Ok(Ok(b)) => auth.bridge = Some(b),
            _ => {
                return Ok(
                    json!({"stage":"install","message":"Codex could not be started. Install it, or check your existing installation, then try again."}),
                );
            }
        }
    }
    let account = tokio::time::timeout(
        Duration::from_secs(15),
        auth.bridge
            .as_ref()
            .expect("auth runtime")
            .call("account/read", json!({"refreshToken":false})),
    )
    .await;
    let account = match account {
        Ok(Ok(a)) => a,
        _ => {
            auth.close().await;
            return Err(
                "Could not check Codex sign-in. Check your connection and try again.".into(),
            );
        }
    };
    if state.setup.cancellation.load(Ordering::Relaxed) != generation {
        auth.close().await;
        return Err("Setup was cancelled. You can continue later.".into());
    }
    if account_connected(&account) {
        let signed_in = auth.login.is_some();
        auth.close().await;
        if signed_in || state.context().bridge.lock().await.is_none() {
            super::connect(state.inner().clone(), false).await?;
        }
        if state.setup.cancellation.load(Ordering::Relaxed) == generation {
            let _ = window.set_focus();
        }
        return Ok(
            json!({"stage":"ready","message":"Codex is connected. Its desktop window can stay closed.","trial_passed":state.setup.trial.load(Ordering::Relaxed) && state.setup.spoken.load(Ordering::Relaxed)}),
        );
    }
    if state.setup.cancellation.load(Ordering::Relaxed) != generation {
        auth.close().await;
        return Err("Setup was cancelled. You can continue later.".into());
    }
    if action == "signin" && auth.login.is_none() {
        let login = tokio::time::timeout(
            Duration::from_secs(15),
            auth.bridge
                .as_ref()
                .expect("auth runtime")
                .call("account/login/start", json!({"type":"chatgpt"})),
        )
        .await;
        let login = match login {
            Ok(Ok(value)) => value,
            _ => {
                auth.close().await;
                return Err("Could not start sign-in. Please try again.".into());
            }
        };
        if state.setup.cancellation.load(Ordering::Relaxed) != generation {
            auth.close().await;
            return Err("Setup was cancelled. You can continue later.".into());
        }
        let id = login["loginId"]
            .as_str()
            .filter(|s| s.len() <= 128)
            .ok_or("Could not start sign-in")?
            .to_owned();
        auth.login = Some(id);
        auth.started = Some(Instant::now());
        let result = match login["authUrl"].as_str() {
            Some(url) => open_browser(url).await,
            None => Err("Sign-in did not return a browser address".into()),
        };
        if let Err(e) = result {
            auth.close().await;
            return Err(e);
        }
    }
    let waiting = auth.login.is_some();
    if !waiting {
        auth.close().await;
    }
    Ok(
        json!({"stage":if waiting{"waiting"}else{"signin"},"message":if waiting{"Finish signing in in your browser. Keep this window open; notdot will reconnect when you return."}else{"Sign in to your ChatGPT account to connect Codex."}}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[tokio::test]
    async fn expired_managed_login_is_cancelled_and_its_runtime_released() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("codex-fixture");
        let log = dir.path().join("cancelled.json");
        let source = format!(
            "#!/usr/bin/env python3\nimport sys,json\nfor line in sys.stdin:\n m=json.loads(line)\n if m.get('method')=='account/login/cancel':open({:?},'w').write(json.dumps(m['params']))\n if 'id' in m:print(json.dumps({{'id':m['id'],'result':{{}}}}),flush=True)\n",
            log.to_str().unwrap()
        );
        std::fs::write(&program, source).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let bridge = Bridge::connect(&program).await.unwrap();
        let mut auth = Auth {
            bridge: Some(bridge),
            login: Some("synthetic-login".into()),
            started: Some(Instant::now() - Duration::from_secs(301)),
        };
        assert!(auth.expire().await);
        assert!(auth.bridge.is_none());
        assert!(auth.login.is_none());
        assert!(auth.started.is_none());
        let cancelled: Value = serde_json::from_slice(&std::fs::read(log).unwrap()).unwrap();
        assert_eq!(cancelled, json!({"loginId":"synthetic-login"}));
        assert!(!auth.expire().await);
    }
    #[test]
    fn account_states_distinguish_signed_out_from_connected_and_malformed() {
        for value in [
            json!({"account":{"type":"chatgpt"},"requiresOpenaiAuth":true}),
            json!({"account":{"type":"apiKey"}}),
            json!({"account":null,"requiresOpenaiAuth":false}),
        ] {
            assert!(account_connected(&value));
        }
        for value in [
            json!({}),
            json!({"account":"unexpected"}),
            json!({"account":{},"requiresOpenaiAuth":true}),
            json!({"account":null,"requiresOpenaiAuth":true}),
        ] {
            assert!(!account_connected(&value));
        }
    }
    #[test]
    fn browser_handoff_only_opens_expected_https_hosts() {
        for s in [
            "https://chatgpt.com/auth?state=example",
            "https://auth.openai.com/authorize",
            "https://learn.chatgpt.com/docs/quickstart",
        ] {
            assert!(browser_url(s).is_ok());
        }
        for s in [
            "file:///etc/passwd",
            "https://chatgpt.com.evil.test/",
            "http://chatgpt.com/",
            "https://user@chatgpt.com/",
            "https://chatgpt.com:444/",
            "javascript:alert(1)",
        ] {
            assert!(browser_url(s).is_err());
        }
    }
}
