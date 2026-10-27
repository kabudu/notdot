#![cfg(unix)]
use notdot_core::bridge::Bridge;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
fn server(source: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runtime");
    std::fs::write(&path, format!("#!/usr/bin/env python3\n{source}")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    (dir, path)
}
#[tokio::test]
async fn interleaved_events_and_host_request_rejection() {
    let (_dir, path) = server(
        r#"import json,sys
for line in sys.stdin:
 m=json.loads(line)
 if m.get('method')=='initialize':print(json.dumps({'id':m['id'],'result':{}}),flush=True)
 elif m.get('method')=='probe':
  print(json.dumps({'id':99,'method':'item/commandExecution/requestApproval','params':{}}),flush=True)
  answer=json.loads(sys.stdin.readline())
  print(json.dumps({'method':'turn/completed','params':{'turn':{'status':'completed'}}}),flush=True)
  print(json.dumps({'id':m['id'],'result':{'approvalRejected':'error' in answer}}),flush=True)
 elif m.get('method')=='fail':print(json.dumps({'id':m['id'],'error':{'code':-32600,'message':'already has an active writer'}}),flush=True)
"#,
    );
    let bridge = Bridge::connect(&path).await.unwrap();
    let mut events = bridge.subscribe();
    assert_eq!(
        bridge.call("probe", json!({})).await.unwrap()["approvalRejected"],
        true
    );
    assert_eq!(
        events.recv().await.unwrap()["method"],
        "notdot/unsupportedRequest"
    );
    assert_eq!(events.recv().await.unwrap()["method"], "turn/completed");
    assert!(
        bridge
            .call("fail", json!({}))
            .await
            .unwrap_err()
            .contains("active writer")
    );
    bridge.shutdown().await;
}
#[tokio::test]
async fn broken_runtime_fails_without_hanging() {
    let (_dir, path) = server("import sys\nsys.exit(0)");
    let result =
        tokio::time::timeout(std::time::Duration::from_secs(3), Bridge::connect(&path)).await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_err());
}

#[tokio::test]
async fn interruption_identifies_both_thread_and_turn() {
    let (_dir, path) = server(
        r#"import json,sys
for line in sys.stdin:
 m=json.loads(line)
 if m.get('method')=='initialize':print(json.dumps({'id':m['id'],'result':{}}),flush=True)
 elif m.get('method')=='turn/interrupt':
  assert m['params']=={'threadId':'thread-one','turnId':'turn-two'}
  print(json.dumps({'id':m['id'],'result':{}}),flush=True)
"#,
    );
    let bridge = Bridge::connect(&path).await.unwrap();
    bridge.interrupt("thread-one", "turn-two").await.unwrap();
    bridge.shutdown().await;
}

#[tokio::test]
async fn unresolved_turn_identity_never_sends_an_interrupt() {
    let (_dir, path) = server(
        r#"import json,sys
for line in sys.stdin:
 m=json.loads(line)
 if m.get('method')=='initialize':print(json.dumps({'id':m['id'],'result':{}}),flush=True)
 elif m.get('method')=='turn/interrupt':sys.exit(91)
 elif m.get('method')=='probe':print(json.dumps({'id':m['id'],'result':{'alive':True}}),flush=True)
"#,
    );
    let b = Bridge::connect(&path).await.unwrap();
    for (thread, turn) in [("t", "submission-pending"), ("t", ""), ("", "turn")] {
        assert!(b.interrupt(thread, turn).await.is_err());
    }
    assert_eq!(b.call("probe", json!({})).await.unwrap()["alive"], true);
    b.shutdown().await;
}
#[tokio::test]
async fn structured_analysis_is_ephemeral_scoped_and_unloaded() {
    let (_dir, path) = server(
        r#"import json,sys
unloaded=False
for line in sys.stdin:
 m=json.loads(line);method=m.get('method')
 if method=='initialize':result={}
 elif method=='config/read':result={'config':{'mcp_servers':{'private':{}},'plugins':{'private':{}}}}
 elif method=='thread/start':
  p=m['params'];assert p['ephemeral'] is True
  assert p['config']['features']['shell_tool'] is False
  assert p['config']['features']['apps'] is False
  assert p['config']['mcp_servers']['private']['enabled'] is False
  assert p['config']['plugins']['private']['enabled'] is False
  result={'thread':{'id':'analysis','ephemeral':True},'model':'gpt-6.1-terra','reasoningEffort':'medium'}
 elif method=='turn/start':
  assert 'outputSchema' in m['params'];result={'turn':{'id':'analysis-turn'}}
  print(json.dumps({'id':m['id'],'result':result}),flush=True)
  print(json.dumps({'method':'item/agentMessage/delta','params':{'threadId':'other','delta':'bad'}}),flush=True)
  print(json.dumps({'method':'item/completed','params':{'threadId':'analysis','turnId':'analysis-turn','item':{'type':'agentMessage','phase':'final_answer','text':'{"notes":[],"digest":null}'}}}),flush=True)
  print(json.dumps({'method':'turn/completed','params':{'threadId':'analysis','turn':{'id':'analysis-turn','status':'completed'}}}),flush=True)
  continue
 elif method=='thread/unsubscribe':unloaded=True;result={}
 elif method=='probe':result={'unloaded':unloaded}
 else:continue
 print(json.dumps({'id':m['id'],'result':result}),flush=True)
"#,
    );
    let b = Bridge::connect(&path).await.unwrap();
    let output = b
        .analyze(
            _dir.path(),
            "synthetic input",
            notdot_core::memory::schema(),
            std::future::pending(),
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&output).unwrap(),
        json!({"notes":[],"digest":null})
    );
    assert_eq!(b.call("probe", json!({})).await.unwrap()["unloaded"], true);
    b.shutdown().await;
}

#[tokio::test]
#[ignore = "explicit live authenticated Terra validation; synthetic ephemeral data only"]
async fn live_semantic_learning_and_distillation() {
    use notdot_core::memory::{Interaction, LearningBatch, Memory};
    let binary = std::env::var_os("NOTDOT_MEMORY_LIVE_RUNTIME")
        .expect("set runtime path for explicit live test");
    let dir = tempfile::tempdir().unwrap();
    let m = Memory::open_with_learning(&dir.path().join("ledger"), true).unwrap();
    let b = Bridge::connect(std::path::Path::new(&binary))
        .await
        .unwrap();
    let text = "For my project Notdot I use Rust and Tauri. I prefer concise explanations and British English spelling in every response.";
    let plan = m
        .plan(
            m.learning_ticket().unwrap(),
            Some(Interaction {
                thread: "synthetic-proof".into(),
                turn: "synthetic-turn-1".into(),
                user: text.into(),
            }),
        )
        .await
        .unwrap()
        .unwrap();
    let answer = b
        .analyze(
            dir.path(),
            &plan.prompt().unwrap(),
            notdot_core::memory::schema(),
            std::future::pending(),
        )
        .await
        .unwrap();
    let batch: LearningBatch = serde_json::from_str(&answer).unwrap();
    assert!(!batch.notes.is_empty());
    let written = m.apply_learning(&plan, batch).await.unwrap();
    assert!(written > 0);
    let facts = m.review("").await.unwrap();
    eprintln!(
        "Synthetic learned facts: {:?}",
        facts
            .iter()
            .map(|r| (&r.content, &r.kind, &r.quality_state))
            .collect::<Vec<_>>()
    );
    assert!(
        facts
            .iter()
            .any(|n| n.content.to_lowercase().contains("rust")
                || n.content.to_lowercase().contains("british"))
    );
    // A separate completed observation confirms the model's grounded proposals.
    let confirmation = m
        .plan(
            m.learning_ticket().unwrap(),
            Some(Interaction {
                thread: "synthetic-proof".into(),
                turn: "synthetic-corroboration".into(),
                user: text.into(),
            }),
        )
        .await
        .unwrap()
        .unwrap();
    let proposals = serde_json::json!({"notes":facts.iter().map(|r|serde_json::json!({"key":r.metadata["semantic_key"],"content":r.content,"evidence":r.metadata["evidence"],"confidence":0.95})).collect::<Vec<_>>(),"digest":null});
    m.apply_learning(&confirmation, serde_json::from_value(proposals).unwrap())
        .await
        .unwrap();
    let plan = m
        .plan(m.learning_ticket().unwrap(), None)
        .await
        .unwrap()
        .unwrap();
    let answer = b
        .analyze(
            dir.path(),
            &plan.prompt().unwrap(),
            notdot_core::memory::schema(),
            std::future::pending(),
        )
        .await
        .unwrap();
    let batch: LearningBatch = serde_json::from_str(&answer).unwrap();
    assert!(batch.notes.is_empty());
    assert!(
        batch.digest.is_some(),
        "Synthetic review response: {answer}"
    );
    assert_eq!(m.apply_learning(&plan, batch).await.unwrap(), 1);
    assert!(
        m.review("")
            .await
            .unwrap()
            .iter()
            .any(|r| r.kind == mnemara::MemoryRecordKind::Summary)
    );
    let old_style = facts
        .iter()
        .find(|r| r.content.to_lowercase().contains("concise"))
        .unwrap();
    let style_key = old_style.metadata["semantic_key"].clone();
    let plan = m.plan(m.learning_ticket().unwrap(), Some(Interaction { thread:"synthetic-proof".into(), turn:"synthetic-turn-2".into(), user:"I changed my mind: I now prefer detailed explanations instead of concise answers. I still want British English spelling.".into() })).await.unwrap().unwrap();
    let answer = b
        .analyze(
            dir.path(),
            &plan.prompt().unwrap(),
            notdot_core::memory::schema(),
            std::future::pending(),
        )
        .await
        .unwrap();
    m.apply_learning(&plan, serde_json::from_str(&answer).unwrap())
        .await
        .unwrap();
    let records = m.review("").await.unwrap();
    assert!(
        !records.iter().any(|r| r.id == old_style.id),
        "correction must supersede the old style: {answer}"
    );
    let corrected = records
        .iter()
        .find(|r| r.metadata.get("semantic_key") == Some(&style_key))
        .unwrap()
        .id
        .clone();
    assert!(
        records
            .iter()
            .find(|r| r.id == corrected)
            .unwrap()
            .content
            .to_lowercase()
            .contains("detailed")
    );
    m.forget(&corrected).await.unwrap();
    drop(m);
    let m = Memory::open_with_learning(&dir.path().join("ledger"), true).unwrap();
    assert!(
        !m.review("")
            .await
            .unwrap()
            .iter()
            .any(|r| r.id == corrected)
    );
    b.shutdown().await;
}
