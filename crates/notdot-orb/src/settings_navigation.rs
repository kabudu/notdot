//! Local settings navigation. Categories never remove controls or change values.
use super::{element, input, on};
use std::{cell::RefCell, rc::Rc};
use wasm_bindgen::{JsCast, JsValue};

const CATEGORIES: [&str; 5] = ["personalise", "voice", "codex", "memory", "app"];
const SECTIONS: [(&str, usize, &str); 14] = [
    (
        "appearance-section",
        0,
        "appearance name colour color theme personalise",
    ),
    (
        "voice-section",
        1,
        "voice speech recognition transcription stt tts elevenlabs deepgram api key sound language",
    ),
    (
        "wake-section",
        1,
        "wake microphone mute hands free follow up conversation listening",
    ),
    (
        "people-section",
        1,
        "people trusted voices owner enrol voice recognition sharing private profiles",
    ),
    (
        "usage-section",
        1,
        "usage quota credits allowance billing reset balance",
    ),
    (
        "intelligence-section",
        2,
        "intelligence model reasoning effort thinking terra",
    ),
    (
        "session-section",
        2,
        "codex chat control permission task completion result report confirmation",
    ),
    (
        "memory-section",
        3,
        "memory learn remember forget review notes reset",
    ),
    (
        "voice-notes-section",
        3,
        "notes lists shopping list dictate export voice notes",
    ),
    (
        "backup-section",
        3,
        "backup restore encrypted export import password",
    ),
    (
        "reminders-section",
        3,
        "reminders follow ups proactive suggestions quiet hours check ins",
    ),
    (
        "startup-section",
        4,
        "startup launch login sign in updates about version",
    ),
    (
        "connection-section",
        4,
        "connection reconnect remove saved key",
    ),
    (
        "orb-help",
        4,
        "help keyboard shortcut drag record orb instructions",
    ),
];
struct Navigation {
    selected: usize,
    onboarding: bool,
    query: String,
    disclosure_states: Option<Vec<bool>>,
    searchable: Vec<String>,
}
type Shared = Rc<RefCell<Navigation>>;
thread_local! {
    static NAVIGATION: RefCell<Option<Shared>> = const { RefCell::new(None) };
}
fn tab_id(index: usize) -> String {
    format!("settings-category-{}", CATEGORIES[index])
}
fn matches(text: &str, query: &str) -> bool {
    query.split_whitespace().all(|term| text.contains(term))
}
fn set_hidden(id: &str, hidden: bool) {
    if hidden {
        let _ = element(id).set_attribute("hidden", "");
    } else {
        let _ = element(id).remove_attribute("hidden");
    }
}
fn render(state: &Shared) {
    let state = state.borrow();
    let searching = !state.query.is_empty();
    let mut count = 0;
    for (index, (id, category, _)) in SECTIONS.iter().enumerate() {
        let visible = if searching {
            matches(&state.searchable[index], &state.query)
        } else {
            *category == state.selected
                || (state.onboarding && state.selected == 0 && *id == "voice-section")
        };
        set_hidden(id, !visible);
        if visible {
            count += 1;
            if searching {
                let _ = element(id).set_attribute("open", "");
            }
        }
    }
    set_hidden("settings-categories", searching);
    set_hidden("settings-search-clear", !searching);
    set_hidden("settings-guidance", searching || !state.onboarding);
    for index in 0..CATEGORIES.len() {
        let tab = element(&tab_id(index));
        let selected = index == state.selected;
        let _ = tab.set_attribute("aria-selected", if selected { "true" } else { "false" });
        let _ = tab.set_attribute("tabindex", if selected { "0" } else { "-1" });
    }
    let panel = element("settings-content");
    let _ = panel.set_attribute("role", if searching { "region" } else { "tabpanel" });
    let label = if searching {
        "settings-search-label".to_owned()
    } else {
        tab_id(state.selected)
    };
    let _ = panel.set_attribute("aria-labelledby", &label);
    let status = if searching {
        match count {
            0 => "No settings match. Try another word.".to_owned(),
            1 => "1 section matches.".to_owned(),
            _ => format!("{count} sections match."),
        }
    } else {
        String::new()
    };
    element("settings-search-status").set_text_content(Some(&status));
}
fn clear_search(state: &Shared) {
    {
        let mut state = state.borrow_mut();
        state.query.clear();
        if let Some(saved) = state.disclosure_states.take() {
            for ((id, _, _), open) in SECTIONS.iter().zip(saved) {
                if open {
                    let _ = element(id).set_attribute("open", "");
                } else {
                    let _ = element(id).remove_attribute("open");
                }
            }
        }
    }
    input("settings-search").set_value("");
    render(state);
}
fn choose(state: &Shared, index: usize, focus: bool) {
    clear_search(state);
    {
        let mut state = state.borrow_mut();
        state.selected = index;
        state.onboarding = false;
    }
    render(state);
    if index == 0 {
        let _ = element("appearance-section").set_attribute("open", "");
    }
    if focus {
        let _ = element(&tab_id(index))
            .unchecked_into::<web_sys::HtmlElement>()
            .focus();
    }
    element("settings-content").set_scroll_top(0);
}
pub(crate) fn install() -> Result<(), JsValue> {
    if NAVIGATION.with(|slot| slot.borrow().is_some()) {
        return Ok(());
    }
    let searchable = SECTIONS
        .iter()
        .map(|(id, _, aliases)| {
            format!(
                "{} {aliases}",
                element(id).text_content().unwrap_or_default()
            )
            .to_lowercase()
        })
        .collect();
    let state = Rc::new(RefCell::new(Navigation {
        selected: 0,
        onboarding: false,
        query: String::new(),
        disclosure_states: None,
        searchable,
    }));
    NAVIGATION.with(|slot| *slot.borrow_mut() = Some(state.clone()));
    let _ = element("appearance-section").set_attribute("open", "");
    for index in 0..CATEGORIES.len() {
        let target = state.clone();
        on(element(&tab_id(index)).as_ref(), "click", move |_| {
            choose(&target, index, false)
        });
        let target = state.clone();
        on(element(&tab_id(index)).as_ref(), "keydown", move |event| {
            let Ok(key) = event.dyn_into::<web_sys::KeyboardEvent>() else {
                return;
            };
            let next = match key.key().as_str() {
                "ArrowRight" | "ArrowDown" => (index + 1) % CATEGORIES.len(),
                "ArrowLeft" | "ArrowUp" => (index + CATEGORIES.len() - 1) % CATEGORIES.len(),
                "Home" => 0,
                "End" => CATEGORIES.len() - 1,
                _ => return,
            };
            key.prevent_default();
            key.stop_propagation();
            choose(&target, next, true);
        });
    }
    let target = state.clone();
    on(element("settings-search").as_ref(), "input", move |_| {
        let query = input("settings-search").value().trim().to_lowercase();
        if query.is_empty() {
            clear_search(&target);
            return;
        }
        {
            let mut state = target.borrow_mut();
            if state.disclosure_states.is_none() {
                state.disclosure_states = Some(
                    SECTIONS
                        .iter()
                        .map(|(id, _, _)| element(id).has_attribute("open"))
                        .collect(),
                );
            }
            state.query = query;
        }
        render(&target);
    });
    let target = state.clone();
    on(
        element("settings-search").as_ref(),
        "keydown",
        move |event| {
            let Ok(key) = event.dyn_into::<web_sys::KeyboardEvent>() else {
                return;
            };
            if key.key() == "Enter" {
                key.prevent_default();
                key.stop_propagation();
            }
            if key.key() == "Escape" && !target.borrow().query.is_empty() {
                key.prevent_default();
                key.stop_propagation();
                clear_search(&target);
            }
        },
    );
    let target = state.clone();
    on(
        element("settings-search-clear").as_ref(),
        "click",
        move |_| {
            clear_search(&target);
            let _ = input("settings-search").focus();
        },
    );
    render(&state);
    Ok(())
}
/// Call whenever the panel opens, after its saved values have been populated.
pub(crate) fn on_open(onboarding: bool) {
    NAVIGATION.with(|slot| {
        if let Some(state) = slot.borrow().as_ref() {
            clear_search(state);
            let mut navigation = state.borrow_mut();
            navigation.onboarding = onboarding;
            if onboarding {
                navigation.selected = 0;
            }
            drop(navigation);
            render(state);
        }
    });
}
/// Reveal a section before directing the user to a voice/setup error or review.
pub(crate) fn reveal(section: &str) {
    if let Some((_, category, _)) = SECTIONS.iter().find(|(id, _, _)| *id == section) {
        NAVIGATION.with(|slot| {
            if let Some(state) = slot.borrow().as_ref() {
                choose(state, *category, false);
                let _ = element(section).set_attribute("open", "");
            }
        });
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn settings_search_matches_all_words_without_requiring_field_order() {
        assert!(super::matches("voice service saved api key", "key voice"));
        assert!(!super::matches("voice service saved api key", "key memory"));
        assert!(super::matches("orb colour color theme", "color"));
    }
    #[test]
    fn every_category_has_a_section_and_search_aliases() {
        for index in 0..super::CATEGORIES.len() {
            assert!(
                super::SECTIONS
                    .iter()
                    .any(|(_, category, terms)| *category == index && !terms.is_empty())
            );
        }
    }
}
