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
