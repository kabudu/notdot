use super::AppState;
use notdot_core::update_consent::{Answer, parse};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::State;
use tauri_plugin_updater::{Update, UpdaterExt};
const PERIOD: u64 = 6 * 60 * 60;
const MAX_DOWNLOAD: u64 = 256 * 1024 * 1024;
const PACKAGE_MANAGER_GUIDANCE: &str = "Update notdot using your Linux package manager. In-app installation is available for the AppImage version.";
fn supports_in_app_install(os: &str, bundle: Option<tauri::utils::config::BundleType>) -> bool {
    os != "linux" || matches!(bundle, Some(tauri::utils::config::BundleType::AppImage))
}
fn can_install_here() -> bool {
    supports_in_app_install(std::env::consts::OS, tauri::utils::platform::bundle_type())
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Status {
    pub phase: String,
    pub version: Option<String>,
    pub message: String,
    pub received: u64,
    pub total: Option<u64>,
    pub prompt: bool,
}
#[derive(Default, Serialize, Deserialize)]
struct Journal {
    last_check: u64,
    deferred: Option<String>,
}
#[derive(Default)]
struct Inner {
    status: Status,
    update: Option<Update>,
    consent: Option<(String, Instant)>,
    journal: Journal,
}
pub struct Updates {
    inner: Mutex<Inner>,
    operation: tokio::sync::Mutex<()>,
    journal: PathBuf,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
impl Updates {
    pub fn new(path: PathBuf) -> Self {
        let journal = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            inner: Mutex::new(Inner {
                journal,
                status: Status {
                    phase: "idle".into(),
                    message: "Updates are checked every six hours when enabled".into(),
                    ..Status::default()
                },
                ..Inner::default()
            }),
            operation: tokio::sync::Mutex::new(()),
            journal: path,
        }
    }
    fn lock_state(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }
    pub fn status(&self) -> Status {
        self.lock_state().status.clone()
    }
    fn persist(&self) -> Result<(), String> {
        let data = serde_json::to_vec(&self.lock_state().journal).map_err(|e| e.to_string())?;
        let temp = self.journal.with_extension("tmp");
        std::fs::write(&temp, data)
            .and_then(|()| std::fs::rename(temp, &self.journal))
            .map_err(|e| e.to_string())
    }
    fn fail(&self, error: &str) {
        let mut i = self.lock_state();
        i.status.phase = "error".into();
        i.status.message = error.into();
        i.status.prompt = false;
        i.consent = None;
    }
}
#[tauri::command]
pub async fn check_updates(
    app: tauri::AppHandle,
    updates: State<'_, Arc<Updates>>,
) -> Result<(), String> {
    check(&app, &updates).await
}
async fn check(app: &tauri::AppHandle, updates: &Updates) -> Result<(), String> {
    if !can_install_here() {
        let mut i = updates.lock_state();
        i.update = None;
        i.consent = None;
        i.status = Status {
            phase: "package_manager".into(),
            message: PACKAGE_MANAGER_GUIDANCE.into(),
            ..Status::default()
        };
        return Ok(());
    }

    let _guard = updates
        .operation
        .try_lock()
        .map_err(|_| "An update operation is already in progress")?;
    {
        let mut i = updates.lock_state();
        i.status.phase = "checking".into();
        i.status.message = "Checking for updates…".into();
        i.journal.last_check = now();
    }
    if let Err(error) = updates.persist() {
        updates.fail(&error);
        return Err(error);
    }
    let result: Result<(), String> = async {
        let update = app
            .updater_builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| e.to_string())?
            .check()
            .await
            .map_err(|_| "Couldn’t check for updates. Check your connection and try again. Updates may not be published yet.".to_string())?;
        if let Some(update) = &update {
            validate_source(update)?;
        }
        let mut i = updates.lock_state();
        i.consent = None;
        i.status = if let Some(update) = &update {
            Status {
                phase: "available".into(),
                version: Some(update.version.clone()),
                message: format!("notdot {} is available", update.version),
                prompt: i.journal.deferred.as_deref() != Some(&update.version),
                ..Status::default()
            }
        } else {
            Status {
                phase: "current".into(),
                message: "You have the latest published version".into(),
                ..Status::default()
            }
        };
        i.update = update;
        Ok::<(), String>(())
    }
    .await;
    if let Err(e) = &result {
        updates.fail(e);
    }
    result
}
fn validate_source(update: &Update) -> Result<(), String> {
    validate_package_url(
        &update.download_url,
        &update.version,
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}
fn validate_package_url(
    url: &reqwest::Url,
    version: &str,
    os: &str,
    arch: &str,
) -> Result<(), String> {
    let (platform, suffix) = match os {
        "macos" => ("darwin", ".app.tar.gz"),
        "windows" => ("windows", "-setup.exe"),
        "linux" => ("linux", ".AppImage"),
        _ => return Err("Updates are not supported on this operating system".into()),
    };
    let expected = format!(
        "/kabudu/notdot/releases/download/v{version}/notdot_{version}_{platform}-{arch}{suffix}"
    );
    if version.contains('-')
        || version.contains('+')
        || url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != expected
    {
        return Err("Update package does not match this stable notdot version and computer's release source".into());
    }
    Ok(())
}
#[tauri::command]
pub async fn defer_update(updates: State<'_, Arc<Updates>>) -> Result<(), String> {
    let _guard = updates
        .operation
        .try_lock()
        .map_err(|_| "An update operation is in progress")?;
    {
        let mut i = updates.lock_state();
        i.status.prompt = false;
        i.consent = None;
        i.journal.deferred = i.status.version.clone();
    }
    updates.persist()
}
#[tauri::command]
pub async fn announce_update(
    state: State<'_, Arc<AppState>>,
    updates: State<'_, Arc<Updates>>,
) -> Result<String, String> {
    if state.wake.status().phase == "capturing" {
        return Err("Wait for the current voice request".into());
    }
    if state.snapshot.lock().await.active_turn.is_some() {
        return Err("Wait for the current response before updating".into());
    }
    let version = {
        let mut i = updates.lock_state();
        if i.status.phase != "available" {
            return Err("No update is available".into());
        }
        i.status.prompt = false;
        i.status.version.clone().ok_or("No update version")?
    };
    state.wake.ready(false);
    // Fixed local speech works even when the paid TTS provider has run out of quota.
    super::system_speech::speak(
        "A notdot update is available. May I install it and restart? Say install update, or later.",
    )
    .await?;
    updates.lock_state().consent =
        Some((version.clone(), Instant::now() + Duration::from_secs(30)));
    Ok(version)
}
#[tauri::command]
pub async fn answer_update(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    updates: State<'_, Arc<Updates>>,
    text: String,
    version: String,
) -> Result<bool, String> {
    let authorized = {
        let mut i = updates.lock_state();
        let consent = i.consent.take();
        consent.is_some_and(|(v, deadline)| v == version && Instant::now() <= deadline)
            && i.status.version.as_deref() == Some(version.as_str())
            && i.status.phase == "available"
    };
    if !authorized {
        return Err("Update confirmation expired. Ask again from Settings.".into());
    }
    match parse(&text) {
        Answer::Install => {
            install(&app, &state, &updates).await?;
            Ok(true)
        }
        Answer::Later | Answer::Unknown => {
            defer_update(updates).await?;
            Ok(false)
        }
    }
}
#[tauri::command]
pub async fn install_update(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    updates: State<'_, Arc<Updates>>,
) -> Result<(), String> {
    install(&app, &state, &updates).await
}
async fn install(
    app: &tauri::AppHandle,
    state: &Arc<AppState>,
    updates: &Updates,
) -> Result<(), String> {
    if !can_install_here() {
        return Err(PACKAGE_MANAGER_GUIDANCE.into());
    }
    if super::is_qa(app) {
        return Err("Live update installation is disabled in the isolated QA profile; use signed local fixtures".into());
    }
    let _operation = updates
        .operation
        .try_lock()
        .map_err(|_| "An update operation is already in progress")?;
    let _turn = state
        .send_gate
        .try_lock()
        .map_err(|_| "Wait for the active submission before updating")?;
    let _voice = state
        .voice_gate
        .try_acquire_many(4)
        .map_err(|_| "Wait for voice processing before updating")?;
    {
        let s = state.snapshot.lock().await;
        if s.active_turn.is_some() {
            return Err("Wait for the current response before updating".into());
        }
    }
    let update = {
        let i = updates.lock_state();
        if i.status.phase != "available" {
            return Err("Check for an available update first".into());
        }
        i.update.clone().ok_or("Check for an update first")?
    };
    validate_source(&update)?;
    {
        let mut i = updates.lock_state();
        i.status.phase = "downloading".into();
        i.status.received = 0;
        i.status.prompt = false;
        i.consent = None;
    }
    state.snapshot.lock().await.phase = "updating".into();
    let result: Result<(), String> = async {
        let (overflow, mut exceeded) = tokio::sync::watch::channel(false);
        let mut received = 0u64;
        let download = update.download(|chunk, total| {
            received = received.saturating_add(chunk as u64);
            if received > MAX_DOWNLOAD || total.is_some_and(|n| n > MAX_DOWNLOAD) { let _ = overflow.send(true); }
            let mut i = updates.lock_state(); i.status.received = received; i.status.total = total; i.status.message = "Downloading update…".into();
        }, || {});
        let bytes = tokio::select! {
            result = tokio::time::timeout(Duration::from_secs(300), download) => result.map_err(|_| "Update download timed out")?.map_err(|e| e.to_string())?,
            _ = exceeded.changed() => return Err("Update exceeds the 256 MB download budget".into()),
        };
        if bytes.len() as u64 > MAX_DOWNLOAD { return Err("Update exceeds the download budget".into()); }
        { let mut i = updates.lock_state(); i.status.phase = "installing".into(); i.status.message = "Installing update and restarting…".into(); }
        // Close our only writer before replacing files. The desktop viewer is independent.
        if let Some(bridge) = state.bridge.lock().await.take() { bridge.shutdown().await; }
        let result = tokio::task::spawn_blocking(move || update.install(bytes)).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string());
        result?;
        app.restart();
    }.await;
    if let Err(e) = &result {
        updates.fail(e);
        state.snapshot.lock().await.phase = "idle".into();
        let disconnected = state.bridge.lock().await.is_none();
        if disconnected {
            // Release the submission gate before reconnecting after an install failure.
            drop(_turn);
            let _ = super::connect(state.clone(), false).await;
        }
    }
    result
}
pub fn spawn(app: tauri::AppHandle, state: Arc<AppState>, updates: Arc<Updates>) {
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let enabled = state.snapshot.lock().await.settings.automatic_updates;
            let due = {
                let i = updates.lock_state();
                now() < i.journal.last_check || now().saturating_sub(i.journal.last_check) >= PERIOD
            };
            if enabled && due {
                let _ = check(&app, &updates).await;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn linux_package_installs_never_offer_appimage_replacement() {
        use tauri::utils::config::BundleType;
        assert!(!super::supports_in_app_install(
            "linux",
            Some(BundleType::Deb)
        ));
        assert!(!super::supports_in_app_install(
            "linux",
            Some(BundleType::Rpm)
        ));
        assert!(!super::supports_in_app_install("linux", None));
        assert!(super::supports_in_app_install(
            "linux",
            Some(BundleType::AppImage)
        ));
        assert!(super::supports_in_app_install("macos", None));
        assert!(super::supports_in_app_install("windows", None));
    }

    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    /// Exercise the real Tauri updater HTTP and signature verifier with a disposable fixture.
    /// This deliberately never invokes install or touches an installed app.
    #[tokio::test]
    async fn real_updater_rejects_tampering_and_version_replay() {
        for (version, tampered, succeeds) in [
            ("9.9.9", false, true),
            ("9.9.9", true, false),
            ("9.9.10", false, false),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let manifest = serde_json::json!({"version":version,"url":format!("http://{address}/artifact"),"signature":include_str!("../tests/fixtures/update.txt.sig").trim()}).to_string();
            let body = if tampered {
                b"tampered payload".to_vec()
            } else {
                include_bytes!("../tests/fixtures/update.txt").to_vec()
            };
            let server = tokio::spawn(async move {
                for response in [manifest.into_bytes(), body] {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let mut request = [0; 8192];
                    let count = stream.read(&mut request).await.unwrap();
                    assert!(count > 0);
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        response.len()
                    );
                    stream.write_all(header.as_bytes()).await.unwrap();
                    stream.write_all(&response).await.unwrap();
                }
            });
            let mut context = tauri::test::mock_context(tauri::test::noop_assets());
            context.config_mut().plugins.0.insert("updater".into(), serde_json::json!({"pubkey":include_str!("../tests/fixtures/updater.pub").trim(),"dangerousInsecureTransportProtocol":true,"requireSignedVersion":true}));
            let app = tauri::test::mock_builder()
                .plugin(tauri_plugin_updater::Builder::new().build())
                .build(context)
                .unwrap();
            let update = app
                .updater_builder()
                .timeout(Duration::from_secs(3))
                .endpoints(vec![format!("http://{address}/manifest").parse().unwrap()])
                .unwrap()
                .build()
                .unwrap()
                .check()
                .await
                .unwrap()
                .unwrap();
            let result = update.download(|_, _| {}, || {}).await;
            assert_eq!(
                result.is_ok(),
                succeeds,
                "version {version}, tampered={tampered}"
            );
            server.await.unwrap();
        }
    }
    #[test]
    fn package_source_must_match_version_and_computer() {
        let address = "https://github.com/kabudu/notdot/releases/download/v1.2.3/notdot_1.2.3_windows-x86_64-setup.exe";
        let url = address.parse().unwrap();
        assert!(validate_package_url(&url, "1.2.3", "windows", "x86_64").is_ok());
        assert!(validate_package_url(&url, "1.2.3", "macos", "aarch64").is_err());
        assert!(validate_package_url(&url, "1.2.4", "windows", "x86_64").is_err());
        for address in [
            address.replace("https:", "http:"),
            address.replace("github.com", "example.com"),
            format!("{address}?token=x"),
            address.replace("github.com", "user@github.com"),
        ] {
            assert!(
                validate_package_url(&address.parse().unwrap(), "1.2.3", "windows", "x86_64")
                    .is_err()
            );
        }
    }
    #[test]
    fn deferred_version_and_polling_time_survive_restart() {
        let path =
            std::env::temp_dir().join(format!("notdot-update-test-{}.json", std::process::id()));
        let updates = Updates::new(path.clone());
        {
            let mut i = updates.lock_state();
            i.journal.last_check = 12345;
            i.journal.deferred = Some("1.2.3".into());
        }
        updates.persist().unwrap();
        let restored = Updates::new(path.clone());
        assert_eq!(restored.lock_state().journal.last_check, 12345);
        assert_eq!(
            restored.lock_state().journal.deferred.as_deref(),
            Some("1.2.3")
        );
        std::fs::remove_file(path).unwrap();
    }
}
