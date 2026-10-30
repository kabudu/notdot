use notdot_core::settings::Startup;
use std::path::PathBuf;
use tauri_plugin_autostart::ManagerExt;

pub fn configure(app: &tauri::AppHandle, mode: Startup) -> Result<(), String> {
    if super::is_qa(app) {
        return if mode == Startup::Manual {
            Ok(())
        } else {
            Err("OS startup changes are disabled in the isolated QA profile".into())
        };
    }
    let manager = app.autolaunch();
    let enabled = manager.is_enabled().map_err(|e| e.to_string())?;
    match (mode == Startup::Manual, enabled) {
        (true, true) => manager.disable(),
        (false, false) => manager.enable(),
        _ => Ok(()),
    }
    .map_err(|e| format!("Could not change sign-in startup: {e}"))
}
pub fn runtime() -> PathBuf {
    if let Some(path) = std::env::var_os("NOTDOT_CODEX_BIN") {
        return path.into();
    }
    #[cfg(windows)]
    if let Some(root) = std::env::var_os("LOCALAPPDATA")
        && let Some(path) = windows_standalone_runtime(std::path::Path::new(&root))
    {
        return path;
    }
    #[cfg(target_os = "macos")]
    for root in ["/Applications/ChatGPT.app", "/Applications/Codex.app"] {
        for suffix in [
            "Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex",
            "Contents/Resources/codex",
        ] {
            let path = PathBuf::from(root).join(suffix);
            if path.is_file() {
                return path;
            }
        }
    }
    // GUI launches need not inherit the user's interactive shell PATH.
    if let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) {
        for dir in [".local/bin", ".cargo/bin", ".npm-global/bin"] {
            let path = PathBuf::from(&home).join(dir).join(if cfg!(windows) {
                "codex.exe"
            } else {
                "codex"
            });
            if path.is_file() {
                return path;
            }
        }
    }
    #[cfg(unix)]
    for root in [
        "/opt/homebrew/bin/codex",
        "/usr/local/bin/codex",
        "/usr/bin/codex",
    ] {
        let path = PathBuf::from(root);
        if path.is_file() {
            return path;
        }
    }
    PathBuf::from(if cfg!(windows) { "codex.exe" } else { "codex" })
}
#[cfg(any(windows, test))]
fn windows_standalone_runtime(local_appdata: &std::path::Path) -> Option<PathBuf> {
    let path = local_appdata
        .join("Programs")
        .join("OpenAI")
        .join("Codex")
        .join("bin")
        .join("codex.exe");
    path.is_file().then_some(path)
}
pub fn desktop_running() -> bool {
    let mut system = sysinfo::System::new();
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::All,
        true,
        sysinfo::ProcessRefreshKind::nothing().with_exe(sysinfo::UpdateKind::OnlyIfNotSet),
    );
    system.processes().values().any(|p| {
        let name = p.name().to_string_lossy().to_lowercase();
        if !matches!(
            name.as_str(),
            "chatgpt" | "chatgpt.exe" | "codex" | "codex.exe"
        ) {
            return false;
        }
        let path = p
            .exe()
            .map(|p| p.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        // Exclude the headless runtime spawned by this app and terminal CLI sessions.
        path.contains("chatgpt.app/contents/macos/")
            || path.contains("codex.app/contents/macos/")
            || name.starts_with("chatgpt")
            || path.contains("windowsapps")
    })
}
pub async fn open_desktop() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let path = ["/Applications/ChatGPT.app", "/Applications/Codex.app"]
            .into_iter()
            .find(|p| std::path::Path::new(p).is_dir())
            .ok_or("Desktop app was not found in Applications")?;
        let status = tokio::process::Command::new("/usr/bin/open")
            .args(["-g", path])
            .status()
            .await
            .map_err(|e| e.to_string())?;
        if !status.success() {
            return Err("Could not open the desktop app in the background".into());
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    Err("Automatic background desktop launch is currently available on macOS; the Codex runtime works independently on this platform".into())
}

#[cfg(test)]
mod runtime_tests {
    #[test]
    fn gui_launch_discovers_standalone_native_runtime_without_shell_path_or_cmd_shim() {
        let temp = tempfile::tempdir().unwrap();
        assert!(super::windows_standalone_runtime(temp.path()).is_none());
        let bin = temp.path().join("Programs/OpenAI/Codex/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("codex.cmd"), "synthetic shim").unwrap();
        assert!(super::windows_standalone_runtime(temp.path()).is_none());
        let exe = bin.join("codex.exe");
        std::fs::write(&exe, "synthetic native runtime").unwrap();
        assert_eq!(super::windows_standalone_runtime(temp.path()), Some(exe));
    }
}
