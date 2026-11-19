//! Local-only native input host. Serve a synthetic UI at localhost:8107 first.
//! This executable has no app-state, bridge, keychain or microphone handlers.
#[path = "../src/orb_input.rs"]
mod orb_input;
use tauri::Manager;
#[tauri::command]
fn record_orb_input_mode(window: tauri::WebviewWindow, collapsed: bool) -> Result<(), String> {
    let result = orb_input::orb_input_mode(window.clone(), collapsed);
    let input = window.app_handle().get_webview_window("orb-input");
    eprintln!(
        "layout collapsed={collapsed} size={:?} result={result:?}",
        input.as_ref().map(|w| w.inner_size())
    );
    let _ = window.set_title(&format!(
        "Input test: collapsed={collapsed}, visible={:?}, focused={:?}, result={result:?}",
        input.as_ref().map(|w| w.is_visible()),
        input.as_ref().map(|w| w.is_focused())
    ));
    result
}
#[tauri::command]
fn record_visual_report(report: serde_json::Value) {
    eprintln!("visual {report}");
}
fn report(app: &tauri::AppHandle) {
    if let (Some(main), Some(input)) = (
        app.get_webview_window("main"),
        app.get_webview_window("orb-input"),
    ) {
        eprintln!(
            "geometry core={:?} size={:?}; visual={:?}",
            input.inner_position(),
            input.inner_size(),
            main.inner_position()
        );
        let _ = input.set_title(&format!(
            "Core {:?}; visual {:?}",
            input.inner_position(),
            main.inner_position()
        ));
    }
}
fn main() {
    eprintln!("input host: entry");
    let mut context = tauri::generate_context!();
    context.config_mut().identifier = "local.notdot.input-acceptance".into();
    eprintln!("input host: context ready");
    let address =
        std::env::var("NOTDOT_INPUT_TEST_URL").unwrap_or_else(|_| "http://localhost:8107".into());
    let url: tauri::Url = address.parse().expect("local fixture URL");
    let host = url.host_str().unwrap_or_default();
    assert!(
        url.scheme() == "http"
            && (host == "localhost"
                || host == "::1"
                || host
                    .parse::<std::net::Ipv4Addr>()
                    .is_ok_and(|ip| ip.is_loopback() || ip.is_private())),
        "Use only a local HTTP fixture server"
    );
    context.config_mut().build.dev_url = Some(url);
    let fixture_scope = format!("{}/*", address.trim_end_matches('/'));
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![record_orb_input_mode,record_visual_report])
        .setup(move |app| {
            eprintln!("input host: setting up windows");
            app.add_capability(serde_json::json!({
                "identifier":"local-input-acceptance",
                "windows":["main","orb-input"],
                "remote":{"urls":[fixture_scope]},
                "permissions":["core:default","core:window:allow-start-dragging","core:window:allow-set-size","core:window:allow-set-position"]
            }).to_string())?;
            orb_input::install(app.handle())?;
            orb_input::refresh(app.handle());
            report(app.handle());
            if let Some(input)=app.get_webview_window("orb-input") {
                let handle=app.handle().clone();
                input.on_window_event(move |event|if matches!(event,tauri::WindowEvent::Moved(_)){report(&handle)});
            }
            Ok(())
        })
        .run(context).expect("local native input host");
}
