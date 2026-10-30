use notdot_core::{bridge::Bridge, settings::Settings};
use std::{path::PathBuf, time::Duration};
#[tokio::main]
async fn main() -> Result<(), String> {
    let root = PathBuf::from(".local/rust-smoke");
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let path = root.join("settings.json");
    let mut settings = Settings::load(&path)?;
    let binary = PathBuf::from(
        "/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex",
    );
    let bridge = Bridge::connect(&binary).await?;
    let thread = bridge
        .thread(
            settings.thread_id.as_deref(),
            &std::env::current_dir().map_err(|e| e.to_string())?,
            "notdot Rust smoke",
        )
        .await?;
    settings.thread_id = Some(thread.clone());
    settings.save(&path)?;
    let mut events = bridge.subscribe();
    let text = std::env::args()
        .nth(1)
        .unwrap_or("Reply exactly: Rust bridge is ready.".into());
    bridge.send(&thread, &text).await?;
    let result = tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let e = events.recv().await.map_err(|e| e.to_string())?;
            if e["method"] == "item/completed" && e["params"]["item"]["phase"] == "final_answer" {
                println!("{}", e["params"]["item"]["text"]);
            }
            if e["method"] == "turn/completed" {
                if e["params"]["turn"]["status"] != "completed" {
                    return Err(e.to_string());
                }
                break;
            }
        }
        Ok::<(), String>(())
    })
    .await
    .map_err(|_| "Smoke turn exceeded 90 seconds".to_string())?;
    bridge.shutdown().await;
    println!("thread={thread}");
    result
}
