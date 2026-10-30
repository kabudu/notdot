//! A durable reset intent is completed before opening any ledger/runtime. Reset
//! after a restart avoids deleting an open Sled database or racing its workers.
use notdot_core::settings::Settings;
use std::{io::Write, path::Path, sync::Arc};
use tauri::{Manager, State};
const INTENT: &str = "memory-reset.pending";
pub fn pending(dir: &Path) -> bool {
    dir.join(INTENT).exists()
}
fn mark(dir: &Path) -> Result<(), String> {
    if pending(dir) {
        return Err("A memory reset is already scheduled. Restart notdot to finish it.".into());
    }
    let temp = dir.join(format!(".memory-reset-{}.tmp", uuid::Uuid::new_v4()));
    let result = || {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp).map_err(|e| e.to_string())?;
        file.write_all(b"reset-memory-v1\n")
            .and_then(|()| file.sync_all())
            .map_err(|e| e.to_string())?;
        drop(file);
        std::fs::rename(&temp, dir.join(INTENT)).map_err(|e| e.to_string())?;
        sync_directory(dir).map_err(|e|format!("Memory reset is scheduled; restart notdot to finish. Could not confirm directory sync: {e}"))
    };
    let result = result();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}
fn sync_directory(dir: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        std::fs::File::open(dir)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
    Ok(())
}
fn remove_file(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}
/// Called synchronously, before Memory::open and before starting any workers.
/// A partial failure leaves the intent in place and aborts startup, failing closed.
pub fn complete_pending(dir: &Path) -> Result<bool, String> {
    if !pending(dir) {
        return Ok(false);
    }
    let metadata = std::fs::symlink_metadata(dir.join(INTENT)).map_err(|e| e.to_string())?;
    if !metadata.is_file()
        || metadata.len() != b"reset-memory-v1\n".len() as u64
        || std::fs::read(dir.join(INTENT)).map_err(|e| e.to_string())? != b"reset-memory-v1\n"
    {
        return Err("Invalid reset request file; no data was deleted".into());
    }
    let result = || {
        let mut settings = Settings::load(&dir.join("settings.json"))?;
        settings.thread_id = None;
        settings.save(&dir.join("settings.json"))?;
        let path = dir.join("memory");
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => remove_file(&path)?,
            Ok(_) => std::fs::remove_dir_all(&path).map_err(|e| e.to_string())?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
        remove_file(&dir.join("followups.json"))?;
        sync_directory(dir)?;
        remove_file(&dir.join(INTENT))?;
        sync_directory(dir)?;
        Ok::<(), String>(())
    };
    result().map_err(|e|format!("Memory reset could not finish. Restart notdot to retry; previous context will not be loaded. {e}"))?;
    Ok(true)
}
#[tauri::command]
pub async fn reset_memory(
    app: tauri::AppHandle,
    state: State<'_, Arc<super::AppState>>,
    confirmed: bool,
) -> Result<(), String> {
    if !confirmed {
        return Err("Confirm the memory reset in Settings first".into());
    }
    let updates = app.state::<Arc<super::updates::Updates>>().status();
    if matches!(updates.phase.as_str(), "downloading" | "installing") {
        return Err("Wait for the app update before resetting memory".into());
    }
    let _submission = state
        .send_gate
        .try_lock()
        .map_err(|_| "Wait for the current operation before resetting memory")?;
    let _voice = state
        .voice_gate
        .try_acquire_many(4)
        .map_err(|_| "Wait for voice processing before resetting memory")?;
    {
        let s = state.snapshot.lock().await;
        if s.active_turn.is_some()
            || matches!(
                s.phase.as_str(),
                "thinking" | "connecting" | "resetting" | "updating"
            )
            || state.wake.status().phase == "capturing"
        {
            return Err("Stop or wait for the current turn before resetting memory".into());
        }
    }
    let dir = state
        .path
        .parent()
        .ok_or("App data directory missing")?
        .to_path_buf();
    tokio::task::spawn_blocking(move || mark(&dir))
        .await
        .map_err(|e| e.to_string())??;
    state.wake.ready(false);
    state.learning.cancel();
    state.initiative.cancel();
    state.memory.set_learning_enabled(false).await;
    {
        let mut s = state.snapshot.lock().await;
        s.phase = "resetting".into();
        s.reply.clear();
        s.error = None;
    }
    // Closing the main runtime prevents an old response reaching the new session.
    state
        .generation
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    if let Some(bridge) = state.bridge.lock().await.take() {
        bridge.shutdown().await;
    }
    app.restart();
}
#[cfg(test)]
mod tests {
    use super::*;
    use notdot_core::memory::Memory;
    #[test]
    fn malformed_intents_never_delete_memory_or_detach_conversation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        Settings {
            thread_id: Some("keep".into()),
            ..Settings::default()
        }
        .save(&path.join("settings.json"))
        .unwrap();
        std::fs::create_dir(path.join("memory")).unwrap();
        std::fs::write(path.join("memory/sentinel"), "keep").unwrap();
        std::fs::write(path.join(INTENT), "unrelated file").unwrap();
        assert!(
            complete_pending(path)
                .unwrap_err()
                .contains("Invalid reset request")
        );
        assert_eq!(
            std::fs::read_to_string(path.join("memory/sentinel")).unwrap(),
            "keep"
        );
        assert_eq!(
            Settings::load(&path.join("settings.json"))
                .unwrap()
                .thread_id
                .as_deref(),
            Some("keep")
        );
        assert!(mark(path).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn reset_does_not_follow_data_symlinks_or_overwrite_an_intent_symlink() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let path = dir.path();
        Settings::default()
            .save(&path.join("settings.json"))
            .unwrap();
        std::fs::write(outside.path().join("sentinel"), "keep").unwrap();
        symlink(outside.path(), path.join("memory")).unwrap();
        symlink(outside.path().join("sentinel"), path.join("followups.json")).unwrap();
        symlink(outside.path().join("sentinel"), path.join(INTENT)).unwrap();
        assert!(mark(path).is_err());
        assert!(complete_pending(path).is_err());
        assert_eq!(
            std::fs::read_to_string(outside.path().join("sentinel")).unwrap(),
            "keep"
        );
        std::fs::remove_file(path.join(INTENT)).unwrap();
        mark(path).unwrap();
        complete_pending(path).unwrap();
        assert_eq!(
            std::fs::read_to_string(outside.path().join("sentinel")).unwrap(),
            "keep"
        );
        assert!(!path.join("memory").exists());
    }
    #[tokio::test]
    async fn reset_deletes_the_entire_store_and_followups_and_detaches_history_while_preserving_preferences()
     {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        let settings = Settings {
            name: "Ava".into(),
            hue: 195,
            voice_id: "saved-voice".into(),
            model: "account-model".into(),
            effort: "high".into(),
            thread_id: Some("old-thread".into()),
            memory_enabled: true,
            ..Settings::default()
        };
        settings.save(&path.join("settings.json")).unwrap();
        let memory = Memory::open_with_learning(&path.join("memory"), true).unwrap();
        let note = memory
            .remember("Private synthetic preference", "old-thread")
            .await
            .unwrap();
        memory.maintain().await.unwrap();
        memory.forget(&note).await.unwrap();
        drop(memory);
        std::fs::write(
            path.join("followups.json"),
            r#"{"entries":[{"message":"old follow-up"}]}"#,
        )
        .unwrap();
        // Keep an unrelated sentinel to prove the reset scope is restricted.
        std::fs::write(path.join("voice-usage.json"), "keep").unwrap();
        mark(path).unwrap();
        assert!(pending(path));
        assert!(complete_pending(path).unwrap());
        assert!(!pending(path));
        assert!(!path.join("memory").exists());
        assert!(!path.join("followups.json").exists());
        assert_eq!(
            std::fs::read_to_string(path.join("voice-usage.json")).unwrap(),
            "keep"
        );
        let after = Settings::load(&path.join("settings.json")).unwrap();
        assert!(after.thread_id.is_none());
        assert_eq!(after.name, settings.name);
        assert_eq!(after.hue, settings.hue);
        assert_eq!(after.voice_id, settings.voice_id);
        assert_eq!(after.selection(), settings.selection());
        assert!(after.memory_enabled);
        let mut expected = serde_json::to_value(&settings).unwrap();
        expected["thread_id"] = serde_json::Value::Null;
        assert_eq!(serde_json::to_value(&after).unwrap(), expected);
        let fresh = Memory::open_with_learning(&path.join("memory"), true).unwrap();
        assert!(fresh.review("").await.unwrap().is_empty());
        assert!(fresh.recall("preference", 10).await.unwrap().is_empty());
        fresh.remember("New slate", "new-thread").await.unwrap();
        assert_eq!(fresh.review("").await.unwrap().len(), 1);
        assert!(!complete_pending(path).unwrap());
    }
    #[test]
    fn interrupted_reset_is_retried_and_does_not_clear_the_intent_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        Settings {
            thread_id: Some("old".into()),
            ..Settings::default()
        }
        .save(&path.join("settings.json"))
        .unwrap();
        mark(path).unwrap();
        std::fs::create_dir(path.join("followups.json")).unwrap();
        assert!(complete_pending(path).is_err());
        assert!(pending(path));
        assert!(
            Settings::load(&path.join("settings.json"))
                .unwrap()
                .thread_id
                .is_none()
        );
        std::fs::remove_dir(path.join("followups.json")).unwrap();
        assert!(complete_pending(path).unwrap());
        assert!(!pending(path));
    }
}
