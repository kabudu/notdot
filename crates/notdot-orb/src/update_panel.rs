//! Compact software-update panel opened by "Check for Updates…" in the app and
//! tray menus. It only presents the native updater's status: checking, install
//! and deferral rules stay in `src-tauri/src/updates.rs`.
use super::{Shared, UpdateStatus, call, dialog, element, on, resize, stop_audio};
use serde_json::json;
use std::cell::RefCell;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlElement;

const ID: &str = "update-panel";

/// How the download bar is drawn. `Percent` is a determinate value the
/// platform exposes to assistive technology; nothing animates for it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Progress {
    Hidden,
    Indeterminate,
    Percent(u8),
}

/// Which controls a native update phase offers. Shared by the panel and the
/// Settings section, so both always agree about what can be done.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Controls {
    pub install: bool,
    pub later: bool,
    pub retry: bool,
    pub busy: bool,
    pub progress: Progress,
}

pub(crate) fn controls(phase: &str, received: u64, total: Option<u64>) -> Controls {
    let progress = match phase {
        "downloading" => {
            total
                .filter(|total| *total > 0)
                .map_or(Progress::Indeterminate, |total| {
                    // Integer maths: no float rounding past 100 and no overflow.
                    let percent = u128::from(received.min(total)) * 100 / u128::from(total);
                    Progress::Percent(u8::try_from(percent).unwrap_or(100))
                })
        }
        "installing" => Progress::Percent(100),
        _ => Progress::Hidden,
    };
    Controls {
        install: phase == "available",
        later: phase == "available",
        retry: phase == "error",
        busy: matches!(phase, "checking" | "downloading" | "installing"),
        progress,
    }
}

/// A status the panel learned from its own command, held until a native
/// snapshot that was requested after the command finished can replace it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Hold {
    /// The command is still running.
    Running,
    /// The command finished at this time (ms since the epoch).
    Settled(f64),
    /// A failed command: keep the error on screen until the person acts.
    Sticky,
}

/// Choose between the panel's own status and a polled native snapshot.
/// `polled_at` is when that snapshot was requested: an older request can
/// still be in flight when a check finishes and must not show stale state.
pub(crate) fn current<'a>(
    local: Option<&'a (UpdateStatus, Hold)>,
    polled: &'a UpdateStatus,
    polled_at: f64,
) -> (&'a UpdateStatus, bool) {
    match local {
        Some((status, Hold::Running | Hold::Sticky)) => (status, true),
        Some((status, Hold::Settled(at))) if polled_at < *at => (status, true),
        _ => (polled, false),
    }
}

#[derive(Default)]
struct Panel {
    local: Option<(UpdateStatus, Hold)>,
    shown: Option<(String, String)>,
    return_focus: Option<HtmlElement>,
}
thread_local! {
    static PANEL: RefCell<Panel> = RefCell::new(Panel::default());
}

fn status(phase: &str, message: impl Into<String>) -> UpdateStatus {
    UpdateStatus {
        phase: phase.into(),
        message: message.into(),
        ..UpdateStatus::default()
    }
}
fn now() -> f64 {
    js_sys::Date::now()
}
fn button(id: &str) -> HtmlElement {
    element(id).unchecked_into()
}
pub(crate) fn is_open() -> bool {
    dialog(ID).open()
}

/// Draw the panel. Called on every native poll and after each command.
pub(crate) fn render(polled: &UpdateStatus, polled_at: f64) {
    if !is_open() {
        return;
    }
    let shown = PANEL.with(|panel| {
        let mut panel = panel.borrow_mut();
        let (shown, held) = current(panel.local.as_ref(), polled, polled_at);
        let shown = shown.clone();
        if !held {
            panel.local = None;
        }
        shown
    });
    let controls = controls(&shown.phase, shown.received, shown.total);
    let text = match shown.phase.as_str() {
        "checking" if shown.message.is_empty() => "Checking for updates…".to_owned(),
        _ => shown.message.clone(),
    };
    // Live regions announce every text replacement in some screen readers:
    // touch the text only when the phase or wording really changes.
    let changed = PANEL.with(|panel| {
        let key = (shown.phase.clone(), text.clone());
        let mut panel = panel.borrow_mut();
        let changed = panel.shown.as_ref() != Some(&key);
        panel.shown = Some(key);
        changed
    });
    if changed {
        element("update-panel-status").set_text_content(Some(&text));
    }
    let _ = element(ID).set_attribute("aria-busy", if controls.busy { "true" } else { "false" });
    for (id, visible) in [
        ("update-panel-install", controls.install),
        ("update-panel-later", controls.later),
        ("update-panel-retry", controls.retry),
    ] {
        let _ = element(id).toggle_attribute_with_force("hidden", !visible);
    }
    let bar = element("update-panel-bar");
    match controls.progress {
        Progress::Hidden => {
            let _ = element("update-panel-progress").set_attribute("hidden", "");
        }
        Progress::Indeterminate => {
            let _ = element("update-panel-progress").remove_attribute("hidden");
            let _ = bar.remove_attribute("value");
            element("update-panel-percent").set_text_content(None);
        }
        Progress::Percent(percent) => {
            let _ = element("update-panel-progress").remove_attribute("hidden");
            let _ = bar.set_attribute("value", &percent.to_string());
            element("update-panel-percent").set_text_content(Some(&format!("{percent}%")));
        }
    }
    keep_focus_inside();
}

/// A hidden button cannot keep focus. Move it to Close instead of the page.
fn keep_focus_inside() {
    let active = super::document().active_element();
    let lost = active
        .as_ref()
        .is_none_or(|active| !element(ID).contains(Some(active)) || active.has_attribute("hidden"));
    if lost {
        let _ = button("update-panel-close").focus();
    }
}

fn check(view: &Shared) {
    PANEL.with(|panel| {
        panel.borrow_mut().local =
            Some((status("checking", "Checking for updates…"), Hold::Running));
    });
    render(&view.borrow().snapshot.updates, now());
    let view = view.clone();
    spawn_local(async move {
        let local = match call("check_updates", json!({})).await {
            Ok(value) => serde_wasm_bindgen::from_value::<UpdateStatus>(value)
                .map(|status| (status, Hold::Settled(now())))
                .unwrap_or_else(|_| (status("checking", ""), Hold::Settled(now()))),
            Err(error) => (status("error", error), Hold::Sticky),
        };
        PANEL.with(|panel| panel.borrow_mut().local = Some(local));
        render(&view.borrow().snapshot.updates, f64::NEG_INFINITY);
    });
}

/// Open the panel and start a check straight away. A check, download or
/// installation that is already running is shown instead of a new check.
pub(crate) fn open(view: &Shared) {
    if !is_open() {
        let previous = super::document()
            .active_element()
            .and_then(|e| e.dyn_into::<HtmlElement>().ok());
        PANEL.with(|panel| {
            let mut panel = panel.borrow_mut();
            panel.return_focus = previous;
            panel.shown = None;
        });
        if !dialog("panel").open() && !dialog("about").open() {
            let _ = resize(400., 380.);
        }
        let _ = dialog(ID).show_modal();
    }
    // Always ask the native updater: it joins a check, download or install
    // that is already running and returns that status, so a stale snapshot
    // here can never hide or duplicate one.
    check(view);
}

fn closed(view: &Shared) {
    // Seeing the panel already tells the person about this version; do not
    // also speak the update prompt as soon as they dismiss it.
    let version = view.borrow().snapshot.updates.version.clone();
    if view.borrow().snapshot.updates.phase == "available" {
        view.borrow_mut().update_announced = version;
    }
    let focus = PANEL.with(|panel| {
        let mut panel = panel.borrow_mut();
        panel.local = None;
        panel.shown = None;
        panel.return_focus.take()
    });
    if !dialog("panel").open() && !dialog("about").open() {
        let _ = resize(360., 360.);
    }
    if let Some(focus) = focus.filter(|f| f.is_connected()) {
        let _ = focus.focus();
    }
}

fn fail(error: String) {
    PANEL.with(|panel| panel.borrow_mut().local = Some((status("error", error), Hold::Sticky)));
}

pub(crate) fn install(view: &Shared) {
    on(element(ID).as_ref(), "close", {
        let view = view.clone();
        move |_| closed(&view)
    });
    on(element("update-panel-close").as_ref(), "click", |_| {
        dialog(ID).close()
    });
    on(element("update-panel-retry").as_ref(), "click", {
        let view = view.clone();
        move |_| check(&view)
    });
    on(element("update-panel-later").as_ref(), "click", {
        let view = view.clone();
        move |_| {
            let view = view.clone();
            spawn_local(async move {
                match call("defer_update", json!({})).await {
                    Ok(_) => dialog(ID).close(),
                    Err(error) => {
                        fail(error);
                        render(&view.borrow().snapshot.updates, now());
                    }
                }
            });
        }
    });
    on(element("update-panel-install").as_ref(), "click", {
        let view = view.clone();
        move |_| {
            stop_audio(&view);
            PANEL.with(|panel| {
                panel.borrow_mut().local = Some((
                    status("downloading", "Starting the download…"),
                    Hold::Settled(now()),
                ));
            });
            render(&view.borrow().snapshot.updates, f64::NEG_INFINITY);
            let view = view.clone();
            spawn_local(async move {
                // Success restarts the app, so only a failure returns here.
                if let Err(error) = call("install_update", json!({})).await {
                    fail(error);
                    render(&view.borrow().snapshot.updates, now());
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn polled(phase: &str) -> UpdateStatus {
        status(phase, phase)
    }

    #[test]
    fn each_phase_offers_only_its_own_actions() {
        let c = controls("available", 0, None);
        assert!(c.install && c.later && !c.retry && !c.busy);
        assert_eq!(c.progress, Progress::Hidden);
        let c = controls("error", 0, None);
        assert!(c.retry && !c.install && !c.later && !c.busy);
        for phase in ["current", "package_manager", "idle"] {
            let c = controls(phase, 0, None);
            assert!(!c.install && !c.later && !c.retry && !c.busy, "{phase}");
            assert_eq!(c.progress, Progress::Hidden);
        }
        let c = controls("checking", 0, None);
        assert!(c.busy && !c.install && !c.retry);
        assert_eq!(c.progress, Progress::Hidden);
    }

    #[test]
    fn download_progress_is_bounded_and_never_divides_by_zero() {
        assert_eq!(
            controls("downloading", 5, None).progress,
            Progress::Indeterminate
        );
        assert_eq!(
            controls("downloading", 5, Some(0)).progress,
            Progress::Indeterminate
        );
        assert_eq!(
            controls("downloading", 0, Some(200)).progress,
            Progress::Percent(0)
        );
        assert_eq!(
            controls("downloading", 199, Some(200)).progress,
            Progress::Percent(99)
        );
        assert_eq!(
            controls("downloading", 900, Some(200)).progress,
            Progress::Percent(100)
        );
        assert_eq!(
            controls("downloading", u64::MAX, Some(u64::MAX)).progress,
            Progress::Percent(100)
        );
        assert_eq!(
            controls("installing", 0, None).progress,
            Progress::Percent(100)
        );
        assert!(controls("installing", 0, None).busy);
    }

    #[test]
    fn a_running_check_hides_stale_snapshots_until_a_fresh_poll() {
        let old = polled("current");
        let running = (status("checking", "Checking"), Hold::Running);
        assert_eq!(current(Some(&running), &old, 5.).0.phase, "checking");
        let settled = (polled("available"), Hold::Settled(10.));
        // A snapshot requested before the check finished may predate it.
        assert_eq!(current(Some(&settled), &old, 9.).0.phase, "available");
        assert!(current(Some(&settled), &old, 9.).1);
        // A snapshot requested afterwards is authoritative again.
        let fresh = polled("downloading");
        assert_eq!(current(Some(&settled), &fresh, 11.).0.phase, "downloading");
        assert!(!current(Some(&settled), &fresh, 11.).1);
        assert_eq!(current(None, &fresh, 0.).0.phase, "downloading");
    }

    #[test]
    fn command_errors_stay_until_the_person_acts() {
        let error = (polled("error"), Hold::Sticky);
        let fresh = polled("available");
        assert_eq!(current(Some(&error), &fresh, f64::MAX).0.phase, "error");
    }
}
