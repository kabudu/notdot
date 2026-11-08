//! Opt-in live Codex reporting test with synthetic, tool-free tasks.
//! Creates then archives only its own disposable chats. Requires Codex sign-in.
use notdot_core::{
    bridge::Bridge,
    session_control::{Action, Consent, Decision, Mode, Plan},
    session_runtime::SessionRuntime,
    task_awareness::Outcome,
};
use serde_json::json;
use std::{path::Path, time::Duration};
#[tokio::main]
async fn main() -> Result<(), String> {
    let arg = std::env::args()
        .nth(1)
        .ok_or("Provide the signed-in Codex executable path to run live tests")?;
    let only = std::env::args().nth(2);
    let reader = Bridge::connect_for_session_control(Path::new(&arg)).await?;
    let runtime = SessionRuntime::connect(Path::new(&arg)).await?;
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let cases = [
        (
            "Draft",
            Outcome::Success,
            "Write a two-line poem about a blue scarf. Check that it has two lines. This task is only to prepare a draft, not publish it. Do not use tools.",
        ),
        (
            "Research",
            Outcome::Partial,
            "Prepare a comparison of three synthetic shops using only these supplied facts: A costs 3 pounds, B costs 4 pounds, C has no available price. The requested comparison needs all three prices. Compare what is known and leave C explicitly unresolved. Do not use tools.",
        ),
        (
            "Delivery",
            Outcome::Failed,
            "Assess this synthetic delivery task. The requested outcome was to deliver a document. The delivery receipt says rejected: invalid destination, document not delivered. Report the underlying delivery outcome, not whether you wrote a summary. Do not retry or use tools.",
        ),
        (
            "Booking",
            Outcome::Unverified,
            "Assess this synthetic booking task. The requested outcome was a confirmed appointment. There is no receipt and no evidence either of success or failure. Report the underlying booking outcome, not whether you wrote a summary. Keep unknown distinct from failure. Do not book anything or use tools.",
        ),
    ];
    let mut chats = Vec::new();
    let result=async {
        for (name,expected,message) in cases {
            if only.as_deref().is_some_and(|filter| filter != name) {continue;}
            let config=json!({"model_reasoning_effort":"medium","project_doc_max_bytes":0,"features":{"shell_tool":false,"apps":false,"js_repl":false,"code_mode":false,"code_mode_host":false,"multi_agent":false,"browser_use":false,"computer_use":false,"hooks":false}});
            let thread=reader.call("thread/start",json!({"cwd":dir.path(),"model":"gpt-6.1-terra","sandbox":"read-only","approvalPolicy":"untrusted","config":config})).await?;
            let id=thread["thread"]["id"].as_str().ok_or("Missing chat ID")?.to_owned();
            chats.push(id.clone());
            let title=format!("notdot synthetic awareness {name} {}",&uuid::Uuid::new_v4().simple().to_string()[..8]);
            reader.call("thread/name/set",json!({"threadId":id,"name":title})).await?;
            let mut events = reader.subscribe();
            let release = reader.call("thread/unsubscribe",json!({"threadId":id})).await?;
            if release["status"] != "notLoaded" {
                tokio::time::timeout(Duration::from_secs(10), async {
                    loop { let event=events.recv().await.map_err(|e|e.to_string())?;
                        if event["method"]=="thread/closed" && event["params"]["threadId"]==id {return Ok::<(),String>(());}
                    }
                }).await.map_err(|_|"Creator did not release the synthetic chat")??;
            }
            let mut consent=Consent::default();
            consent.begin(Mode::Confirm,"Synthetic task awareness request");consent.bind("source");
            consent.stage("source",Plan{action:Action::Send,thread_id:id.clone(),title,message:message.into(),unambiguous:true})?;
            let pending=consent.pending().ok_or("No proposal")?;
            let authority=match consent.question(&pending)? {Decision::Execute(a)=>a,Decision::Ask(_)=>consent.answer(&pending,"yes send it")?.ok_or("No consent")?};
            runtime.execute_with_notifications(authority,true).await?;
            let completion=tokio::time::timeout(Duration::from_secs(120),async {
                loop { if let Some(c)=runtime.completion().await {break c;}tokio::time::sleep(Duration::from_millis(100)).await; }
            }).await.map_err(|_|"Task result timed out")?;
            println!("{name}: {:?}; {}",completion.task().outcome,completion.message());
            if completion.task().outcome!=expected {return Err(format!("Unexpected {name} outcome: expected {expected:?}"));}
            runtime.acknowledge_completion(&completion.key()).await;
        }
        Ok(())
    }.await;
    runtime.shutdown().await;
    let mut cleanup_error = None;
    for id in chats {
        if let Err(error) = reader.call("thread/archive", json!({"threadId":id})).await {
            cleanup_error = Some(format!("Could not archive a disposable chat: {error}"));
        }
    }
    reader.shutdown().await;
    if let Some(error) = cleanup_error {
        return Err(match result {
            Ok(()) => error,
            Err(original) => format!("{original}; {error}"),
        });
    }

    result
}
