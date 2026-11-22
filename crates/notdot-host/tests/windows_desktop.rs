//! Native UI Automation smoke test in an interactive Windows session.
#![cfg(windows)]

use notdot_host::desktop;
use std::{
    process::{Child, Command},
    time::{Duration, Instant},
};

struct Fixture(Child);

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "requires an interactive Windows desktop"]
fn invokes_a_disposable_native_button() {
    let temp = tempfile::tempdir().unwrap();
    let script = temp.path().join("fixture.ps1");
    let marker = temp.path().join("pressed");
    std::fs::write(
        &script,
        r#"
param([string]$Marker)
Add-Type -AssemblyName System.Windows.Forms
$form = New-Object System.Windows.Forms.Form
$form.Text = 'notdot disposable desktop test'
$button = New-Object System.Windows.Forms.Button
$button.Text = 'Mark verified'
$button.AutoSize = $true
$button.Add_Click({ [IO.File]::WriteAllText($Marker, 'verified') }.GetNewClosure())
$form.Controls.Add($button)
[void]$form.ShowDialog()
"#,
    )
    .unwrap();
    let fixture = Fixture(
        Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-STA", "-File"])
            .arg(&script)
            .arg(&marker)
            .spawn()
            .unwrap(),
    );
    let pid = fixture.0.id() as i32;
    let deadline = Instant::now() + Duration::from_secs(15);
    let button = loop {
        if desktop::apps().unwrap().iter().any(|app| app.pid == pid)
            && let Some(button) = desktop::observe(pid)
                .unwrap()
                .into_iter()
                .find(|node| node.label == "Mark verified" && node.pressable)
        {
            break button;
        }
        assert!(
            Instant::now() < deadline,
            "fixture button was not available"
        );
        std::thread::sleep(Duration::from_millis(150));
    };
    assert!(desktop::press(pid, &button.reference, &button.role, "Changed").is_err());
    assert!(!marker.exists());
    desktop::press(pid, &button.reference, &button.role, &button.label).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if std::fs::read_to_string(&marker).ok().as_deref() == Some("verified") {
            break;
        }
        assert!(Instant::now() < deadline, "button did not update fixture");
        std::thread::sleep(Duration::from_millis(100));
    }
}
