//! Local-only people controls. No audio or permission changes go through the model.
use super::{call, element, input, on};
use serde_json::{Value, json};
use std::cell::RefCell;
use wasm_bindgen::{JsCast, prelude::*};
use wasm_bindgen_futures::spawn_local;
use web_sys::{Element, HtmlSelectElement};

#[wasm_bindgen(
    inline_js = "export function listenSpeakerProgress(f){return window.__TAURI__.event.listen('speaker-enrol-progress',e=>f(e.payload))}"
)]
extern "C" {
    #[wasm_bindgen(catch)]
    fn listenSpeakerProgress(f: &js_sys::Function) -> Result<js_sys::Promise, JsValue>;
}
thread_local! {
    static OWNER_NAME: RefCell<String> = const { RefCell::new(String::new()) };
    static ACTIVE: RefCell<bool> = const { RefCell::new(false) };
    static EPOCH: RefCell<u64> = const { RefCell::new(0) };
    static CHAT_TITLES: RefCell<std::collections::HashMap<String,String>> = RefCell::new(std::collections::HashMap::new());
    static CAPTURE: RefCell<Option<String>> = const { RefCell::new(None) };
    static CONFIRM: RefCell<Option<(String, Value)>> = const { RefCell::new(None) };
}
fn hidden(id: &str, yes: bool) {
    let _ = element(id).toggle_attribute_with_force("hidden", yes);
}
fn status(text: &str) {
    element("speaker-status").set_text_content(Some(text));
}
fn button(
    parent: &Element,
    label: &str,
    f: impl FnMut(web_sys::Event) + 'static,
) -> Result<(), JsValue> {
    let b = web_sys::window()
        .unwrap()
        .document()
        .unwrap()
        .create_element("button")?;
    b.set_attribute("type", "button")?;
    b.set_text_content(Some(label));
    on(b.unchecked_ref(), "click", f);
    parent.append_child(&b)?;
    Ok(())
}
fn confirm(command: &str, args: Value, text: &str) {
    CONFIRM.with(|c| *c.borrow_mut() = Some((command.into(), args)));
    element("speaker-confirm-text").set_text_content(Some(text));
    hidden("speaker-confirm", false);
}
async fn command(command: &str, args: Value) {
    match call(command, args).await {
        Ok(_) => on_open(),
        Err(e) => status(&e),
    }
}
fn progress(v: &Value) {
    if !ACTIVE.with(|a| *a.borrow()) {
        return;
    }
    CAPTURE.with(|c| *c.borrow_mut() = None);
    let p = v.get("progress").unwrap_or(v);
    let step = p["step"].as_str().unwrap_or("failed");
    hidden("speaker-enrol", false);
    hidden("speaker-enrol-save", step != "ready");
    hidden("speaker-record", step == "ready" || step == "failed");
    element("speaker-phrase").set_text_content(p["phrase"].as_str());
    let text = match step {
        "ready" => "All four phrases are ready. Save this voice to finish.".into(),
        "prompt" => format!(
            "Phrase {} of {}. Select Record, read the phrase, then pause.",
            p["index"], p["of"]
        ),
        _ => p["guidance"]
            .as_str()
            .or(v["error"].as_str())
            .unwrap_or("Please cancel and try again.")
            .into(),
    };
    element("speaker-enrol-status").set_text_content(Some(&text));
    input("speaker-record").set_disabled(!input("speaker-consent").checked());
}
pub(crate) fn cancel() {
    ACTIVE.with(|a| *a.borrow_mut() = false);
    EPOCH.with(|e| *e.borrow_mut() += 1);
    let token = CAPTURE.with(|c| c.borrow_mut().take());
    CONFIRM.with(|c| *c.borrow_mut() = None);
    hidden("speaker-enrol", true);
    hidden("speaker-confirm", true);
    spawn_local(async move {
        if let Some(token) = token {
            let _ = call("cancel_capture", json!({"token":token})).await;
        }
        let _ = call("speaker_enrol_cancel", json!({})).await;
    });
}
pub(crate) fn on_open() {
    spawn_local(async {
        if let Err(e) = refresh().await {
            status(&e);
        }
    });
}
async fn refresh() -> Result<(), String> {
    let v: Value = serde_wasm_bindgen::from_value(call("speaker_summary", json!({})).await?)
        .map_err(|e| e.to_string())?;
    element("speaker-mode")
        .unchecked_into::<HtmlSelectElement>()
        .set_value(v["mode"].as_str().unwrap_or("open"));
    let owner = v["people"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["role"].as_str() == Some("owner"));
    OWNER_NAME.with(|n| {
        *n.borrow_mut() = owner
            .and_then(|p| p["name"].as_str())
            .unwrap_or("Me")
            .into()
    });
    let recovery = v["recovery_required"].as_bool() == Some(true);
    let health = v["health"]["state"].as_str().unwrap_or("idle");
    hidden("speaker-recover", !recovery);
    hidden(
        "speaker-reset-voices",
        recovery
            || !(health == "voices_unavailable"
                || (health == "model_unavailable"
                    && v["people"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|p| p["voice_enrolled"].as_bool() == Some(true)))),
    );
    let owner_ready = owner.is_some_and(|p| p["voice_enrolled"].as_bool() == Some(true));
    input("speaker-add").set_disabled(!owner_ready);
    element("speaker-add-guidance").set_text_content(Some(if owner_ready {
        "Adding a person turns on Trusted people. You can change this listening mode later."
    } else {
        "Teach your voice first so notdot can keep everyone’s context separate."
    }));
    let current = v["people"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["id"] == v["active_profile"]);
    element("speaker-active").set_text_content(Some(&format!(
        "Current profile: {}",
        current
            .and_then(|p| p["name"].as_str())
            .unwrap_or("not selected")
    )));
    hidden(
        "speaker-use-owner",
        owner.is_none_or(|p| p["id"] == v["active_profile"]),
    );
    let list = element("speaker-people");
    list.set_text_content(None);
    for person in v["people"].as_array().into_iter().flatten() {
        render_person(&list, person).map_err(|e| format!("Could not show people: {e:?}"))?;
    }
    status(if v["recovery_required"].as_bool() == Some(true) {
        "People setup needs recovery. Review the recovery option below."
    } else if health == "voices_unavailable" {
        "Saved voices are unavailable. Reset voice recognition to teach them again."
    } else if health == "model_unavailable" {
        "The voice model is unavailable. Reinstall notdot to restore it. Resetting voices will not repair the model."
    } else {
        "Changes here take effect immediately."
    });
    Ok(())
}
fn render_person(list: &Element, p: &Value) -> Result<(), JsValue> {
    let doc = web_sys::window().unwrap().document().unwrap();
    let card = doc.create_element("article")?;
    card.set_class_name("speaker-person");
    let id = p["id"].as_str().unwrap_or_default().to_owned();
    let name = p["name"].as_str().unwrap_or("Person");
    let owner = p["role"].as_str() == Some("owner");
    let title = doc.create_element("h3")?;
    title.set_text_content(Some(&format!(
        "{name}{}",
        if owner { " · you" } else { "" }
    )));
    card.append_child(&title)?;
    let note = doc.create_element("p")?;
    note.set_text_content(Some(if p["voice_enrolled"].as_bool() == Some(true) {
        "Voice saved on this computer"
    } else {
        "No voice saved"
    }));
    card.append_child(&note)?;
    let rename = doc.create_element("input")?;
    rename.set_attribute("aria-label", &format!("Name for {name}"))?;
    rename.set_attribute("maxlength", "80")?;
    rename
        .clone()
        .unchecked_into::<web_sys::HtmlInputElement>()
        .set_value(name);
    card.append_child(&rename)?;
    let actions = doc.create_element("div")?;
    actions.set_class_name("speaker-actions");
    let profile = id.clone();
    button(&actions, "Save name", move |_| {
        let name = rename
            .clone()
            .unchecked_into::<web_sys::HtmlInputElement>()
            .value();
        let profile = profile.clone();
        spawn_local(async move {
            command(
                "speaker_rename_person",
                json!({"profile":profile,"name":name}),
            )
            .await;
        });
    })?;
    let profile = id.clone();
    button(&actions, "Teach voice", move |_| {
        let profile = profile.clone();
        let epoch = EPOCH.with(|e| *e.borrow());
        spawn_local(async move {
            match call("speaker_enrol_begin", json!({"profile":profile})).await {
                Ok(v) => {
                    if EPOCH.with(|e| *e.borrow()) != epoch {
                        return;
                    }
                    ACTIVE.with(|a| *a.borrow_mut() = true);
                    input("speaker-consent").set_checked(false);
                    match serde_wasm_bindgen::from_value::<Value>(v) {
                        Ok(p) => progress(&p),
                        Err(e) => status(&e.to_string()),
                    }
                }
                Err(e) => status(&e),
            }
        });
    })?;
    if p["voice_enrolled"].as_bool() == Some(true) {
        let profile = id.clone();
        button(&actions, "Forget voice", move |_| {
            confirm(
                "speaker_forget_voice",
                json!({"profile":profile}),
                "Forget this saved voice? Their conversations and memories will remain.",
            )
        })?;
    }
    if !owner {
        let profile = id.clone();
        button(&actions, "Remove person", move |_| {
            confirm(
                "speaker_remove_person",
                json!({"profile":profile}),
                "Remove this person, their saved voice, private memories and chat access? This cannot be undone.",
            )
        })?;
    }
    card.append_child(&actions)?;
    if !owner {
        let select = doc.create_element("select")?;
        select.set_attribute("aria-label", &format!("Chat to share with {name}"))?;
        let blank = doc.create_element("option")?;
        blank.set_attribute("value", "")?;
        blank.set_text_content(Some("Load chats to choose one"));
        select.append_child(&blank)?;
        card.append_child(&select)?;
        let choices = select.clone();
        button(&card, "Load available chats", move |_| {
            let choices = choices.clone();
            spawn_local(async move {
                match call("speaker_available_chats", json!({})).await {
                    Ok(v) => {
                        let result: Value =
                            serde_wasm_bindgen::from_value(v).unwrap_or(Value::Null);
                        choices.set_text_content(None);
                        let doc = web_sys::window().unwrap().document().unwrap();
                        for chat in result["chats"].as_array().into_iter().flatten() {
                            if let (Some(id), Some(title)) =
                                (chat["id"].as_str(), chat["title"].as_str())
                            {
                                CHAT_TITLES.with(|c| {
                                    c.borrow_mut().insert(id.into(), title.into());
                                });
                            }
                            if let Ok(o) = doc.create_element("option") {
                                let _ = o.set_attribute(
                                    "value",
                                    chat["id"].as_str().unwrap_or_default(),
                                );
                                o.set_text_content(chat["title"].as_str());
                                let _ = choices.append_child(&o);
                            }
                        }
                    }
                    Err(e) => status(&e),
                }
            });
        })?;
        let profile = id.clone();
        button(&card, "Share for reading", move |_| {
            let chat = select.clone().unchecked_into::<HtmlSelectElement>().value();
            if !chat.is_empty() {
                confirm(
                    "speaker_share_chat",
                    json!({"profile":profile,"chat":chat,"access":"read"}),
                    "Allow this person to read this chat? Sending messages needs a separate permission.",
                );
            }
        })?;
        for grant in p["shared_chats"].as_array().into_iter().flatten() {
            let chat = grant["chat_id"].as_str().unwrap_or_default().to_owned();
            let row = doc.create_element("div")?;
            row.set_class_name("speaker-actions");
            let label = doc.create_element("p")?;
            label.set_text_content(Some(&format!(
                "{} · {}",
                CHAT_TITLES
                    .with(|c| c.borrow().get(&chat).cloned())
                    .unwrap_or_else(|| "Shared chat".into()),
                if grant["access"].as_str() == Some("read_write") {
                    "read and message"
                } else {
                    "read only"
                }
            )));
            row.append_child(&label)?;
            if grant["access"].as_str() == Some("read") {
                let profile = id.clone();
                let chat = chat.clone();
                button(&row, "Allow messages", move |_| {
                    confirm(
                        "speaker_share_chat",
                        json!({"profile":profile,"chat":chat,"access":"read_write"}),
                        "Allow this person to add messages to this chat? Their messages become context for the next continuation, without starting work or creating a completed turn. Continue the chat later in Codex. The main chat-control setting still applies.",
                    )
                })?;
            }
            let profile = id.clone();
            button(&row, "Stop sharing", move |_| {
                confirm(
                    "speaker_unshare_chat",
                    json!({"profile":profile,"chat":chat}),
                    "Remove this person's access to this chat?",
                )
            })?;
            card.append_child(&row)?;
        }
    }
    list.append_child(&card)?;
    Ok(())
}
pub(crate) fn install() -> Result<(), JsValue> {
    on(
        element("speaker-reset-voices").unchecked_ref(),
        "click",
        |_| {
            confirm(
                "speaker_forget_all_voices",
                json!({}),
                "Remove all saved voices and return to Anyone mode? Anyone can then use your conversation and memories. People, memories, chats and sharing are kept. You will need to teach voices again.",
            )
        },
    );
    on(element("speaker-recover").unchecked_ref(), "click", |_| {
        let name = OWNER_NAME.with(|n| n.borrow().clone());
        confirm(
            "speaker_recover",
            json!({"ownerName":name}),
            "Recover people setup and restart notdot? This removes saved voices, people and sharing, and returns to Anyone mode. Your history and memories are kept. Previous member folders remain on disk. Anyone can use your context until you teach your voice again.",
        );
    });
    on(
        element("speaker-use-owner").unchecked_ref(),
        "click",
        |_| {
            spawn_local(async {
                command("speaker_use_owner", json!({})).await;
            })
        },
    );
    on(element("speaker-add").unchecked_ref(), "click", |_| {
        let name = input("speaker-new-name").value();
        spawn_local(async move {
            command("speaker_add_person", json!({"name":name})).await;
        });
    });
    on(
        element("speaker-mode-save").unchecked_ref(),
        "click",
        |_| {
            let mode = element("speaker-mode")
                .unchecked_into::<HtmlSelectElement>()
                .value();
            confirm(
                "speaker_set_mode",
                json!({"mode":mode}),
                "Apply this listening mode? Anyone uses your context; other modes require saved voices. Voice matching is not authentication.",
            );
        },
    );
    on(
        element("speaker-confirm-yes").unchecked_ref(),
        "click",
        |_| {
            let action = CONFIRM.with(|c| c.borrow_mut().take());
            hidden("speaker-confirm", true);
            if let Some((cmd, args)) = action {
                spawn_local(async move {
                    command(&cmd, args).await;
                });
            }
        },
    );
    on(
        element("speaker-confirm-no").unchecked_ref(),
        "click",
        |_| {
            CONFIRM.with(|c| *c.borrow_mut() = None);
            hidden("speaker-confirm", true);
        },
    );
    on(element("speaker-consent").unchecked_ref(), "change", |_| {
        input("speaker-record").set_disabled(!input("speaker-consent").checked())
    });
    on(
        element("speaker-enrol-cancel").unchecked_ref(),
        "click",
        |_| cancel(),
    );
    on(
        element("speaker-enrol-save").unchecked_ref(),
        "click",
        |_| {
            if !input("speaker-consent").checked() {
                return;
            }
            spawn_local(async {
                match call("speaker_enrol_save", json!({})).await {
                    Ok(_) => {
                        hidden("speaker-enrol", true);
                        ACTIVE.with(|a| *a.borrow_mut() = false);
                        on_open();
                    }
                    Err(e) => status(&e),
                }
            });
        },
    );
    on(element("speaker-record").unchecked_ref(), "click", |_| {
        if !input("speaker-consent").checked() {
            return;
        }
        input("speaker-record").set_disabled(true);
        element("speaker-enrol-status")
            .set_text_content(Some("Listening. Read the phrase, then pause."));
        let token = format!("speaker-enrol-{}", js_sys::Date::now());
        CAPTURE.with(|c| *c.borrow_mut() = Some(token.clone()));
        spawn_local(async move {
            let result = async {
                call("speaker_enrol_capture", json!({"token":token})).await?;
                call(
                    "start_capture",
                    json!({"token":token,"limit":12,"followup":false}),
                )
                .await?;
                Ok::<(), String>(())
            }
            .await;
            if let Err(e) = result {
                status(&e);
                cancel();
            }
        });
    });
    let f = Closure::<dyn FnMut(JsValue)>::new(|v| {
        if let Ok(v) = serde_wasm_bindgen::from_value::<Value>(v) {
            if !CAPTURE.with(|c| {
                c.borrow()
                    .as_deref()
                    .is_some_and(|token| v["token"].as_str() == Some(token))
            }) {
                return;
            }
            progress(&v);
        }
    });
    let _ = listenSpeakerProgress(f.as_ref().unchecked_ref())?;
    f.forget();
    Ok(())
}
