//! Separate decoration from input: the large transparent visual window never
//! receives pointer events while collapsed. A smaller, inscribed input window
//! owns the centre. This avoids cursor polling and its first-click race.
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

pub struct InputWindow {
    collapsed: AtomicBool,
}
fn geometry(width: f64, height: f64) -> (f64, f64, f64) {
    let side = width.min(height) * 0.17 * std::f64::consts::SQRT_2;
    ((width - side) / 2., (height - side) / 2., side)
}
fn sync(app: &tauri::AppHandle) -> Result<(), String> {
    let Some(main) = app.get_webview_window("main") else {
        return Ok(());
    };
    let Some(input) = app.get_webview_window("orb-input") else {
        return Ok(());
    };
    let collapsed = app.state::<InputWindow>().collapsed.load(Ordering::Relaxed);
    if !collapsed || !main.is_visible().unwrap_or(false) || main.is_minimized().unwrap_or(true) {
        main.set_ignore_cursor_events(false)
            .map_err(|e| e.to_string())?;
        return input.hide().map_err(|e| e.to_string());
    }
    let position = main.inner_position().map_err(|e| e.to_string())?;
    let size = main.inner_size().map_err(|e| e.to_string())?;
    let (x, y, side) = geometry(f64::from(size.width), f64::from(size.height));
    input
        .set_size(tauri::PhysicalSize::new(
            side.round() as u32,
            side.round() as u32,
        ))
        .map_err(|e| e.to_string())?;
    input
        .set_position(tauri::PhysicalPosition::new(
            position.x + x.round() as i32,
            position.y + y.round() as i32,
        ))
        .map_err(|e| e.to_string())?;
    input.show().map_err(|e| e.to_string())?;
    main.set_ignore_cursor_events(true)
        .map_err(|e| e.to_string())
}
pub fn install(app: &tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    app.manage(InputWindow {
        collapsed: AtomicBool::new(false),
    });
    WebviewWindowBuilder::new(app, "orb-input", WebviewUrl::App("orb-input.html".into()))
        .title("notdot voice control")
        .inner_size(86., 86.)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .resizable(false)
        .skip_taskbar(true)
        .visible(false)
        .focused(false)
        .build()?;
    if let Some(main) = app.get_webview_window("main") {
        let handle = app.clone();
        main.on_window_event(move |event| {
            if matches!(
                event,
                tauri::WindowEvent::Moved(_)
                    | tauri::WindowEvent::Resized(_)
                    | tauri::WindowEvent::ScaleFactorChanged { .. }
            ) && let Err(error) = sync(&handle)
            {
                rollback(&handle);
                eprintln!("Orb input layout: {error}");
            }
        });
    }
    if let Err(error) = sync(app) {
        rollback(app);
        return Err(error.into());
    }
    Ok(())
}
pub fn refresh(app: &tauri::AppHandle) {
    if let Err(error) = sync(app) {
        rollback(app);
        eprintln!("Orb input layout: {error}");
    }
}
fn rollback(app: &tauri::AppHandle) {
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.set_ignore_cursor_events(false);
    }
    if let Some(input) = app.get_webview_window("orb-input") {
        let _ = input.hide();
    }
}
#[tauri::command]
pub fn orb_input_mode(window: WebviewWindow, collapsed: bool) -> Result<(), String> {
    if window.label() != "main" {
        return Err("Only the companion window can change input layout".into());
    }
    let app = window.app_handle();
    app.try_state::<InputWindow>()
        .ok_or("Orb input is not initialized")?
        .collapsed
        .store(collapsed, Ordering::Relaxed);
    let result = sync(app);
    if !collapsed {
        let _ = window.set_focus();
    } else if window.is_focused().unwrap_or(false)
        && let Some(input) = app.get_webview_window("orb-input")
    {
        let _ = input.set_focus();
    }
    if result.is_err() {
        rollback(app);
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn input_corners_stay_inside_body_at_all_densities() {
        for scale in [1., 1.25, 1.5, 2.] {
            let w = 360. * scale;
            let (x, y, side) = geometry(w, w);
            assert!((x + side / 2. - w / 2.).abs() < 1e-9);
            assert!((y + side / 2. - w / 2.).abs() < 1e-9);
            assert!((side / std::f64::consts::SQRT_2 - w * 0.17).abs() < 1e-9);
            assert!(side / scale > 44.);
        }
    }
}
