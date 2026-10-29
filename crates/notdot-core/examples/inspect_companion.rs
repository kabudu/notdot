//! Read-only native acceptance probe; never resumes a chat or starts a turn.
use notdot_core::{bridge::Bridge, settings::Settings};
use serde_json::json;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let runtime = args
        .next()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("LOCALAPPDATA")
                .map(|p| PathBuf::from(p).join("Programs/OpenAI/Codex/bin/codex.exe"))
        })
        .ok_or("Pass the native Codex executable and notdot settings path")?;
    let path = args
        .next()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("APPDATA")
                .map(|p| PathBuf::from(p).join("com.kabudu.notdot/settings.json"))
        })
        .ok_or("Pass the notdot settings path")?;
    let settings = Settings::load(&path)?;
    let id = settings.thread_id.ok_or("No companion chat is saved")?;
    let bridge = Bridge::connect(&runtime).await?;
    let result = bridge
        .call("thread/read", json!({"threadId":id,"includeTurns":true}))
        .await;
    bridge.shutdown().await;
    let reply = result?;
    let turns = reply["thread"]["turns"]
        .as_array()
        .ok_or("Runtime omitted turns")?;
    println!(
        "Saved model: {}; effort: {}; turns: {}",
        settings.model,
        settings.effort,
        turns.len()
    );
    for turn in turns.iter().skip(turns.len().saturating_sub(2)) {
        println!("Turn status: {}", turn["status"]);
        if let Some(items) = turn["items"].as_array() {
            for item in items {
                match item["type"].as_str() {
                    Some("userMessage") => {
                        if let Some(content) = item["content"].as_array() {
                            for part in content {
                                if let Some(text) = part["text"].as_str() {
                                    println!(
                                        "Request: {}",
                                        text.chars().take(1000).collect::<String>()
                                    );
                                }
                            }
                        }
                    }
                    Some("agentMessage") => {
                        if let Some(text) = item["text"].as_str() {
                            println!("Reply: {}", text.chars().take(1000).collect::<String>());
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(())
}
