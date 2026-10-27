use std::time::Duration;
pub async fn speak(text: &str) -> Result<(), String> {
    static SPEECH: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _guard = SPEECH.try_lock().map_err(|_| "System voice is busy")?;
    let mut command = speech_command(text);
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

fn speech_command(text: &str) -> tokio::process::Command {
    #[cfg(target_os = "macos")]
    let command = {
        let mut c = tokio::process::Command::new("/usr/bin/say");
        c.arg("--").arg(text);
        c
    };
    #[cfg(windows)]
    let command = {
        let mut c = tokio::process::Command::new("powershell.exe");
        c.args(["-NoProfile", "-NonInteractive", "-Command", "Add-Type -AssemblyName System.Speech; $s=New-Object System.Speech.Synthesis.SpeechSynthesizer; $s.Speak($env:NOTDOT_SYSTEM_SPEECH)"]).env("NOTDOT_SYSTEM_SPEECH", text).creation_flags(0x08000000);
        c
    };
    #[cfg(target_os = "linux")]
    let command = {
        let mut c = tokio::process::Command::new("spd-say");
        c.args(["--wait", "--", text]);
        c
    };
    command
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    fn reminder_text_cannot_be_interpreted_as_speech_program_options() {
        let command = super::speech_command("-f/private/synthetic-secret");
        let args: Vec<_> = command
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, ["--", "-f/private/synthetic-secret"]);
    }
}
