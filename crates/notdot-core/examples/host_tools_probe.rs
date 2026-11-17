//! Attended discovery probe: which browser and computer tools does notdot's
//! own Codex runtime load, and does the Node REPL get a trusted browser
//! service? (Recorded with the preferences sent ungated at 63fba78, the tool
//! inventory was identical with the features on and off.) It starts no model turn (nothing is billed), uses
//! only documented app-server methods, and never connects to the person's
//! browser: the one direct tool call asks the Node REPL whether a trusted
//! browser service is present and reports the error if it is not. It prints
//! tool and server names only, with the home folder replaced by `~`.
//!
//! Run: cargo run -p notdot-core --example host_tools_probe -- CODEX_PATH
use notdot_core::bridge::{Bridge, ThreadOptions};
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};

/// Reports whether the host-provided browser access variables exist (never
/// their values) and what the trusted browser service says to a setup call.
const BROWSER_CHECK: &str = r#"const names=['BROWSER_USE_ACCESS_PIPE_PATH','BROWSER_USE_ACCESS_EXECUTOR_ID','BROWSER_USE_ACCESS_VERIFYING_IDENTITY'];
const present=Object.fromEntries(names.map(n=>[n,typeof process!=='undefined'&&!!process.env?.[n]]));
let rpc='missing',setup=null;
try{rpc=typeof nodeRepl?.rpc;if(rpc==='function'){setup=await Promise.race([nodeRepl.rpc('browser',{method:'setup',params:{environment:'iab'}}).then(()=>'ok'),new Promise(r=>setTimeout(()=>r('timeout'),8000))]);}}catch(e){setup='error: '+String(e?.message??e).slice(0,300);}
nodeRepl.write(JSON.stringify({present,rpc,setup}));"#;

fn redact(value: Value, home: &str) -> Value {
    if home.is_empty() {
        return value;
    }
    serde_json::from_str(&value.to_string().replace(home, "~")).unwrap_or(Value::Null)
}

async fn probe(bridge: &Bridge, browser_use: bool, computer_use: bool) -> Result<Value, String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let thread = bridge
        .thread_with_options(
            None,
            dir.path(),
            "notdot host tools probe",
            &notdot_core::models::Selection::default(),
            ThreadOptions {
                browser_use,
                computer_use,
                ..Default::default()
            },
        )
        .await?;
    let status = bridge
        .call(
            "mcpServerStatus/list",
            json!({"threadId":thread,"detail":"toolsAndAuthOnly"}),
        )
        .await?;
    let servers: Vec<Value> = status["data"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|s| {
            let tools: Vec<String> = match &s["tools"] {
                Value::Object(map) => map.keys().cloned().collect(),
                Value::Array(list) => list
                    .iter()
                    .filter_map(|t| t["name"].as_str().map(str::to_owned))
                    .collect(),
                _ => Vec::new(),
            };
            json!({"name":s["name"],"plugin":s["pluginId"],"status":s["runtimeStatus"],"tools":tools,"toolsError":s["toolsError"]})
        })
        .collect();
    let has_repl = servers.iter().any(|s| {
        s["name"] == "node_repl"
            && s["tools"]
                .as_array()
                .is_some_and(|t| t.iter().any(|n| n == "js"))
    });
    let browser_service = if has_repl {
        match tokio::time::timeout(
            Duration::from_secs(30),
            bridge.call(
                "mcpServer/tool/call",
                json!({"threadId":thread,"server":"node_repl","tool":"js","arguments":{"code":BROWSER_CHECK}}),
            ),
        )
        .await
        {
            Ok(Ok(r)) => r["content"].clone(),
            Ok(Err(e)) => json!(format!("error: {e}")),
            Err(_) => json!("timeout"),
        }
    } else {
        json!("node_repl js tool not loaded")
    };
    let _ = bridge
        .call("thread/archive", json!({"threadId":thread}))
        .await;
    Ok(json!({
        "browserUse": browser_use,
        "computerUse": computer_use,
        "servers": servers,
        "browserService": browser_service,
    }))
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let runtime = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("Pass the Codex runtime path")?,
    );
    let home = std::env::var("HOME").unwrap_or_default();
    let bridge = Bridge::connect(&runtime).await?;
    let mut results = Vec::new();
    for (browser, computer) in [(true, true)] {
        results.push(match probe(&bridge, browser, computer).await {
            Ok(v) => v,
            Err(e) => json!({"browserUse":browser,"computerUse":computer,"error":e}),
        });
    }
    bridge.shutdown().await;
    println!(
        "{}",
        serde_json::to_string_pretty(&redact(json!(results), &home)).map_err(|e| e.to_string())?
    );
    Ok(())
}
