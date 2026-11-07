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
    /// When shared-chat titles were last looked up (ms), and whether a lookup is running.
    static TITLE_LOOKUP: RefCell<(f64, bool)> = const { RefCell::new((f64::NEG_INFINITY, false)) };
    static CAPTURE: RefCell<Option<String>> = const { RefCell::new(None) };
    static CONFIRM: RefCell<Option<(String, Value)>> = const { RefCell::new(None) };
    /// Name the current confirmation must be typed as, if any.
    static CONFIRM_NAME: RefCell<Option<String>> = const { RefCell::new(None) };
    /// Person whose voice is being taught, and whether it is the owner.
    static ENROL_FOR: RefCell<(String, bool)> = const { RefCell::new((String::new(), false)) };
    /// Pending self-removal: (profile, name).
    static LEAVING: RefCell<Option<(String, String)>> = const { RefCell::new(None) };
    /// Outcome to show after the next refresh, instead of the default line.
    static NOTICE: RefCell<Option<String>> = const { RefCell::new(None) };
}
fn notice_after_refresh(text: String) {
    NOTICE.with(|n| *n.borrow_mut() = Some(text));
    on_open();
}
fn same_name(typed: &str, name: &str) -> bool {
    let norm = |s: &str| {
        s.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    !typed.trim().is_empty() && norm(typed) == norm(name)
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
    CONFIRM_NAME.with(|c| *c.borrow_mut() = None);
    element("speaker-confirm-text").set_text_content(Some(text));
    hidden("speaker-confirm-named", true);
    input("speaker-confirm-yes").set_disabled(false);
    hidden("speaker-confirm", false);
    // The box sits at the end of People; bring it to the button that asked.
    element("speaker-confirm").scroll_into_view();
    let _ = input("speaker-confirm-yes").focus();
}
/// A confirmation that only enables once `name` is typed. The typed text is
/// sent as `confirmName`; notdot checks it again natively.
fn confirm_named(command: &str, args: Value, text: &str, name: &str) {
    confirm(command, args, text);
    CONFIRM_NAME.with(|c| *c.borrow_mut() = Some(name.into()));
    element("speaker-confirm-name-label")
        .set_text_content(Some(&format!("Type {name} to confirm")));
    input("speaker-confirm-name").set_value("");
    hidden("speaker-confirm-named", false);
    input("speaker-confirm-yes").set_disabled(true);
    let _ = input("speaker-confirm-name").focus();
}
/// Plain account of what a removal actually did.
fn removal_report(v: &Value) -> String {
    let name = v["name"].as_str().unwrap_or("This person");
    let own = v["route"].as_str() == Some("self_service");
    let mut lines = vec![if own {
        format!("{name}'s profile was removed.")
    } else {
        format!("{name} was removed. They can't use notdot by voice any more.")
    }];
    lines.push(match (v["data"].as_str(), v["voice"].as_str()) {
        (Some("deleted"), Some("deleted")) => "Their saved voice and private memories on this computer were deleted.".into(),
        (Some("deleted"), _) => "Their private memories were deleted. Their saved voice is no longer used and will be deleted the next time voices load.".into(),
        (_, Some("deleted")) => "Their saved voice was deleted. Their private memories will finish deleting when notdot restarts.".into(),
        _ => "Their saved voice is no longer used and their private memories will finish deleting when notdot restarts.".into(),
    });
    match v["chat"].as_str() {
        Some("archived") => lines.push("Their own notdot chat was archived in Codex.".into()),
        Some("pending") => lines.push("Codex wasn't available, so notdot will keep trying to archive their own chat. It's already hidden everywhere in notdot.".into()),
        _ => {}
    }
    lines.push("Messages they added to shared chats stay in those chats.".into());
    lines.join(" ")
}
async fn command(command: &str, args: Value) -> bool {
    let removal = matches!(
        command,
        "speaker_remove_person" | "speaker_confirm_self_removal"
    );
    match call(command, args).await {
        Ok(v) if removal => {
            let report: Value = serde_wasm_bindgen::from_value(v).unwrap_or(Value::Null);
            notice_after_refresh(removal_report(&report));
            true
        }
        Ok(_) => {
            on_open();
            true
        }
        Err(e) => {
            status(&e);
            false
        }
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
    let (name, owner) = ENROL_FOR.with(|e| e.borrow().clone());
    let whose = if owner {
        "your".to_owned()
    } else {
        format!("{name}'s")
    };
    let text = match step {
        "ready" => format!("All four phrases are ready. Save {whose} voice to finish."),
        "prompt" => format!(
            "Phrase {} of {} for {}. Select Record, read the phrase, then pause.",
            p["index"],
            p["of"],
            if owner { "you" } else { name.as_str() }
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
/// A phrase is being recorded for enrolment. The microphone lease must be
/// renewed for it although Settings is open.
pub(crate) fn recording() -> bool {
    CAPTURE.with(|c| c.borrow().is_some())
}
pub(crate) fn cancel() {
    ACTIVE.with(|a| *a.borrow_mut() = false);
    EPOCH.with(|e| *e.borrow_mut() += 1);
    let token = CAPTURE.with(|c| c.borrow_mut().take());
    CONFIRM.with(|c| *c.borrow_mut() = None);
    hidden("speaker-enrol", true);
    hidden("speaker-confirm", true);
    CONFIRM_NAME.with(|c| *c.borrow_mut() = None);
    spawn_local(async move {
        if let Some(token) = token {
            let _ = call("cancel_capture", json!({"token":token})).await;
        }
        let _ = call("speaker_enrol_cancel", json!({})).await;
    });
}
fn doc() -> web_sys::Document {
    web_sys::window().unwrap().document().unwrap()
}
/// "Weekend plans · read only". Until the title is known: "Shared chat".
fn grant_label(chat: &str, access: &str) -> String {
    format!(
        "{} · {}",
        CHAT_TITLES
            .with(|c| c.borrow().get(chat).cloned())
            .unwrap_or_else(|| "Shared chat".into()),
        if access == "read_write" {
            "read and message"
        } else {
            "read only"
        }
    )
}
fn remember_titles(result: &Value) {
    for chat in result["chats"].as_array().into_iter().flatten() {
        if let (Some(id), Some(title)) = (chat["id"].as_str(), chat["title"].as_str()) {
            CHAT_TITLES.with(|c| {
                c.borrow_mut().insert(id.into(), title.into());
            });
        }
    }
}
/// Shared chats are stored by ID only. Look their titles up in the
/// background (one lookup at a time, at most once a minute) and relabel
/// the rows in place. Failures keep the generic label silently.
fn fill_chat_titles() {
    let labels = doc()
        .query_selector_all("#speaker-people .speaker-grant-title")
        .ok();
    let missing = labels.as_ref().is_some_and(|l| {
        (0..l.length()).filter_map(|i| l.item(i)).any(|n| {
            n.dyn_ref::<Element>()
                .and_then(|e| e.get_attribute("data-chat"))
                .is_some_and(|chat| CHAT_TITLES.with(|c| !c.borrow().contains_key(&chat)))
        })
    });
    let now = js_sys::Date::now();
    let start = missing
        && TITLE_LOOKUP.with(|t| {
            let mut t = t.borrow_mut();
            if t.1 || now - t.0 < 60_000.0 {
                return false;
            }
            *t = (now, true);
            true
        });
    if !start {
        return;
    }
    spawn_local(async {
        if let Ok(v) = call("speaker_available_chats", json!({})).await {
            remember_titles(&serde_wasm_bindgen::from_value(v).unwrap_or(Value::Null));
        }
        TITLE_LOOKUP.with(|t| t.borrow_mut().1 = false);
        let Ok(labels) = doc().query_selector_all("#speaker-people .speaker-grant-title") else {
            return;
        };
        for e in (0..labels.length())
            .filter_map(|i| labels.item(i))
            .filter_map(|n| n.dyn_into::<Element>().ok())
        {
            if let (Some(chat), Some(access)) =
                (e.get_attribute("data-chat"), e.get_attribute("data-access"))
            {
                e.set_text_content(Some(&grant_label(&chat, &access)));
            }
        }
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
    let pending = v["removed_chats"]["pending"].as_u64().unwrap_or(0);
    hidden("speaker-removed-chats", pending == 0);
    element("speaker-removed-chats").set_text_content(Some(&format!(
        "notdot is still archiving {pending} removed {} in Codex and will keep trying. {} hidden everywhere in notdot.",
        if pending == 1 { "person's chat" } else { "people's chats" },
        if pending == 1 { "It's already" } else { "They're already" }
    )));
    render_leaving(&v);
    let list = element("speaker-people");
    list.set_text_content(None);
    for person in v["people"].as_array().into_iter().flatten() {
        render_person(&list, person).map_err(|e| format!("Could not show people: {e:?}"))?;
    }
    fill_chat_titles();
    let notice = NOTICE.with(|n| n.borrow_mut().take());
    status(&if v["recovery_required"].as_bool() == Some(true) {
        "People setup needs recovery. Review the recovery option below.".to_owned()
    } else if health == "voices_unavailable" {
        "Saved voices are unavailable. Reset voice recognition to teach them again.".to_owned()
    } else if health == "model_unavailable" {
        "The voice model is unavailable. Reinstall notdot to restore it. Resetting voices will not repair the model.".to_owned()
    } else {
        notice.unwrap_or_else(|| "Changes here take effect immediately.".to_owned())
    });
    Ok(())
}
/// A member asked by voice to remove their own profile.
fn render_leaving(v: &Value) {
    let pending = &v["self_removal"];
    let profile = pending["profile"].as_str().unwrap_or_default().to_owned();
    let name = v["people"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["id"].as_str() == Some(profile.as_str()))
        .and_then(|p| p["name"].as_str())
        .unwrap_or_default()
        .to_owned();
    if profile.is_empty() || name.is_empty() {
        LEAVING.with(|l| *l.borrow_mut() = None);
        hidden("speaker-leave", true);
        return;
    }
    let ready = pending["stage"].as_str() == Some("awaiting_local_confirmation");
    element("speaker-leave-title")
        .set_text_content(Some(&format!("{name} asked to remove their profile")));
    element("speaker-leave-text").set_text_content(Some(&if ready {
        format!("{name}, removing your profile:\n• deletes your saved voice and private memories on this computer\n• archives your own notdot chat in Codex\n• ends access to any chats shared with you. Messages you added there stay.\nThis can't be undone.")
    } else {
        format!("Waiting for {name} to say the code notdot gave them (about {} seconds left). Nothing has been deleted.", pending["expires_in_s"].as_u64().unwrap_or(0))
    }));
    element("speaker-leave-name-label")
        .set_text_content(Some(&format!("{name}, type your name to confirm")));
    hidden("speaker-leave-confirm", !ready);
    hidden("speaker-leave-yes", !ready);
    input("speaker-leave-yes").set_disabled(true);
    input("speaker-leave-name").set_value("");
    LEAVING.with(|l| *l.borrow_mut() = Some((profile, name)));
    hidden("speaker-leave", false);
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
    note.set_class_name("hint speaker-voice-state");
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
    let needs_name = p["needs_name"].as_bool() == Some(true);
    if needs_name {
        rename.set_attribute("placeholder", "Your name")?;
        rename
            .clone()
            .unchecked_into::<web_sys::HtmlInputElement>()
            .set_value("");
    }
    card.append_child(&rename)?;
    if needs_name {
        let hint = doc.create_element("p")?;
        hint.set_class_name("hint");
        hint.set_text_content(Some(
            "What should notdot call you? Type your name, then teach your voice.",
        ));
        card.append_child(&hint)?;
    }
    let actions = doc.create_element("div")?;
    actions.set_class_name("speaker-actions");
    let typed = rename.clone();
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
    let person = name.to_owned();
    button(&actions, "Teach voice", move |_| {
        let profile = profile.clone();
        let typed = typed
            .clone()
            .unchecked_into::<web_sys::HtmlInputElement>()
            .value();
        if needs_name && typed.trim().is_empty() {
            status("Type what notdot should call you, then select Teach voice.");
            return;
        }
        let person = if needs_name {
            typed.trim().to_owned()
        } else {
            person.clone()
        };
        let epoch = EPOCH.with(|e| *e.borrow());
        spawn_local(async move {
            let args = if needs_name {
                json!({"profile":profile,"name":person})
            } else {
                json!({"profile":profile})
            };
            match call("speaker_enrol_begin", args).await {
                Ok(v) => {
                    if EPOCH.with(|e| *e.borrow()) != epoch {
                        return;
                    }
                    ACTIVE.with(|a| *a.borrow_mut() = true);
                    ENROL_FOR.with(|e| *e.borrow_mut() = (person.clone(), owner));
                    element("speaker-enrol-title").set_text_content(Some(&if owner {
                        "Teach notdot your voice".to_owned()
                    } else {
                        format!("Teach notdot {person}'s voice")
                    }));
                    element("speaker-consent-text").set_text_content(Some(&if owner {
                        "I agree that notdot can recognise my voice on this computer. My voiceprint stays on this computer, encrypted.".to_owned()
                    } else {
                        format!("{person} is here with me and agrees that notdot can recognise their voice on this computer. The voiceprint stays on this computer, encrypted.")
                    }));
                    input("speaker-consent").set_checked(false);
                    match serde_wasm_bindgen::from_value::<Value>(v) {
                        Ok(p) => progress(&p),
                        Err(e) => status(&e.to_string()),
                    }
                    // The panel sits below the people list; bring it into view.
                    element("speaker-enrol").scroll_into_view();
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
        let person = name.to_owned();
        let shared = p["shared_chats"].as_array().map_or(0, Vec::len);
        button(&actions, &format!("Remove {name}"), move |_| {
            let shares = match shared {
                0 => String::new(),
                1 => format!(
                    "\n• {person} loses access to the chat you shared. Messages {person} already added stay in it."
                ),
                n => format!(
                    "\n• {person} loses access to the {n} chats you shared. Messages {person} already added stay in them."
                ),
            };
            confirm_named(
                "speaker_remove_person",
                json!({"profile":profile}),
                &format!(
                    "Remove {person} from notdot?\n• From now on, {person} can't use notdot by voice. Anything {person} asked for that's still waiting is cancelled.\n• {person}'s saved voice and private memories on this computer are deleted.\n• {person}'s own notdot chat is archived in Codex and can no longer be opened from notdot.{shares}\nThis can't be undone."
                ),
                &person,
            )
        })?;
    }
    card.append_child(&actions)?;
    if !owner {
        let share = doc.create_element("div")?;
        share.set_class_name("speaker-share");
        let heading = doc.create_element("p")?;
        heading.set_class_name("speaker-share-title");
        heading.set_text_content(Some("Shared chats"));
        share.append_child(&heading)?;
        let select = doc.create_element("select")?;
        select.set_attribute("aria-label", &format!("Chat to share with {name}"))?;
        let blank = doc.create_element("option")?;
        blank.set_attribute("value", "")?;
        blank.set_text_content(Some("Load chats to choose one"));
        select.append_child(&blank)?;
        share.append_child(&select)?;
        let share_actions = doc.create_element("div")?;
        share_actions.set_class_name("speaker-actions");
        let choices = select.clone();
        button(&share_actions, "Load available chats", move |_| {
            let choices = choices.clone();
            spawn_local(async move {
                match call("speaker_available_chats", json!({})).await {
                    Ok(v) => {
                        let result: Value =
                            serde_wasm_bindgen::from_value(v).unwrap_or(Value::Null);
                        remember_titles(&result);
                        choices.set_text_content(None);
                        let doc = web_sys::window().unwrap().document().unwrap();
                        for chat in result["chats"].as_array().into_iter().flatten() {
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
        button(&share_actions, "Share for reading", move |_| {
            let chat = select.clone().unchecked_into::<HtmlSelectElement>().value();
            if !chat.is_empty() {
                confirm(
                    "speaker_share_chat",
                    json!({"profile":profile,"chat":chat,"access":"read"}),
                    "Allow this person to read this chat? Sending messages needs a separate permission.",
                );
            }
        })?;
        share.append_child(&share_actions)?;
        for grant in p["shared_chats"].as_array().into_iter().flatten() {
            let chat = grant["chat_id"].as_str().unwrap_or_default().to_owned();
            let item = doc.create_element("div")?;
            item.set_class_name("speaker-grant");
            let row = doc.create_element("div")?;
            row.set_class_name("speaker-actions");
            let access = grant["access"].as_str().unwrap_or("read");
            let label = doc.create_element("p")?;
            label.set_class_name("speaker-grant-title");
            label.set_attribute("data-chat", &chat)?;
            label.set_attribute("data-access", access)?;
            label.set_text_content(Some(&grant_label(&chat, access)));
            item.append_child(&label)?;
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
            item.append_child(&row)?;
            share.append_child(&item)?;
        }
        card.append_child(&share)?;
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
            if command("speaker_add_person", json!({"name":name})).await {
                input("speaker-new-name").set_value("");
            }
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
            let required = CONFIRM_NAME.with(|c| c.borrow().clone());
            let typed = input("speaker-confirm-name").value();
            if required
                .as_deref()
                .is_some_and(|name| !same_name(&typed, name))
            {
                return;
            }
            let action = CONFIRM.with(|c| c.borrow_mut().take());
            CONFIRM_NAME.with(|c| *c.borrow_mut() = None);
            hidden("speaker-confirm", true);
            if let Some((cmd, mut args)) = action {
                if required.is_some() {
                    args["confirmName"] = json!(typed);
                }
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
            CONFIRM_NAME.with(|c| *c.borrow_mut() = None);
            hidden("speaker-confirm", true);
        },
    );
    on(
        element("speaker-confirm-name").unchecked_ref(),
        "input",
        |_| {
            let ok = CONFIRM_NAME.with(|c| {
                c.borrow()
                    .as_deref()
                    .is_none_or(|name| same_name(&input("speaker-confirm-name").value(), name))
            });
            input("speaker-confirm-yes").set_disabled(!ok);
        },
    );
    on(
        element("speaker-leave-name").unchecked_ref(),
        "input",
        |_| {
            let ok = LEAVING.with(|l| {
                l.borrow()
                    .as_ref()
                    .is_some_and(|(_, name)| same_name(&input("speaker-leave-name").value(), name))
            });
            input("speaker-leave-yes").set_disabled(!ok);
        },
    );
    on(
        element("speaker-leave-yes").unchecked_ref(),
        "click",
        |_| {
            let Some((profile, name)) = LEAVING.with(|l| l.borrow().clone()) else {
                return;
            };
            let typed = input("speaker-leave-name").value();
            if !same_name(&typed, &name) {
                return;
            }
            hidden("speaker-leave", true);
            spawn_local(async move {
                command(
                    "speaker_confirm_self_removal",
                    json!({"profile":profile,"confirmName":typed}),
                )
                .await;
            });
        },
    );
    on(
        element("speaker-leave-cancel").unchecked_ref(),
        "click",
        |_| {
            hidden("speaker-leave", true);
            spawn_local(async {
                match call("speaker_cancel_self_removal", json!({})).await {
                    Ok(_) => {
                        notice_after_refresh("The removal was stopped. Nothing was deleted.".into())
                    }
                    Err(e) => status(&e),
                }
            });
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
                match call("speaker_enrol_save", json!({"consent":true})).await {
                    Ok(_) => {
                        hidden("speaker-enrol", true);
                        ACTIVE.with(|a| *a.borrow_mut() = false);
                        let (name, owner) = ENROL_FOR.with(|e| e.borrow().clone());
                        notice_after_refresh(if owner {
                            "Got it. notdot will know your voice now.".to_owned()
                        } else {
                            format!("Got it. notdot will know {name}'s voice now.")
                        });
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
