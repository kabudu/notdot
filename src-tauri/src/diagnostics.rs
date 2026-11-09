//! Explicit export of a small allowlist, never a dump of app state or logs.
use super::AppState;
use std::sync::Arc;
use tauri::{Manager, State};
#[tauri::command]
pub async fn export_diagnostics(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<String, String> {
    let context = state.context();
    let connected = context.bridge.lock().await.is_some();
    let report = {
        let s = state.snapshot.lock().await;
        serde_json::json!({"schema":1,"version":app.package_info().version.to_string(),"platform":std::env::consts::OS,"architecture":std::env::consts::ARCH,"runtime_connected":connected,"phase":s.phase,"recognition_service":s.settings.stt_provider,"reply_service":s.settings.voice_provider,"recognition_key_available":state.credentials.cached(s.settings.transcription_id()),"reply_key_available":state.credentials.cached(&s.settings.voice_provider),"microphone_muted":s.settings.microphone_muted,"wake_enabled":s.settings.wake_enabled,"memory_enabled":s.settings.memory_enabled,"onboarding_complete":s.settings.onboarding_complete})
    };
    let bytes = serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    let path = app
        .path()
        .download_dir()
        .map_err(|_| "Downloads folder is unavailable")?
        .join(format!("notdot-diagnostics-{stamp}.json"));
    let target = path.clone();
    tokio::task::spawn_blocking(move || {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut f = options.open(target)?;
        f.write_all(&bytes)?;
        f.sync_all()
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("Could not save diagnostics: {e}"))?;
    Ok(format!(
        "Saved {}. This report contains connection and settings flags, not keys, recordings, chats or memories. Review it before sharing.",
        path.display()
    ))
}
