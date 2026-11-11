//! Settings review for spoken notes and the encrypted backup controls.
//! Every action is checked natively against the active person; this view
//! only shows what the native side returns.
use crate::{call, document, element, input, js_error, on};
use serde_json::{Value, json};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::spawn_local;

fn status(id: &str, text: &str) {
    element(id).set_text_content(Some(text));
}
fn child(
    parent: &web_sys::Element,
    tag: &str,
    text: Option<&str>,
) -> Result<web_sys::Element, String> {
    let node = document().create_element(tag).map_err(js_error)?;
    if let Some(text) = text {
        node.set_text_content(Some(text));
    }
    parent.append_child(&node).map_err(js_error)?;
    Ok(node)
}
fn button(
    parent: &web_sys::Element,
    text: &str,
    class: Option<&str>,
) -> Result<web_sys::Element, String> {
    let b = child(parent, "button", Some(text))?;
    b.set_attribute("type", "button").map_err(js_error)?;
    if let Some(class) = class {
        b.set_class_name(class);
    }
    Ok(b)
}
fn focus(node: &web_sys::Element) {
    if let Some(node) = node.dyn_ref::<web_sys::HtmlElement>() {
        let _ = node.focus();
    }
}

pub async fn review() -> Result<(), String> {
    crate::settings_navigation::reveal("voice-notes-section");
    let value = call("notes_list", json!({})).await?;
    let book: Value = serde_wasm_bindgen::from_value(value).map_err(|e| e.to_string())?;
    let container = element("voice-notes");
    container.set_text_content(None);
    let notes = book["notes"].as_array().cloned().unwrap_or_default();
    if notes.is_empty() {
        child(
            &container,
            "p",
            Some("No notes or lists yet. Try saying “Make a shopping list”."),
        )?
        .set_class_name("hint");
        return Ok(());
    }
    for note in notes.iter().rev() {
        render(&container, note)?;
    }
    Ok(())
}

fn render(container: &web_sys::Element, note: &Value) -> Result<(), String> {
    let id = note["id"].as_str().ok_or("Note ID missing")?.to_owned();
    let title = note["title"].as_str().unwrap_or("Untitled").to_owned();
    let list = note["kind"] == "list";
    let card = child(container, "article", None)?;
    card.set_attribute(
        "aria-label",
        &format!("{title} {}", if list { "list" } else { "note" }),
    )
    .map_err(js_error)?;
    child(&card, "h4", Some(&title))?;
    if list {
        let items = note["items"].as_array().cloned().unwrap_or_default();
        if items.is_empty() {
            child(&card, "p", Some("Empty list"))?.set_class_name("hint");
        } else {
            let ul = child(&card, "ul", None)?;
            for item in items {
                let done = item["done"] == true;
                let li = child(&ul, "li", item["text"].as_str())?;
                if done {
                    li.set_class_name("done");
                    li.set_attribute(
                        "aria-label",
                        &format!("{}, done", item["text"].as_str().unwrap_or("")),
                    )
                    .map_err(js_error)?;
                }
            }
        }
    } else {
        // Shown as text, never as HTML.
        let body = child(&card, "p", note["body"].as_str())?;
        body.set_class_name("note-body");
    }
    let actions = child(&card, "div", None)?;
    actions.set_class_name("tool-actions");
    let export = button(&actions, "Export…", None)?;
    let delete = button(&actions, "Delete…", Some("danger-action"))?;
    let confirm = child(&card, "section", None)?;
    confirm.set_class_name("reset-confirm");
    confirm.set_attribute("hidden", "").map_err(js_error)?;
    confirm.set_attribute("role", "group").map_err(js_error)?;
    confirm
        .set_attribute("aria-label", &format!("Delete {title}?"))
        .map_err(js_error)?;
    child(
        &confirm,
        "p",
        Some(&format!("Delete {title}? This can’t be undone.")),
    )?;
    let row = child(&confirm, "div", None)?;
    row.set_class_name("tool-actions");
    let keep = button(&row, "Keep it", None)?;
    let remove = button(
        &row,
        &format!("Delete {}", if list { "list" } else { "note" }),
        Some("danger-action"),
    )?;
    {
        let id = id.clone();
        on(export.as_ref(), "click", move |_| {
            let id = id.clone();
            spawn_local(async move {
                let text = match call("notes_export", json!({"id":id})).await {
                    Ok(value) => value.as_string().unwrap_or_default(),
                    Err(e) => e,
                };
                status("voice-notes-status", &text);
            });
        });
    }
    {
        let confirm = confirm.clone();
        let keep = keep.clone();
        on(delete.as_ref(), "click", move |_| {
            let _ = confirm.remove_attribute("hidden");
            focus(&keep);
        });
    }
    {
        let confirm = confirm.clone();
        let delete = delete.clone();
        on(keep.as_ref(), "click", move |_| {
            let _ = confirm.set_attribute("hidden", "");
            focus(&delete);
        });
    }
    on(remove.as_ref(), "click", move |_| {
        let id = id.clone();
        let title = title.clone();
        spawn_local(async move {
            match call("notes_delete", json!({"id":id})).await {
                Ok(_) => {
                    status("voice-notes-status", &format!("Deleted {title}."));
                    if let Err(e) = review().await {
                        status("voice-notes-status", &e);
                    }
                    focus(&element("review-voice-notes"));
                }
                Err(e) => status("voice-notes-status", &e),
            }
        });
    });
    Ok(())
}

fn take_password(id: &str) -> String {
    let field = input(id);
    let value = field.value();
    field.set_value("");
    value
}
fn busy(on: bool) {
    for id in ["backup-save", "backup-open", "backup-apply"] {
        let _ = if on {
            element(id).set_attribute("disabled", "")
        } else {
            element(id).remove_attribute("disabled")
        };
    }
}
fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}
fn describe(preview: &Value) -> String {
    let n = |k: &str| preview[k].as_u64().unwrap_or(0);
    let mut text = format!(
        "This adds {} and {}.",
        plural(n("memories"), "memory", "memories"),
        plural(n("notes"), "note", "notes")
    );
    if n("skipped_memories") > 0 {
        text.push_str(&format!(
            " {} already here will be skipped.",
            plural(n("skipped_memories"), "memory", "memories")
        ));
    }
    if n("renamed_notes") > 0 {
        text.push_str(&format!(
            " {} with names already in use will be added with “(restored)”.",
            plural(n("renamed_notes"), "note", "notes")
        ));
    }
    if preview["preferences"] == true {
        text.push_str(" Your colour and reminder limits will be set from the backup.");
    }
    let made = js_sys::Date::new(&JsValue::from_f64(
        preview["created_unix_ms"].as_f64().unwrap_or(0.),
    ));
    text.push_str(&format!(
        " Backup made {}.",
        made.to_locale_date_string("en-GB", &JsValue::UNDEFINED)
    ));
    text
}

pub fn install() {
    on(element("review-voice-notes").as_ref(), "click", |_| {
        spawn_local(async {
            if let Err(e) = review().await {
                status("voice-notes-status", &e);
            }
        });
    });
    on(element("export-voice-notes").as_ref(), "click", |_| {
        spawn_local(async {
            let text = match call("notes_export", json!({"id":null})).await {
                Ok(value) => value.as_string().unwrap_or_default(),
                Err(e) => e,
            };
            status("voice-notes-status", &text);
        });
    });
    on(element("backup-save").as_ref(), "click", |_| {
        let password = take_password("backup-password");
        let repeat = take_password("backup-password-repeat");
        if password.chars().count() < 12 {
            status(
                "backup-status",
                "Use a backup password of at least 12 characters.",
            );
            focus(&element("backup-password"));
            return;
        }
        if password != repeat {
            status(
                "backup-status",
                "The two passwords don’t match. Nothing was saved.",
            );
            focus(&element("backup-password"));
            return;
        }
        busy(true);
        status("backup-status", "Choose where to save the backup…");
        spawn_local(async move {
            let text = match call("backup_export", json!({"password":password})).await {
                Ok(value) => value.as_string().unwrap_or_default(),
                Err(e) => e,
            };
            busy(false);
            status("backup-status", &text);
        });
    });
    on(element("backup-open").as_ref(), "click", |_| {
        let password = take_password("backup-password");
        let _ = take_password("backup-password-repeat");
        if password.is_empty() {
            status("backup-status", "Enter the backup’s password first.");
            focus(&element("backup-password"));
            return;
        }
        busy(true);
        let _ = element("backup-confirm").set_attribute("hidden", "");
        status("backup-status", "Choose a backup to check…");
        spawn_local(async move {
            let result = call("backup_inspect", json!({"password":password})).await;
            busy(false);
            match result {
                Ok(value) => {
                    let preview: Value =
                        serde_wasm_bindgen::from_value(value).unwrap_or(Value::Null);
                    let Some(id) = preview["id"].as_str() else {
                        status("backup-status", "Restore cancelled. Nothing was changed.");
                        return;
                    };
                    let _ = element("backup-apply").set_attribute("data-restore-id", id);
                    element("backup-preview").set_text_content(Some(&describe(&preview)));
                    let _ = element("backup-confirm").remove_attribute("hidden");
                    status("backup-status", "");
                    focus(&element("backup-keep"));
                }
                Err(e) => status("backup-status", &e),
            }
        });
    });
    on(element("backup-keep").as_ref(), "click", |_| {
        let _ = element("backup-confirm").set_attribute("hidden", "");
        let _ = element("backup-apply").remove_attribute("data-restore-id");
        spawn_local(async {
            let _ = call("backup_cancel", json!({})).await;
        });
        status("backup-status", "Nothing was restored.");
        focus(&element("backup-open"));
    });
    on(element("backup-apply").as_ref(), "click", |_| {
        let Some(id) = element("backup-apply").get_attribute("data-restore-id") else {
            return;
        };
        let _ = element("backup-apply").remove_attribute("data-restore-id");
        busy(true);
        status("backup-status", "Restoring…");
        spawn_local(async move {
            let text = match call("backup_restore", json!({"id":id})).await {
                Ok(value) => value.as_string().unwrap_or_default(),
                Err(e) => e,
            };
            busy(false);
            let _ = element("backup-confirm").set_attribute("hidden", "");
            status("backup-status", &text);
            focus(&element("backup-open"));
        });
    });
}
