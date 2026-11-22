//! macOS desktop checks that need a logged-in session. Run with
//! `cargo test -p notdot-host --test desktop -- --ignored`.
#![cfg(target_os = "macos")]
use notdot_host::{Status, desktop};

#[test]
#[ignore = "needs a logged-in macOS session"]
fn lists_top_level_apps_and_reports_accessibility_truthfully() {
    let apps = desktop::apps().unwrap();
    assert!(
        apps.iter().any(|a| a.bundle_id == "com.apple.finder"),
        "Finder is a running app"
    );
    assert!(
        !apps
            .iter()
            .any(|a| a.bundle_id.is_empty() || a.name.is_empty())
    );
    let finder = apps
        .iter()
        .find(|a| a.bundle_id == "com.apple.finder")
        .unwrap();
    match desktop::readiness() {
        Status::Ready => assert!(desktop::observe(finder.pid).is_ok()),
        Status::SetupRequired(why) => {
            assert!(why.contains("Accessibility"));
            // Without the permission nothing is read, and nothing is pressed.
            assert!(
                desktop::observe(finder.pid)
                    .unwrap_err()
                    .contains("not allowed")
            );
            assert!(desktop::press(finder.pid, "0", "AXWindow", "x").is_err());
        }
        Status::Unsupported(why) => panic!("macOS is supported: {why}"),
    }
}

#[tokio::test]
#[ignore = "needs an unlocked macOS session with Screen Recording permission"]
async fn captures_only_the_disposable_counter_window() {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let temp = tempfile::tempdir().unwrap();
    let binary = temp.path().join("counter");
    assert!(
        Command::new("/usr/bin/swiftc")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../notdot-core/examples/fixtures/Counter.swift"
            ))
            .arg("-o")
            .arg(&binary)
            .status()
            .unwrap()
            .success()
    );
    let mut fixture = tokio::process::Command::new(binary)
        .kill_on_drop(true)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let pid = fixture.id().unwrap() as i32;
    let deadline = Instant::now() + Duration::from_secs(10);
    let window = loop {
        let windows = desktop::screenshot_windows(pid).unwrap();
        if let [window] = windows.as_slice() {
            break *window;
        }
        assert!(Instant::now() < deadline, "fixture has no visible window");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert!(desktop::screenshot(pid, u32::MAX).await.is_err());
    let jpeg = desktop::screenshot(pid, window).await.unwrap();
    let path = temp.path().join("capture.jpg");
    std::fs::write(&path, jpeg).unwrap();
    let metadata = Command::new("/usr/bin/sips")
        .args(["-g", "pixelWidth", "-g", "pixelHeight"])
        .arg(path)
        .output()
        .unwrap();
    assert!(metadata.status.success());
    let details = String::from_utf8(metadata.stdout).unwrap();
    for (key, range) in [("pixelWidth:", 320..=960), ("pixelHeight:", 140..=560)] {
        let pixels: i32 = details
            .lines()
            .find_map(|line| line.trim().strip_prefix(key))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(
            range.contains(&pixels),
            "{key} {pixels} is outside fixture bounds"
        );
    }
    fixture.kill().await.unwrap();
    assert!(desktop::screenshot(pid, window).await.is_err());
}
