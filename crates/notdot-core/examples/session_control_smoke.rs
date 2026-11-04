//! Explicit authenticated smoke test. Creates and archives synthetic chats only.
use notdot_core::{
    bridge::Bridge,
    session_control::{Action, Authorised, Consent, Decision, Mode, Plan},
    session_runtime::SessionRuntime,
};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
fn approve(consent: &mut Consent, plan: Plan) -> Authorised {
    consent.begin(Mode::Confirm, "Synthetic current user request");
    consent.bind("synthetic-turn");
    let stop = plan.action == Action::Stop;
    consent.stage("synthetic-turn", plan).unwrap();
    let id = consent.pending().unwrap();
    match consent.question(&id).unwrap() {
        Decision::Execute(authority) => authority,
        Decision::Ask(_) => consent
            .answer(&id, if stop { "yes stop it" } else { "yes send it" })
            .unwrap()
            .unwrap(),
    }
}
async fn latest(reader: &Bridge, tid: &str) -> Value {
    reader
        .call(
            "thread/turns/list",
            json!({"threadId":tid,"limit":1,"itemsView":"full","sortDirection":"desc"}),
        )
        .await
        .unwrap()
}
#[tokio::main]
async fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    let runtime = Path::new(
        args.get(1)
            .expect("usage: session_control_smoke CODEX_RUNTIME NOTDOT_EXECUTABLE"),
    );
    let helper = Path::new(args.get(2).expect("provide notdot executable"))
        .canonicalize()
        .expect("notdot executable must exist");
    let target = Bridge::connect_for_session_control(runtime).await.unwrap();
    let source = Bridge::connect_with_chat_tools(runtime, &helper)
        .await
        .unwrap();
    let session = SessionRuntime::connect(runtime).await.unwrap();
    let mut session_events = session.subscribe();
    let diagnostics = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let record = diagnostics.clone();
    tokio::spawn(async move {
        while let Ok(e) = session_events.recv().await {
            if matches!(
                e["method"].as_str(),
                Some(
                    "turn/started"
                        | "turn/completed"
                        | "thread/status/changed"
                        | "thread/closed"
                        | "notdot/disconnected"
                )
            ) {
                let mut r = record.lock().await;
                if r.len() < 60 {
                    r.push(json!({"method":e["method"],"thread":e["params"]["threadId"],"turn":e["params"]["turn"]["id"],"status":e["params"]["turn"]["status"],"thread_status":e["params"]["status"]}));
                }
            }
        }
    });

    let dir = tempfile::tempdir().unwrap();
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let title = format!("notdot synthetic control {}", &nonce[..8]);
    let marker = format!("CONTROL{}", &nonce[..12]);
    let config = json!({"model_reasoning_effort":"medium","project_doc_max_bytes":0,"features":{"shell_tool":false,"apps":false,"js_repl":false,"code_mode":false,"code_mode_host":false,"multi_agent":false,"browser_use":false,"computer_use":false,"hooks":false}});
    let created=target.call("thread/start",json!({"cwd":dir.path(),"model":"gpt-6.1-terra","sandbox":"read-only","approvalPolicy":"untrusted","config":config})).await.unwrap();
    let tid = created["thread"]["id"].as_str().unwrap().to_owned();
    // A saved session needs a completed turn to appear in the local history index.
    let mut seed_events = target.subscribe();
    let seeded = target
        .send(
            &tid,
            &format!("This is the synthetic chat {title}. Reply ready only. Do not use tools."),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let event = seed_events.recv().await.unwrap();
            if event["method"] == "turn/completed" && event["params"]["turn"]["id"] == seeded {
                break;
            }
        }
    })
    .await
    .unwrap();
    target
        .call("thread/name/set", json!({"threadId":tid,"name":title}))
        .await
        .unwrap();
    target
        .call("thread/unsubscribe", json!({"threadId":tid}))
        .await
        .unwrap();
    let sid = source
        .thread(None, dir.path(), "Synthetic session controller")
        .await
        .unwrap();
    let mut events = source.subscribe();
    let message = format!("Reply with {marker} only.");
    let mut consent = Consent::default();
    consent.begin(Mode::Confirm, &format!("Tell {title} to {message}"));
    let turn=source.send(&sid,&format!("Find the local Codex chat titled {title:?}, then use prepare_chat_action to prepare a send to that exact chat with message {message:?}. Do not execute it or claim it was sent. Do not use other tools.")).await.unwrap();
    consent.bind(&turn);
    let planned = tokio::time::timeout(Duration::from_secs(90), async {
        let mut staged = false;
        loop {
            let event = events.recv().await.unwrap();
            if let Some(plan) = Plan::from_tool_event(&event, &sid, &turn).unwrap() {
                consent.stage(&turn, plan).unwrap();
                staged = true;
            }
            if event["method"] == "turn/completed" && event["params"]["turn"]["id"] == turn {
                break staged;
            }
        }
    })
    .await
    .unwrap();
    assert!(planned, "model did not prepare the action");
    assert!(
        latest(&target, &tid).await["data"][0]["id"] == seeded,
        "preparation must not post"
    );
    let pending = consent.pending().unwrap();
    assert!(matches!(
        consent.question(&pending).unwrap(),
        Decision::Ask(_)
    ));
    let authority = consent.answer(&pending, "yes send it").unwrap().unwrap();
    assert!(
        session
            .execute(authority)
            .await
            .unwrap()
            .starts_with("Sent to")
    );
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let page = latest(&target, &tid).await;
            if page["data"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|t| t["items"].as_array().into_iter().flatten())
                .any(|i| {
                    i["type"] == "agentMessage"
                        && i["text"].as_str().is_some_and(|s| s.contains(&marker))
                })
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    })
    .await
    .unwrap();
    let plan = |action, message: &str| Plan {
        action,
        thread_id: tid.clone(),
        title: title.clone(),
        unambiguous: true,
        message: message.into(),
    };
    // Wait for the completion event, rather than confusing an accepted send with completion.
    for _ in 0..100 {
        let status = session
            .execute(approve(&mut consent, plan(Action::Status, "")))
            .await
            .unwrap();
        if status.contains("completed") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    let sent = session
        .execute(approve(
            &mut consent,
            plan(
                Action::Send,
                "Explain compiler design in two thousand words.",
            ),
        ))
        .await
        .unwrap();
    assert!(sent.starts_with("Sent to"));
    assert!(
        session
            .execute(approve(
                &mut consent,
                plan(Action::Steer, "Use short paragraphs.")
            ))
            .await
            .unwrap()
            .starts_with("Added")
    );
    let stopped = session
        .execute(approve(&mut consent, plan(Action::Stop, "")))
        .await;
    assert!(
        stopped
            .as_ref()
            .is_ok_and(|s| s.starts_with("Stop requested") || s.contains("No stop was needed"))
            || stopped.as_ref().is_err_and(|s| s.contains("does not own")),
        "unexpected stop result: {stopped:?}"
    );
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let status = session
                .execute(approve(&mut consent, plan(Action::Status, "")))
                .await
                .unwrap();
            if !status.contains("still running") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "Lifecycle wait failed: {}",
            diagnostics
                .try_lock()
                .map(|d| json!(*d).to_string())
                .unwrap_or_default()
        )
    });
    // Check stop separately from steering: a steered task may finish before a later stop.
    assert!(
        session
            .execute(approve(
                &mut consent,
                plan(
                    Action::Send,
                    "Write a six thousand word history of compiler design."
                )
            ))
            .await
            .unwrap()
            .starts_with("Sent to")
    );
    assert!(
        session
            .execute(approve(&mut consent, plan(Action::Stop, "")))
            .await
            .unwrap()
            .starts_with("Stop requested")
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if latest(&target, &tid).await["data"][0]["status"] == "interrupted" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    session.shutdown().await;
    source
        .call("thread/unsubscribe", json!({"threadId":sid}))
        .await
        .unwrap();
    source
        .call("thread/archive", json!({"threadId":sid}))
        .await
        .unwrap();
    target
        .call("thread/archive", json!({"threadId":tid}))
        .await
        .unwrap();
    source.shutdown().await;
    target.shutdown().await;
    println!(
        "{}",
        json!({"status":"passed","model":"gpt-6.1-terra","effort":"medium","model_prepared_action":true,"no_post_before_confirmation":true,"confirmed_send_and_reply":true,"steer":true,"stop":true,"scope":"Authenticated synthetic chat control; no microphone, speaker or voice keys"})
    );
}
