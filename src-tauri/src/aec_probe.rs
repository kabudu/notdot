//! Debug-only acoustic probe for echo-cancelled interruption listening.
//!
//! `notdot --aec-probe http://127.0.0.1:8109/` opens one window on a local
//! fixture page under its own identifier. The page captures the microphone
//! through the webview (with and without `echoCancellation`), sends 16 kHz
//! PCM here, and this process runs the real on-device wake detector on it.
//! No app state, keys, stores, providers or settings are opened. The "person"
//! is a synthetic fixture played by a separate system process, so the webview
//! cannot use it as its echo reference. Compiled out of release builds.
use std::{
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::{Manager, State};

struct Probe {
    detector: Mutex<Option<crate::wake::probe::Detector>>,
    fixtures: PathBuf,
    started: Instant,
    player: Mutex<Option<std::process::Child>>,
}

#[tauri::command]
fn probe_reset(state: State<'_, Probe>, phrase: String) -> Result<(), String> {
    let models = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/wake");
    *state.detector.lock().map_err(|_| "probe")? =
        Some(crate::wake::probe::Detector::new(&models, &phrase)?);
    Ok(())
}
/// Feed 16 kHz mono PCM; returns milliseconds since probe start when the
/// wake phrase is detected in this chunk.
#[tauri::command]
fn probe_frames(state: State<'_, Probe>, samples: Vec<f32>) -> Result<Option<u128>, String> {
    if samples.len() > 16_000 {
        return Err("chunk too large".into());
    }
    let mut detector = state.detector.lock().map_err(|_| "probe")?;
    let detector = detector.as_mut().ok_or("reset first")?;
    let mut hit = false;
    for chunk in samples.chunks(320) {
        hit |= detector.accept(16_000, chunk);
    }
    Ok(hit.then(|| state.started.elapsed().as_millis()))
}
/// Play a fixture through a separate system process (the synthetic person).
#[tauri::command]
fn probe_play_person(state: State<'_, Probe>, name: String, volume: f32) -> Result<u128, String> {
    if !matches!(
        name.as_str(),
        "hey-ava.wav" | "hey-ava-uk.wav" | "hey-ava-us.wav" | "ava.wav" | "negative.wav"
    ) || !(0.0..=1.0).contains(&volume)
    {
        return Err("unknown fixture".into());
    }
    let path = state.fixtures.join(&name);
    let child = player(&path, volume)
        .spawn()
        .map_err(|e| format!("could not start the system player: {e}"))?;
    *state.player.lock().map_err(|_| "probe")? = Some(child);
    Ok(state.started.elapsed().as_millis())
}
fn player(path: &std::path::Path, volume: f32) -> std::process::Command {
    #[cfg(target_os = "macos")]
    {
        let mut c = std::process::Command::new("/usr/bin/afplay");
        c.arg("-v").arg(volume.to_string()).arg(path);
        c
    }
    #[cfg(target_os = "linux")]
    {
        let _ = volume;
        let mut c = std::process::Command::new("paplay");
        c.arg(path);
        c
    }
    #[cfg(target_os = "windows")]
    {
        let _ = volume;
        let mut c = std::process::Command::new("powershell");
        c.args(["-NoProfile", "-Command"]).arg(format!(
            "(New-Object Media.SoundPlayer '{}').PlaySync()",
            path.display()
        ));
        c
    }
}
struct Params(String);
#[tauri::command]
fn probe_params(params: State<'_, Params>) -> String {
    params.0.clone()
}
#[tauri::command]
fn probe_ping(value: String) -> String {
    format!("pong:{value}")
}
#[tauri::command]
fn probe_now(state: State<'_, Probe>) -> u128 {
    state.started.elapsed().as_millis()
}
#[tauri::command]
fn probe_cpu() -> f32 {
    use sysinfo::{Pid, ProcessesToUpdate, System};
    let mut system = System::new();
    let pid = Pid::from_u32(std::process::id());
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    std::thread::sleep(Duration::from_millis(500));
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    system.process(pid).map_or(-1., |p| p.cpu_usage())
}
/// Results go to the developer's ignored `.local/claude-logs` folder, since a
/// LaunchServices launch has no terminal. Synthetic counts only; no audio.
fn log_path(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.local/claude-logs");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(name)
}
#[tauri::command]
fn probe_log(line: String) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path("aec-probe-progress.log"))
    {
        let _ = writeln!(f, "{}", line.chars().take(2000).collect::<String>());
    }
}
#[tauri::command]
fn probe_report(report: serde_json::Value) {
    println!("AEC-PROBE {report}");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let _ = std::fs::write(
        log_path(&format!("aec-probe-{stamp}.json")),
        serde_json::to_vec_pretty(&report).unwrap_or_default(),
    );
}
#[tauri::command]
fn probe_done(app: tauri::AppHandle) {
    app.exit(0);
}

pub fn run(address: &str) {
    let url: tauri::Url = address.parse().expect("local fixture URL");
    let host = url.host_str().unwrap_or_default();
    assert!(
        url.scheme() == "http" && (host == "127.0.0.1" || host == "localhost"),
        "Use only a local HTTP fixture server"
    );
    let mut context = crate::app_context();
    context.config_mut().identifier = "local.notdot.aec-probe".into();
    context.config_mut().app.windows.clear();
    context.config_mut().app.tray_icon = None;
    // Load the page as this debug build's own development origin, so its
    // commands are local IPC rather than a remote site's. Only the fixed
    // loopback origin is used; the query is passed through `probe_params`.
    let origin: tauri::Url = format!("{}://{}:{}", url.scheme(), host, url.port().unwrap_or(80))
        .parse()
        .expect("loopback origin");
    context.config_mut().build.dev_url = Some(origin);
    let page = url.path().trim_start_matches('/').to_owned();
    let query = url.query().unwrap_or_default().to_owned();
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    tauri::Builder::default()
        .manage(Params(query))
        .manage(Probe {
            detector: Mutex::new(None),
            fixtures,
            started: Instant::now(),
            player: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            probe_reset,
            probe_frames,
            probe_play_person,
            probe_ping,
            probe_params,
            probe_now,
            probe_cpu,
            probe_report,
            probe_log,
            probe_done
        ])
        .setup(move |app| {
            app.add_capability(
                serde_json::json!({
                    "identifier": "local-aec-probe",
                    "windows": ["probe"],
                    "permissions": ["core:default"]
                })
                .to_string(),
            )?;
            tauri::WebviewWindowBuilder::new(
                app,
                "probe",
                tauri::WebviewUrl::App(page.clone().into()),
            )
            .title("notdot echo probe")
            .inner_size(640., 480.)
            .build()?;
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                // Hard stop for an unattended run.
                std::thread::sleep(Duration::from_secs(900));
                if let Some(state) = handle.try_state::<Probe>()
                    && let Ok(mut child) = state.player.lock()
                    && let Some(child) = child.as_mut()
                {
                    let _ = child.kill();
                }
                handle.exit(3);
            });
            Ok(())
        })
        .run(context)
        .expect("echo probe");
}
