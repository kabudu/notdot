//! Guided setup uses native commands; no credentials or auth URLs enter the DOM.
use super::*;
thread_local! {static TESTING: RefCell<bool> = const {RefCell::new(false)}; static WAITING: RefCell<bool> = const {RefCell::new(false)}; static BUSY: RefCell<bool> = const {RefCell::new(false)}; static DEADLINE: RefCell<f64> = const {RefCell::new(0.)};}
fn show_status(value: &Value) {
    let stage = value["stage"].as_str().unwrap_or("error");
    let focused = document().active_element();
    element("setup-status").set_text_content(value["message"].as_str());
    for (id, visible) in [
        ("setup-install", stage == "install"),
        ("setup-signin", stage == "signin"),
        ("setup-cancel", stage == "waiting"),
        ("setup-voice", stage == "ready"),
        ("setup-try", stage == "ready"),
        (
            "setup-finish",
            stage == "ready" && value["trial_passed"] == true,
        ),
    ] {
        if visible {
            let _ = element(id).remove_attribute("hidden");
        } else {
            let _ = element(id).set_attribute("hidden", "");
        }
    }
    WAITING.with(|s| *s.borrow_mut() = stage == "waiting");
    if focused.is_some_and(|e| e.has_attribute("hidden")) {
        let id = match stage {
            "install" => "setup-install",
            "signin" => "setup-signin",
            "waiting" => "setup-cancel",
            _ => "setup-voice",
        };
        let _ = element(id).unchecked_into::<web_sys::HtmlElement>().focus();
    }
}
fn show_error(message: &str) {
    WAITING.with(|s| *s.borrow_mut() = false);
    element("setup-status").set_text_content(Some(message));
    let _ = element("setup-voice").remove_attribute("hidden");
    let _ = element("setup-try").set_attribute("hidden", "");
    let _ = element("setup-finish").set_attribute("hidden", "");
}
async fn command(action: &str) -> Result<Value, String> {
    serde_wasm_bindgen::from_value(call("setup_codex", json!({"action":action})).await?)
        .map_err(|e| e.to_string())
}
pub(crate) fn open() {
    if dialog("setup-panel").open() {
        return;
    }
    BUSY.with(|s| *s.borrow_mut() = true);
    let _ = resize(450., 540.);
    if !dialog("setup-panel").open() {
        let _ = dialog("setup-panel").show_modal();
    }
    spawn_local(async {
        match command("check").await {
            Ok(v) => {
                if dialog("setup-panel").open() {
                    show_status(&v);
                }
            }
            Err(e) => show_error(&e),
        }
        BUSY.with(|s| *s.borrow_mut() = false);
    });
}
pub(crate) fn install(view: &Shared) {
    on(element("setup-open").as_ref(), "click", move |_| {
        dialog("panel").close();
        open();
    });
    on(element("export-diagnostics").as_ref(), "click", move |_| {
        spawn_local(async {
            match call("export_diagnostics", json!({})).await {
                Ok(v) => message(v.as_string().unwrap_or_default()),
                Err(e) => message(e),
            }
        });
    });
    for (id, action) in [
        ("setup-check", "check"),
        ("setup-install", "install"),
        ("setup-signin", "signin"),
        ("setup-cancel", "cancel"),
    ] {
        on(element(id).as_ref(), "click", move |_| {
            if BUSY.with(|s| s.replace(true)) {
                return;
            }
            spawn_local(async move {
                match command(action).await {
                    Ok(v) => {
                        if dialog("setup-panel").open() {
                            show_status(&v);
                        }
                    }
                    Err(e) => show_error(&e),
                }
                BUSY.with(|s| *s.borrow_mut() = false);
            });
        });
    }
    let owner = view.clone();
    on(element("setup-voice").as_ref(), "click", move |_| {
        dialog("setup-panel").close();
        open_panel(&owner);
    });
    let owner = view.clone();
    on(element("setup-try").as_ref(), "click", move |_| {
        let owner = owner.clone();
        spawn_local(async move {
            match command("trial").await {
                Ok(_) => {
                    TESTING.with(|s| *s.borrow_mut() = true);
                    DEADLINE.with(|s| *s.borrow_mut() = js_sys::Date::now() + 120_000.);
                    dialog("setup-panel").close();
                    let _ = resize(360., 360.);
                    // This explicit test button permits manual capture even if wake is selected.
                    owner.borrow_mut().snapshot.settings.onboarding_complete = true;
                    if let Err(e) = voice(owner.clone()).await {
                        TESTING.with(|s| *s.borrow_mut() = false);
                        open();
                        element("setup-status").set_text_content(Some(&e));
                    }
                }
                Err(e) => show_error(&e),
            }
        });
    });
    let owner = view.clone();
    on(element("setup-finish").as_ref(), "click", move |_| {
        let owner = owner.clone();
        spawn_local(async move {
            match command("finish").await {
                Ok(_) => {
                    TESTING.with(|s| *s.borrow_mut() = false);
                    owner.borrow_mut().snapshot.settings.onboarding_complete = true;
                    dialog("setup-panel").close();
                    let _ = resize(360., 360.);
                }
                Err(e) => show_error(&e),
            }
        });
    });
    on(element("setup-later").as_ref(), "click", move |_| {
        WAITING.with(|s| *s.borrow_mut() = false);
        TESTING.with(|s| *s.borrow_mut() = false);
        dialog("setup-panel").close();
        let _ = resize(360., 360.);
        spawn_local(async {
            let _ = command("cancel").await;
        });
    });
    on(element("setup-panel").as_ref(), "cancel", move |_| {
        WAITING.with(|s| *s.borrow_mut() = false);
        TESTING.with(|s| *s.borrow_mut() = false);
        let _ = resize(360., 360.);
        spawn_local(async {
            let _ = command("cancel").await;
        });
    });
    let owner = view.clone();
    let tick = Closure::<dyn FnMut()>::new(move || {
        let testing = TESTING.with(|s| *s.borrow());
        let waiting = WAITING.with(|s| *s.borrow());
        if testing && DEADLINE.with(|s| js_sys::Date::now() > *s.borrow()) {
            TESTING.with(|s| *s.borrow_mut() = false);
            stop_audio(&owner);
            open();
            element("setup-status").set_text_content(Some(
                "The test timed out. Check microphone access and Voice settings, then try again.",
            ));
            return;
        }
        if (!testing && !waiting) || BUSY.with(|s| s.replace(true)) {
            return;
        }
        let owner = owner.clone();
        spawn_local(async move {
            match command("poll").await {
                Ok(v) => {
                    let idle = {
                        let s = owner.borrow();
                        s.snapshot.phase == "idle"
                            && !s.voice_pending
                            && !s.speech_worker
                            && s.audio.as_ref().is_none_or(|a| a.ended())
                    };
                    if testing && v["trial_passed"] == true && idle {
                        TESTING.with(|s| *s.borrow_mut() = false);
                        let _ = resize(450., 540.);
                        let _ = dialog("setup-panel").show_modal();
                        show_status(&v);
                        element("setup-status").set_text_content(Some("Did you hear notdot answer? If so, finish setup. Otherwise, check Voice settings and try again."));
                    } else if waiting {
                        show_status(&v);
                    }
                }
                Err(e) => {
                    WAITING.with(|s| *s.borrow_mut() = false);
                    if dialog("setup-panel").open() {
                        element("setup-status").set_text_content(Some(&e));
                    }
                }
            }
            BUSY.with(|s| *s.borrow_mut() = false);
        });
    });
    let _ = window().set_interval_with_callback_and_timeout_and_arguments_0(
        tick.as_ref().unchecked_ref(),
        2000,
    );
    tick.forget();
}
pub(crate) fn trial() -> bool {
    TESTING.with(|s| *s.borrow())
}
