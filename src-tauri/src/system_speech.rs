use std::time::Duration;
pub async fn speak(text: &str) -> Result<(), String> {
    static SPEECH: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _guard = SPEECH.try_lock().map_err(|_| "System voice is busy")?;
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = tokio::process::Command::new("/usr/bin/say");
        c.arg(text);
        c
    };
    #[cfg(windows)]
    let mut command = {
        let mut c = tokio::process::Command::new("powershell.exe");
        c.args(["-NoProfile", "-NonInteractive", "-Command", "Add-Type -AssemblyName System.Speech; $s=New-Object System.Speech.Synthesis.SpeechSynthesizer; $s.Speak($env:NOTDOT_SYSTEM_SPEECH)"]).env("NOTDOT_SYSTEM_SPEECH", text).creation_flags(0x08000000);
        c
    };
    #[cfg(target_os = "linux")]
    let mut command = {
        let mut c = tokio::process::Command::new("spd-say");
        c.args(["--wait", "--", text]);
        c
    };
    command
        .kill_on_drop(true)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let status = tokio::time::timeout(Duration::from_secs(25), command.status())
        .await
        .map_err(|_| "System voice timed out")?
        .map_err(|e| format!("System voice unavailable: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("System voice could not play the alert".into())
    }
}
