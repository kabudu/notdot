//! Explicit, attended live probe of owner-only live web search. It runs ONE
//! real Codex turn on the signed-in ChatGPT account (refusing API-key
//! sign-in), through the same companion thread configuration notdot uses,
//! against a read-only public page. It changes no notdot settings and
//! archives its disposable chat afterwards. It prints no account details.
//!
//! An optional second argument replaces the request, and `PROBE_LIVE=0` sends
//! the turn without live web search (for the browser-control boundary check:
//! the owner's Browser Use and Computer Use preferences are requested too).
//!
//! Run: cargo run -p notdot-core --example live_web_search_probe -- CODEX_PATH [REQUEST]
use notdot_core::bridge::{Bridge, ThreadOptions};
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};

const REQUEST: &str = "Using web search, open https://example.com and tell me the exact main heading on that page. Name the URL you used.";

#[tokio::main]
async fn main() -> Result<(), String> {
    let runtime = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("Pass the Codex runtime path")?,
    );
    let request = std::env::args()
        .nth(2)
        .unwrap_or_else(|| REQUEST.to_owned());
    let live = std::env::var("PROBE_LIVE").map_or(true, |v| v != "0");
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let bridge = Bridge::connect(&runtime).await?;
    let account = bridge
        .call("account/read", json!({"refreshToken":false}))
        .await?;
    let kind = account["account"]["type"]
        .as_str()
        .unwrap_or("none")
        .to_owned();
    if kind != "chatgpt" {
        bridge.shutdown().await;
        return Err(format!(
            "Refusing to run: sign-in type is {kind}, not the existing ChatGPT sign-in"
        ));
    }
    let mut events = bridge.subscribe();
    let thread = bridge
        .thread_with_options(
            None,
            dir.path(),
            "notdot live web search probe",
            &notdot_core::models::Selection::default(),
            ThreadOptions {
                live_web_search: live,
                browser_use: true,
                computer_use: true,
                ..ThreadOptions::default()
            },
        )
        .await?;
    let turn = bridge.send(&thread, &request).await?;
    let mut items: Vec<Value> = Vec::new();
    let mut answer = String::new();
    let outcome = tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            let e = events.recv().await.map_err(|e| e.to_string())?;
            if e["method"] == "notdot/unsupportedRequest" {
                items.push(json!({"type":"refusedHostRequest","method":e["params"]["method"]}));
            }
            if e["method"] == "item/completed" {
                let item = &e["params"]["item"];
                let kind = item["type"].as_str().unwrap_or("").to_owned();
                match kind.as_str() {
                    "webSearch" => items
                        .push(json!({"type":kind,"query":item["query"],"action":item["action"]})),
                    "mcpToolCall" | "dynamicToolCall" => items.push(
                        json!({"type":kind,"server":item["server"],"tool":item["tool"],"status":item["status"]}),
                    ),
                    "agentMessage" => {
                        answer = item["text"].as_str().unwrap_or("").to_owned();
                        items.push(json!({"type":kind}));
                    }
                    _ => items.push(json!({"type":kind})),
                }
            }
            if e["method"] == "turn/completed" {
                return Ok::<Value, String>(e["params"]["turn"]["status"].clone());
            }
        }
    })
    .await
    .map_err(|_| "The probe turn exceeded 180 seconds".to_string());
    if outcome.is_err() {
        let _ = bridge.interrupt(&thread, &turn).await;
    }
    let archived = bridge
        .call("thread/archive", json!({"threadId":thread}))
        .await
        .is_ok();
    bridge.shutdown().await;
    let status = outcome??;
    let kinds: Vec<&str> = items.iter().filter_map(|i| i["type"].as_str()).collect();
    let only_search = kinds.iter().all(|k| {
        matches!(
            *k,
            "userMessage" | "reasoning" | "webSearch" | "agentMessage"
        )
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "signIn": kind,
            "request": request,
            "liveWebSearch": live,
            "status": status,
            "items": items,
            "answer": answer,
            "usedWebSearch": kinds.contains(&"webSearch"),
            "noOtherToolsUsed": only_search,
            "chatArchived": archived,
        }))
        .map_err(|e| e.to_string())?
    );
    Ok(())
}
