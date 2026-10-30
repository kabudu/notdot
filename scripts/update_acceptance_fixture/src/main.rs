//! Explicitly launched, no-window native updater acceptance fixture. No real runtime or account.
use notdot_core::bridge::Bridge;
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::{BufRead, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tauri_plugin_updater::UpdaterExt;

fn lab(path: &Path) -> PathBuf {
    let path = path.canonicalize().expect("fixture lab");
    assert_eq!(
        fs::read_to_string(path.join("fixture-only")).unwrap(),
        "notdot-disposable-native-update"
    );
    path
}
fn event(lab: &Path, kind: &str, data: Value) {
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(lab.join("events.jsonl"))
        .unwrap();
    let mut b =
        serde_json::to_vec(&json!({"event":kind,"pid":std::process::id(),"data":data})).unwrap();
    b.push(b'\n');
    f.write_all(&b).unwrap();
    f.sync_all().unwrap();
}
fn writer() {
    let root = lab(std::env::current_exe().unwrap().parent().unwrap());
    let lease = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(root.join("writer.lock"))
        .unwrap();
    if lease.try_lock().is_err() {
        event(&root, "writer_overlap", json!({}));
        std::process::exit(75);
    }
    event(&root, "writer_started", json!({}));
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap();
        let value: Value = serde_json::from_str(&line).unwrap();
        if let Some(id) = value.get("id") {
            let result = if value["method"] == "fixture/resume" {
                json!({"thread":"synthetic-update-thread","writer_pid":std::process::id()})
            } else {
                json!({})
            };
            println!("{}", json!({"id":id,"result":result}));
            std::io::stdout().flush().unwrap();
        }
    }
    event(&root, "writer_stopped", json!({}));
    drop(lease);
}
async fn connect(root: &Path) -> Result<Bridge, String> {
    let bridge = Bridge::connect(&root.join("fixture-writer")).await?;
    let result = bridge
        .call(
            "fixture/resume",
            json!({"thread":"synthetic-update-thread"}),
        )
        .await?;
    if result["thread"] != "synthetic-update-thread" {
        return Err("synthetic profile association lost".into());
    }
    Ok(bridge)
}
fn assert_no_writer(root: &Path) -> Result<(), String> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join("writer.lock"))
        .map_err(|e| e.to_string())?;
    file.try_lock()
        .map_err(|_| "writer lease remained held after actual Bridge::shutdown".to_string())?;
    Ok(())
}
async fn exercise(
    app: tauri::AppHandle,
    root: PathBuf,
    duplicate_seen: Arc<AtomicBool>,
) -> Result<(), String> {
    let version = app.package_info().version.to_string();
    let profile = fs::read(root.join("profile.json")).map_err(|e| e.to_string())?;
    assert_eq!(
        serde_json::from_slice::<Value>(&profile).unwrap()["thread"],
        "synthetic-update-thread"
    );
    let mut bridge = connect(&root).await?;
    event(&root, "writer_connected", json!({"version":version}));
    if version == "9.9.9" {
        let child = tokio::process::Command::new(std::env::current_exe().unwrap())
            .arg("--lab")
            .arg(&root)
            .spawn()
            .map_err(|e| e.to_string())?;
        let status = tokio::time::timeout(Duration::from_secs(5), child.wait_with_output())
            .await
            .map_err(|_| "duplicate process timed out")?
            .map_err(|e| e.to_string())?;
        let observed_until = std::time::Instant::now() + Duration::from_secs(2);
        while !duplicate_seen.load(Ordering::SeqCst) && std::time::Instant::now() < observed_until {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        if !status.status.success() || !duplicate_seen.load(Ordering::SeqCst) {
            return Err("single-instance restart ownership was not established".into());
        }
        bridge.call("fixture/resume", json!({})).await?;
        bridge.shutdown().await;
        assert_no_writer(&root)?;
        assert_eq!(fs::read(root.join("profile.json")).unwrap(), profile);
        event(
            &root,
            "acceptance_passed",
            json!({"version":version,"profile_preserved":true,"duplicate_rejected":true}),
        );
        fs::write(
            root.join("result.json"),
            b"{\"passed\":true,\"version\":\"9.9.9\"}",
        )
        .map_err(|e| e.to_string())?;
        app.exit(0);
        return Ok(());
    }
    assert_eq!(version, "9.9.8");
    let metadata: Value =
        serde_json::from_slice(&fs::read(root.join("update.json")).unwrap()).unwrap();
    let updater = app
        .updater_builder()
        .target("fixture-macos")
        .build()
        .map_err(|e| e.to_string())?;
    let update = updater
        .restore_update(metadata.clone())
        .map_err(|e| e.to_string())?
        .ok_or("fixture update missing")?;
    let bad_signature = fs::read_to_string(root.join("malformed.bin.sig")).unwrap();
    let mut bad_metadata = metadata;
    bad_metadata["platforms"]["fixture-macos"]["signature"] = json!(bad_signature.trim());
    let bad = updater
        .restore_update(bad_metadata)
        .map_err(|e| e.to_string())?
        .unwrap();
    let bad_bytes = fs::read(root.join("malformed.bin")).unwrap();
    bad.verify(&bad_bytes).map_err(|e| e.to_string())?;
    // Same ordering as notdot: close sole writer before replacing; reconnect on failure.
    bridge.shutdown().await;
    assert_no_writer(&root)?;
    let failure = tokio::task::spawn_blocking(move || bad.install(bad_bytes))
        .await
        .map_err(|e| e.to_string())?;
    if failure.is_ok() {
        return Err("malformed archive unexpectedly installed".into());
    }
    bridge = connect(&root).await?;
    assert_eq!(fs::read(root.join("profile.json")).unwrap(), profile);
    event(
        &root,
        "failed_install_recovered",
        json!({"version":version}),
    );
    bridge.shutdown().await;
    assert_no_writer(&root)?;
    let bytes = fs::read(root.join("update.app.tar.gz")).unwrap();
    tokio::task::spawn_blocking(move || update.install(bytes))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    event(
        &root,
        "installed_before_restart",
        json!({"old_version":version}),
    );
    app.restart();
}
fn main() {
    if std::env::args().nth(1).as_deref() == Some("app-server") {
        writer();
        return;
    }
    let args: Vec<_> = std::env::args_os().collect();
    assert_eq!(args.get(1).and_then(|s| s.to_str()), Some("--lab"));
    let root = lab(Path::new(args.get(2).expect("explicit fixture lab")));
    let watchdog = root.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(45));
        event(&watchdog, "watchdog_timeout", json!({}));
        std::process::exit(76);
    });
    let pubkey = fs::read_to_string(root.join("fixture.pub")).unwrap();
    let duplicate_seen = Arc::new(AtomicBool::new(false));
    let observed = duplicate_seen.clone();
    let duplicate_lab = root.clone();
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(move |_, _, _| {
            observed.store(true, Ordering::SeqCst);
            event(&duplicate_lab, "duplicate_rejected", json!({}));
        }))
        .plugin(
            tauri_plugin_updater::Builder::new()
                .pubkey(pubkey.trim())
                .build(),
        )
        .setup(move |app| {
            assert!(
                app.config()
                    .identifier
                    .starts_with("com.kabudu.notdot.update-acceptance-fixture")
            );
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = exercise(handle.clone(), root.clone(), duplicate_seen).await {
                    event(&root, "acceptance_failed", json!({"error":e}));
                    let _ = fs::write(
                        root.join("result.json"),
                        json!({"passed":false,"error":e}).to_string(),
                    );
                    handle.exit(77);
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("native fixture runtime");
}
