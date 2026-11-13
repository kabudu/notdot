//! Local owner controls only. Phone names and pairing codes are rendered as
//! text, never HTML. Results are discarded when Settings closes or changes person.
use super::{call, document, element, input, js_error, notes_panel, on};
use serde_json::{Value, json};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::{HtmlDialogElement, HtmlSelectElement};

fn status(text: &str) {
    element("phones-message").set_text_content(Some(text));
}
fn child(parent: &web_sys::Element, tag: &str, text: &str) -> Result<web_sys::Element, String> {
    let node = document().create_element(tag).map_err(js_error)?;
    node.set_text_content(Some(text));
    parent.append_child(&node).map_err(js_error)?;
    Ok(node)
}
async fn invoke(command: &str, args: Value, generation: u64) {
    match call(command, args).await {
        Ok(_) => {
            if notes_panel::generation() == generation {
                refresh().await;
            }
        }
        Err(e) => {
            if notes_panel::generation() == generation {
                status(&e);
            }
        }
    }
}
async fn refresh() {
    let generation = notes_panel::generation();
    let result = call("phones_status", json!({})).await;
    if generation != notes_panel::generation() {
        return;
    }
    match result.and_then(|v| serde_wasm_bindgen::from_value::<Value>(v).map_err(|e| e.to_string()))
    {
        Err(e) => status(&e),
        Ok(v) => {
            if let Err(e) = render(&v) {
                status(&e);
            }
        }
    }
}
fn button(
    parent: &web_sys::Element,
    label: &str,
    command: &'static str,
    args: Value,
    confirm: bool,
) -> Result<(), String> {
    let b = child(parent, "button", label)?;
    b.set_attribute("type", "button").map_err(js_error)?;
    on(b.as_ref(), "click", move |_| {
        if confirm {
            let dialog = element("phones-confirm");
            element("phones-confirm-title").set_text_content(Some(
                if command == "phones_sensitive" {
                    "Allow sensitive actions?"
                } else {
                    "Remove this phone?"
                },
            ));
            element("phones-confirm-allow").set_text_content(Some(
                if command == "phones_sensitive" {
                    "Allow for 15 minutes"
                } else {
                    "Remove phone"
                },
            ));
            let _ = dialog.set_attribute("data-command", command);
            let _ = dialog.set_attribute("data-args", &args.to_string());
            element("phones-confirm-text").set_text_content(Some(if command == "phones_sensitive" { "Allow this owner's phone to erase assistant memories for 15 minutes? Every erase still needs confirmation and a fingerprint or face on that phone." } else { "Disconnect this phone and remove its access to notdot? It will need to pair again." }));
            let _ = dialog.dyn_into::<HtmlDialogElement>().unwrap().show_modal();
        } else {
            let args = args.clone();
            let generation = notes_panel::generation();
            spawn_local(async move {
                invoke(command, args, generation).await;
            });
        }
    });
    Ok(())
}
fn render(v: &Value) -> Result<(), String> {
    let available = v["available"].as_bool().unwrap_or(false);
    for id in [
        "phones-enable",
        "phones-offer",
        "phones-save-policy",
        "phones-revocations",
    ] {
        if available {
            let _ = element(id).remove_attribute("disabled");
        } else {
            let _ = element(id).set_attribute("disabled", "");
        }
    }
    if !available {
        status(
            v["reason"]
                .as_str()
                .unwrap_or("Phones are unavailable in this build"),
        );
        return Ok(());
    }
    input("phones-enable").set_checked(v["settings"]["enabled"].as_bool().unwrap_or(false));
    status(v["error"].as_str().unwrap_or(if v["running"] == true {
        "Ready for phones on the same network."
    } else {
        "Phone connections are off."
    }));
    let select: HtmlSelectElement = element("phones-profile").dyn_into().unwrap();
    let selected = select.value();
    select.set_text_content(None);
    for person in v["profiles"].as_array().into_iter().flatten() {
        let option = child(
            select.as_ref(),
            "option",
            person["name"].as_str().unwrap_or("Person"),
        )?;
        option
            .set_attribute("value", person["id"].as_str().unwrap_or(""))
            .map_err(js_error)?;
    }
    if !selected.is_empty() {
        select.set_value(&selected);
    }
    if element("phones-section")
        .get_attribute("data-policy-loaded")
        .is_none()
    {
        let s = &v["settings"];
        input("phones-android-package")
            .set_value(s["android_apps"][0]["package"].as_str().unwrap_or(""));
        input("phones-android-signer")
            .set_value(s["android_apps"][0]["signers"][0].as_str().unwrap_or(""));
        input("phones-apple-team").set_value(s["apple_apps"][0]["team_id"].as_str().unwrap_or(""));
        input("phones-apple-bundle")
            .set_value(s["apple_apps"][0]["bundle_id"].as_str().unwrap_or(""));
        input("phones-apple-category").set_value(
            &s["apple_categories"][0]
                .as_u64()
                .map(|v| v.to_string())
                .unwrap_or_default(),
        );
        input("phones-apple-development")
            .set_checked(s["apple_development"].as_bool().unwrap_or(false));
        let _ = element("phones-section").set_attribute("data-policy-loaded", "");
    }
    element("phones-list-status").set_text_content(Some(if v["revocations_fresh"] == true {
        "Android certificate checks are up to date."
    } else {
        "Android sensitive actions need an up-to-date certificate list. Refresh when online."
    }));
    if let Some(e) = v["revocations"]["last_error"].as_str() {
        element("phones-list-status").set_text_content(Some(e));
    }
    let pending = element("phones-pending");
    pending.set_text_content(None);
    for p in v["pending"].as_array().into_iter().flatten() {
        let row = child(&pending, "div", "")?;
        child(
            &row,
            "p",
            &format!(
                "{} · Compare code {} on your phone.",
                p["name"].as_str().unwrap_or("Phone"),
                p["code"].as_str().unwrap_or("")
            ),
        )?;
        let offer = p["offer"].clone();
        button(
            &row,
            "Codes match: pair",
            "phones_decide",
            json!({"offer":offer,"allow":true}),
            false,
        )?;
        button(
            &row,
            "Decline",
            "phones_decide",
            json!({"offer":offer,"allow":false}),
            false,
        )?;
    }
    let devices = element("phones-devices");
    devices.set_text_content(None);
    for d in v["devices"].as_array().into_iter().flatten() {
        let row = child(&devices, "div", "")?;
        child(&row, "h4", d["name"].as_str().unwrap_or("Phone"))?;
        child(
            &row,
            "p",
            if d["step_up"].is_null() {
                "Sensitive actions are not set up."
            } else {
                "This phone has set up sensitive actions. Owner permission is still required."
            },
        )?;
        let id = d["id"].clone();
        button(
            &row,
            "Remove phone",
            "phones_remove",
            json!({"device":id}),
            true,
        )?;
        if d["owner"] == true {
            button(
                &row,
                "Allow sensitive actions for 15 minutes",
                "phones_sensitive",
                json!({"device":id,"allow":true}),
                true,
            )?;
            button(
                &row,
                "End permission",
                "phones_sensitive",
                json!({"device":id,"allow":false}),
                false,
            )?;
        }
    }
    Ok(())
}
pub fn reset() {
    for id in ["phones-pending", "phones-devices", "phones-code"] {
        element(id).set_text_content(None);
    }
    let _ = element("phones-section").remove_attribute("data-policy-loaded");
    for id in [
        "phones-android-package",
        "phones-android-signer",
        "phones-apple-team",
        "phones-apple-bundle",
        "phones-apple-category",
    ] {
        input(id).set_value("");
    }
    let dialog: HtmlDialogElement = element("phones-confirm").dyn_into().unwrap();
    dialog.close();
    let _ = dialog.remove_attribute("data-command");
    let _ = dialog.remove_attribute("data-args");
}
pub fn on_open() {
    spawn_local(refresh());
}
pub fn install() {
    on(element("phones-enable").as_ref(), "change", |_| {
        let enabled = input("phones-enable").checked();
        let g = notes_panel::generation();
        spawn_local(async move {
            invoke("phones_set_enabled", json!({"enabled":enabled}), g).await;
        });
    });
    on(element("phones-refresh").as_ref(), "click", |_| {
        spawn_local(refresh());
    });
    on(element("phones-revocations").as_ref(), "click", |_| {
        let g = notes_panel::generation();
        spawn_local(async move {
            invoke("phones_refresh_revocations", json!({}), g).await;
        });
    });
    on(element("phones-offer").as_ref(), "click", |_| {
        let profile = element("phones-profile")
            .dyn_into::<HtmlSelectElement>()
            .unwrap()
            .value();
        let g = notes_panel::generation();
        spawn_local(async move {
            match call("phones_create_offer", json!({"profile":profile})).await {
                Ok(value) => {
                    if g == notes_panel::generation()
                        && let Ok(v) = serde_wasm_bindgen::from_value::<Value>(value)
                    {
                        element("phones-code").set_text_content(v["code"].as_str());
                        status(
                            "Copy this pairing code to the phone. It expires after two minutes. Then refresh to compare the codes.",
                        );
                    }
                }
                Err(e) => {
                    if g == notes_panel::generation() {
                        status(&e);
                    }
                }
            }
        });
    });
    on(element("phones-save-policy").as_ref(), "click", |_| {
        let package = input("phones-android-package").value();
        let signer = input("phones-android-signer").value();
        let team = input("phones-apple-team").value();
        let bundle = input("phones-apple-bundle").value();
        let category = input("phones-apple-category").value();
        let android = if package.trim().is_empty() {
            vec![]
        } else {
            vec![json!({"package":package.trim(),"signers":[signer.trim()]})]
        };
        let apple = if team.trim().is_empty() {
            vec![]
        } else {
            vec![json!({"team_id":team.trim(),"bundle_id":bundle.trim()})]
        };
        let categories: Vec<u32> = match category.trim() {
            "" => vec![],
            s => match s.parse() {
                Ok(c) => vec![c],
                Err(_) => {
                    status("Enter a whole number for the App Attest category.");
                    return;
                }
            },
        };
        let settings = json!({"enabled":input("phones-enable").checked(),"android_apps":android,"apple_apps":apple,"apple_categories":categories,"apple_development":input("phones-apple-development").checked()});
        let g = notes_panel::generation();
        spawn_local(async move {
            invoke("phones_set_settings", json!({"settings":settings}), g).await;
        });
    });
    on(element("phones-confirm-cancel").as_ref(), "click", |_| {
        element("phones-confirm")
            .dyn_into::<HtmlDialogElement>()
            .unwrap()
            .close();
    });
    on(element("phones-confirm-allow").as_ref(), "click", |_| {
        let dialog = element("phones-confirm");
        let command = dialog.get_attribute("data-command").unwrap_or_default();
        let args = dialog
            .get_attribute("data-args")
            .and_then(|s| serde_json::from_str::<Value>(&s).ok());
        dialog.dyn_into::<HtmlDialogElement>().unwrap().close();
        if let Some(args) = args {
            let g = notes_panel::generation();
            spawn_local(async move {
                invoke(
                    if command == "phones_sensitive" {
                        "phones_sensitive"
                    } else {
                        "phones_remove"
                    },
                    args,
                    g,
                )
                .await;
            });
        }
    });
}
