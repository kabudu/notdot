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
