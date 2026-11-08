//! Live synthetic proof: member posts a USER item without running a model turn
//! or changing the target's permissions. Archives the disposable target.
use notdot_core::{
    bridge::Bridge,
    chat_tools::ChatPolicy,
    profiles::{Actor, Admission, ChatAccess, Roster, SpeakerOutcome},
    session_control::{Action, Consent, Decision, Mode, Plan},
    session_runtime::SessionRuntime,
};
use serde_json::json;
#[tokio::main]
async fn main() -> Result<(), String> {
    let runtime = std::env::args_os().nth(1).ok_or("Pass Codex runtime")?;
    let runtime = std::path::Path::new(&runtime);
    let directory = tempfile::tempdir().map_err(|e| e.to_string())?;
    let bridge = Bridge::connect(runtime).await?;
    let started=bridge.call("thread/start",json!({"model":notdot_core::models::Selection::default().model,"cwd":directory.path(),"sandbox":"workspace-write","approvalPolicy":"untrusted","config":{"features":{"shell_tool":false,"apps":false,"remote_plugin":false,"browser_use":false,"computer_use":false}}})).await?;
    let thread = started["thread"]["id"]
        .as_str()
        .ok_or("No thread")?
        .to_owned();
    let result=async{
        bridge.call("thread/name/set",json!({"threadId":thread,"name":"notdot synthetic member posting proof"})).await?;
        let mut setup_events=bridge.subscribe();
        let setup=bridge.call("turn/start",json!({"threadId":thread,"input":[{"type":"text","text":"Synthetic setup only: reply with the single word READY. Do not use tools."}]})).await?;
        let setup_turn=setup["turn"]["id"].as_str().ok_or("No setup turn")?;
        tokio::time::timeout(std::time::Duration::from_secs(90),async{loop{let event=setup_events.recv().await.map_err(|e|e.to_string())?;if event["method"]=="turn/completed"&&event["params"]["turn"]["id"]==setup_turn {if event["params"]["turn"]["status"]!="completed"{return Err("Synthetic setup failed".to_string());}return Ok::<_,String>(());}}}).await.map_err(|_|"Synthetic setup timed out")??;
        let mut owner_events=bridge.subscribe();
        let release=bridge.call("thread/unsubscribe",json!({"threadId":thread})).await?;
        if release["status"]=="unloading" {tokio::time::timeout(std::time::Duration::from_secs(5),async{loop{let event=owner_events.recv().await.map_err(|e|e.to_string())?;if event["method"]=="thread/closed"&&event["params"]["threadId"]==thread{return Ok::<_,String>(());}}}).await.map_err(|_|"Synthetic writer did not release")??;}

        bridge.shutdown().await;
        let baseline=Bridge::connect(runtime).await?;
        let before=baseline.call("thread/resume",json!({"threadId":thread,"excludeTurns":true})).await?;
        let before_path=before["thread"]["path"].as_str().ok_or("No baseline path")?;
        let before_raw=std::fs::read_to_string(before_path).map_err(|e|e.to_string())?;
        let permission_history=|raw:&str|raw.lines().filter_map(|line|serde_json::from_str::<serde_json::Value>(line).ok()).filter(|item|item["type"]=="turn_context").map(|item|json!({"approval_policy":item["payload"]["approval_policy"],"sandbox_policy":item["payload"]["sandbox_policy"]})).collect::<Vec<_>>();
        let before_permissions=permission_history(&before_raw);
        if before_permissions.is_empty(){return Err("No persisted setup turn context".into());}
        baseline.shutdown().await;
        let mut roster=Roster::new("Owner")?;let member=roster.add_member(&Actor::LocalDevice,"Member")?;roster.set_voice_enrolled(&Actor::LocalDevice,&member,true)?;
        roster.grant_chat(&Actor::LocalDevice,&thread,&member,ChatAccess::Read)?;roster.grant_chat(&Actor::LocalDevice,&thread,&member,ChatAccess::ReadWrite)?;
        let Admission::Admit(turn)=roster.admit("synthetic-capture",&SpeakerOutcome::Recognised{profile:member,score:0.99})else{return Err("Admission failed".into())};
        let path=directory.path().join("profiles.json");roster.save(&path)?;let policy=ChatPolicy{roster_path:path,turn,global_mode:Mode::Confirm,companion_thread:None};
        let text="SYNTHETIC_MEMBER_USER_POST_29741";
        let mut consent=Consent::default();consent.begin(Mode::Confirm,"Post this message");consent.bind("source");consent.stage("source",Plan{action:Action::Send,thread_id:thread.clone(),title:"notdot synthetic member posting proof".into(),message:text.into(),unambiguous:true})?;
        let id=consent.pending().ok_or("No proposal")?;if !matches!(consent.question(&id)?,Decision::Ask(_)){return Err("No readback".into());}
        let authority=consent.answer(&id,"yes please send that message")?.ok_or("No authority")?;
        let session=SessionRuntime::connect(runtime).await?;let mut events=session.subscribe();
        let posted=session.execute_for_profile(authority,false,Some(&policy)).await;
        let completion=session.completion().await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let mut starts=0;while let Ok(event)=events.try_recv(){if event["method"]=="turn/started"{starts+=1;}}
        session.shutdown().await;
        let ack=posted?;
        let inspector=Bridge::connect(runtime).await?;
        let resumed=inspector.call("thread/resume",json!({"threadId":thread,"excludeTurns":true})).await?;
        if resumed["sandbox"]!=before["sandbox"] || resumed["approvalPolicy"]!=before["approvalPolicy"]{return Err("Posting changed fresh-resume effective permissions".into());}
        let visible=inspector.call("thread/read",json!({"threadId":thread,"includeTurns":true})).await?.to_string().contains(text);
        let path=resumed["thread"]["path"].as_str().ok_or("Synthetic rollout path unavailable")?;
        let raw=std::fs::read_to_string(path).map_err(|e|e.to_string())?;
        let permissions_preserved=permission_history(&raw)==before_permissions;
        if !permissions_preserved{return Err("Posting changed persisted permission history".into());}
        let persisted_user=raw.lines().filter_map(|line|serde_json::from_str::<serde_json::Value>(line).ok()).any(|item|{
            let item=if item["type"]=="response_item"{&item["payload"]}else{&item};item["type"]=="message"&&item["role"]=="user"&&item.to_string().contains(text)
        });
        if !persisted_user || starts!=0 || completion.is_some(){return Err(format!("Post validation failed: persisted_user={persisted_user}, started_turns={starts}"));}
        inspector.shutdown().await;
        Ok(json!({"status":"passed","persisted_forced_user_message":persisted_user,"started_turns":starts,"completion_report_created":completion.is_some(),"fresh_resume_sandbox_unchanged":true,"fresh_resume_approval_unchanged":true,"persisted_permission_history_unchanged":permissions_preserved,"original_persisted_permissions":before_permissions,"acknowledgement":ack,"desktop_turn_view_contains_raw_post":visible}))
    }.await;
    bridge.shutdown().await;
    let cleanup_bridge = Bridge::connect(runtime).await?;
    let cleanup = cleanup_bridge
        .call("thread/archive", json!({"threadId":thread}))
        .await;
    cleanup_bridge.shutdown().await;
    cleanup?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result?).map_err(|e| e.to_string())?
    );
    Ok(())
}
