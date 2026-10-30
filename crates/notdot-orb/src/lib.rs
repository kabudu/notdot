#![forbid(unsafe_code)]
mod audio_meter;
mod interaction;
mod speech_queue;
mod visual_state;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{cell::RefCell, f64::consts::TAU, rc::Rc};
use wasm_bindgen::{JsCast, prelude::*};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{
    CanvasRenderingContext2d as Context, Event, HtmlCanvasElement, HtmlDialogElement,
    HtmlInputElement,
};
#[wasm_bindgen(inline_js = r#"
export function listenCapture(f){return window.__TAURI__.event.listen('voice-capture',e=>f(e.payload))}
export function invoke(c,a){return window.__TAURI__.core.invoke(c,a)}
export function drag(){return window.__TAURI__.window.getCurrentWindow().startDragging()}
export async function resize(w,h){
    const api=window.__TAURI__, win=api.window.getCurrentWindow();
    const monitor=await api.window.currentMonitor();
    if(!monitor){
        await win.setSize(new api.dpi.LogicalSize(Math.min(w,screen.availWidth-32),Math.min(h,screen.availHeight-64)));
        return;
    }
    const area=monitor.workArea, margin=Math.round(16*monitor.scaleFactor);
    await win.setSize(new api.dpi.PhysicalSize(
        Math.max(1,Math.min(Math.round(w*monitor.scaleFactor),area.size.width-2*margin)),
        Math.max(1,Math.min(Math.round(h*monitor.scaleFactor),area.size.height-2*margin))));
    const [position,size]=await Promise.all([win.outerPosition(),win.outerSize()]);
    const x=Math.max(area.position.x+margin,Math.min(position.x,area.position.x+area.size.width-size.width-margin));
    const y=Math.max(area.position.y+margin,Math.min(position.y,area.position.y+area.size.height-size.height-margin));
    if(x!==position.x || y!==position.y) await win.setPosition(new api.dpi.PhysicalPosition(x,y));
}
"#)]
extern "C" {
    fn invoke(c: &str, a: JsValue) -> js_sys::Promise;
    #[wasm_bindgen(js_name=listenCapture)]
    fn listen_capture(f: &js_sys::Function) -> js_sys::Promise;
    fn drag() -> js_sys::Promise;
    fn resize(w: f64, h: f64) -> js_sys::Promise;
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct Settings {
    name: String,
    model: String,
    effort: String,
    hue: u16,
    voice_id: String,
    voice_provider: String,
    speech_language: String,
    stt_provider: String,
    voice_name: String,
    memory_enabled: bool,
    initiative: Value,
    onboarding_complete: bool,
    thread_id: Option<String>,
    startup: String,
    open_desktop: bool,
    automatic_updates: bool,
    wake_enabled: bool,
    wake_name_only: bool,
    microphone_muted: bool,
}
#[derive(Clone, Default, Deserialize)]
struct Snapshot {
    settings: Settings,
    phase: String,
    reply: String,
    #[serde(default)]
    speech: SpeechText,
    request_id: Option<String>,
    error: Option<String>,
    key_configured: bool,
    key_checking: bool,
    key_authorized: bool,
    stt_ready: bool,
    window_visible: bool,
    version: String,
    desktop_launch_supported: bool,
    updates: UpdateStatus,
    quota: Value,
    wake: Value,
    learning: Value,
    initiative: Value,
}
#[derive(Clone, Default, Deserialize)]
struct SpeechText {
    turn: String,
    text: String,
    done: bool,
    failed: bool,
}
#[derive(Clone, Default, Deserialize)]
struct UpdateStatus {
    phase: String,
    version: Option<String>,
    message: String,
    received: u64,
    total: Option<u64>,
    prompt: bool,
}
#[derive(Clone, Deserialize)]
struct VoiceChoice {
    id: String,
    name: String,
    description: String,
}
#[derive(Deserialize)]
struct VoicePage {
    voices: Vec<VoiceChoice>,
    next_cursor: Option<String>,
}
#[derive(Deserialize)]
struct EncodedAudio {
    data: String,
    mime: String,
}
#[derive(Default)]
struct View {
    snapshot: Snapshot,
    recording: Option<web_sys::MediaRecorder>,
    stream: Option<web_sys::MediaStream>,
    audio: Option<web_sys::HtmlAudioElement>,
    audio_url: Option<String>,
    voice_pending: bool,
    speech_end: Option<interaction::SpeechEnd>,
    discard_capture: bool,
    reply_turn: String,
    request_id: Option<String>,
    speech_queue: speech_queue::SpeechQueue,
    speech_worker: bool,
    polling: bool,
    native_manual: bool,
    capture_token: Option<String>,
    last_capture_cue: String,
    capture_feedback_events: u64,
    last_poll_at: f64,
    generation: u64,
    last_frame: f64,
    amplitude: f64,
    speech_energy: f64,
    keyboard_navigation: bool,
    local_phase: Option<&'static str>,
    voices: Vec<VoiceChoice>,
    next_cursor: Option<String>,
    catalog_generation: u64,
    models: Vec<Value>,
    model_refreshing: bool,
    model_refresh_at: f64,
    preview_hue: Option<u16>,
    initialized: bool,
    meter: audio_meter::Meter,
    last_mode: visual_state::Mode,
    last_wake_enabled: bool,
    last_wake_status: String,
    last_report: f64,
    frames: u64,
    peak_speaking_rms: f64,
    update_announced: Option<String>,
    update_listening: Option<String>,
    memory_listening: Option<String>,
    consent_heard_voice: bool,
    quota_display: Option<Value>,
    quota_retry_at: f64,
    consent_silence: Option<f64>,
}
type Shared = Rc<RefCell<View>>;
fn window() -> web_sys::Window {
    web_sys::window().expect("WASM requires a browser window")
}
fn document() -> web_sys::Document {
    window().document().expect("notdot document exists")
}
fn show_stt_fields() {
    if select("stt-provider").value() == "deepgram" {
        let _ = element("deepgram-settings").remove_attribute("hidden");
    } else {
        let _ = element("deepgram-settings").set_attribute("hidden", "");
    }
}
fn element(id: &str) -> web_sys::Element {
    document()
        .get_element_by_id(id)
        .expect("notdot bundled UI element exists")
}
fn input(id: &str) -> HtmlInputElement {
    element(id).unchecked_into()
}
fn dialog(id: &str) -> HtmlDialogElement {
    element(id).unchecked_into()
}
fn select(id: &str) -> web_sys::HtmlSelectElement {
    element(id).unchecked_into()
}
fn apply_theme(hue: u16) {
    if let Some(root) = document().document_element() {
        let _ = root.set_attribute("style", &format!("--hue:{hue}"));
    }
}
fn close_voice_picker() {
    let _ = element("voice-popup").set_attribute("hidden", "");
    let _ = element("voice-toggle").set_attribute("aria-expanded", "false");
}
fn render_voice_options(view: &Shared, chosen: &str, saved_name: &str) {
    let dropdown = element("voice");
    dropdown.set_text_content(None);
    let empty = document().create_element("option").expect("voice option");
    let _ = empty.set_attribute("value", "");
    empty.set_text_content(Some("Choose a voice"));
    let _ = dropdown.append_child(&empty);
    let state = view.borrow();
    if !chosen.is_empty() && !state.voices.iter().any(|v| v.id == chosen) {
        let option = document().create_element("option").expect("saved voice");
        let _ = option.set_attribute("value", chosen);
        option.set_text_content(Some(if saved_name.is_empty() {
            "Saved voice"
        } else {
            saved_name
        }));
        let _ = dropdown.append_child(&option);
    }
    for voice in &state.voices {
        let option = document().create_element("option").expect("voice option");
        let _ = option.set_attribute("value", &voice.id);
        option.set_text_content(Some(&voice.name));
        let _ = dropdown.append_child(&option);
    }
    select("voice").set_value(chosen);
    let name = state
        .voices
        .iter()
        .find(|v| v.id == chosen)
        .map(|v| v.name.as_str())
        .unwrap_or(saved_name);
    element("voice-value").set_text_content(Some(if chosen.is_empty() {
        "Choose a voice"
    } else if name.is_empty() {
        "Saved voice"
    } else {
        name
    }));
    let _ = element("preview-voice").toggle_attribute_with_force("disabled", chosen.is_empty());
    let results = element("voice-results");
    results.set_text_content(None);
    for voice in &state.voices {
        let button = document().create_element("button").expect("voice choice");
        let _ = button.set_attribute("type", "button");
        let _ = button.set_attribute(
            "aria-pressed",
            if voice.id == chosen { "true" } else { "false" },
        );
        button.set_text_content(Some(&voice.name));
        let _ = button.set_attribute("data-voice-id", &voice.id);
        let _ = button.set_attribute("data-description", &voice.description);
        let _ = results.append_child(&button);
    }
}
async fn load_catalog(view: Shared, append: bool) -> Result<(), String> {
    let provider = select("provider").value();
    let key = input("key").value();
    if !key.is_empty() {
        call("save_voice_key", json!({"provider":provider,"key":key})).await?;
        input("key").set_value("");
        view.borrow_mut().snapshot.key_configured = true;
        input("key").set_placeholder("Key saved · leave blank to keep");
    }
    let search = input("voice-search").value();
    let cursor = if append {
        view.borrow().next_cursor.clone()
    } else {
        None
    };
    let generation = {
        let mut v = view.borrow_mut();
        v.catalog_generation += 1;
        v.catalog_generation
    };
    element("voice-info").set_text_content(Some("Loading voices…"));
    let result = call(
        "voice_catalog",
        json!({"provider":provider,"search":search,"cursor":cursor}),
    )
    .await;
    if view.borrow().catalog_generation != generation {
        return Ok(());
    }
    let page: VoicePage = match result
        .and_then(|value| serde_wasm_bindgen::from_value(value).map_err(|error| error.to_string()))
    {
        Ok(page) => page,
        Err(error) => {
            element("voice-info")
                .set_text_content(Some(&format!("Could not load voices: {error}")));
            return Err(error);
        }
    };
    let chosen = select("voice").value();
    let saved_name = element("voice-value").text_content().unwrap_or_default();
    let has_more = page.next_cursor.is_some()
        && (!append || view.borrow().voices.len() + page.voices.len() <= 1000);
    if search.is_empty() {
        if page.voices.len() > 5 || has_more {
            let _ = element("voice-search-label").remove_attribute("hidden");
        } else {
            let _ = element("voice-search-label").set_attribute("hidden", "");
        }
    }
    {
        let mut v = view.borrow_mut();
        if !append {
            v.voices.clear();
        }
        for voice in page.voices {
            if !v.voices.iter().any(|v| v.id == voice.id) && v.voices.len() < 1000 {
                v.voices.push(voice);
            }
        }
        v.next_cursor = if has_more { page.next_cursor } else { None };
    }
    render_voice_options(&view, &chosen, &saved_name);
    if has_more {
        let _ = element("more-voices").remove_attribute("hidden");
    } else {
        let _ = element("more-voices").set_attribute("hidden", "");
    }
    element("voice-info").set_text_content(Some(if view.borrow().voices.is_empty() {
        "No matching voices"
    } else {
        "Choose a voice, then preview it."
    }));
    Ok(())
}
fn usage_time(value: &Value) -> String {
    value
        .as_f64()
        .map(|t| {
            js_sys::Date::new(&JsValue::from_f64(t * 1000.))
                .to_locale_string("en-GB", &JsValue::UNDEFINED)
                .as_string()
                .unwrap_or_default()
        })
        .unwrap_or_else(|| "not reported".into())
}
fn usage_text(parent: &web_sys::Element, tag: &str, class: &str, text: &str) -> web_sys::Element {
    let node = document().create_element(tag).expect("usage element");
    node.set_class_name(class);
    node.set_text_content(Some(text));
    let _ = parent.append_child(&node);
    node
}
fn credit_count(value: u64) -> String {
    let digits = value.to_string();
    let mut output = String::with_capacity(digits.len() + 6);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            output.push(',');
        }
        output.push(ch);
    }
    output
}
fn usage_date(value: &Value) -> String {
    value
        .as_u64()
        .filter(|n| *n > 0)
        .map(|n| {
            let options = json!({"day":"numeric", "month":"short"})
                .serialize(&serde_wasm_bindgen::Serializer::json_compatible())
                .unwrap_or(JsValue::UNDEFINED);
            js_sys::Date::new(&JsValue::from_f64(n as f64 * 1000.))
                .to_locale_date_string("en-GB", &options)
                .into()
        })
        .unwrap_or_else(|| "Not reported".into())
}
fn render_usage(quota: &Value) {
    let expanded = document()
        .get_element_by_id("usage-more")
        .is_some_and(|e| e.has_attribute("open"));
    let area = element("usage-details");
    area.set_text_content(None);
    if let Some(card) = element("usage-title")
        .parent_element()
        .and_then(|e| e.parent_element())
    {
        let _ = card.set_attribute("data-level", quota["level"].as_str().unwrap_or("normal"));
    }
    let overview = quota["report"]["meters"]
        .as_array()
        .and_then(|m| m.first())
        .and_then(|m| {
            let used = m["used"].as_u64()?;
            let limit = m["limit"].as_u64()?;
            (limit > 0).then(|| {
                format!(
                    "{:.1}% used{}",
                    used as f64 / limit as f64 * 100.,
                    if quota["stale"] == true {
                        " · last known"
                    } else {
                        ""
                    }
                )
            })
        })
        .unwrap_or_else(|| "Not checked yet".into());
    element("usage-overview").set_text_content(Some(&overview));
    let stale = quota["availability"] != "available" || quota["stale"] == true;
    let has_report = quota["report"]["meters"]
        .as_array()
        .is_some_and(|m| !m.is_empty());
    if stale {
        usage_text(
            &area,
            "p",
            "usage-notice",
            if has_report {
                "Last known balance · unable to refresh"
            } else {
                quota["message"]
                    .as_str()
                    .unwrap_or("Usage has not been checked yet.")
            },
        );
    }
    let more = document().create_element("details").expect("usage details");
    more.set_id("usage-more");
    if expanded {
        let _ = more.set_attribute("open", "");
    }
    usage_text(&more, "summary", "", "Details");
    if let Some(meters) = quota["report"]["meters"].as_array() {
        for meter in meters {
            let used = meter["used"].as_u64().unwrap_or(0);
            let limit = meter["limit"].as_u64().unwrap_or(0);
            let label = meter["label"].as_str().unwrap_or("Voice allowance");
            let unit = match meter["unit"].as_str().unwrap_or("units") {
                "provider credits" | "provider credits (legacy character fields)" => "credits",
                unit => unit,
            };
            if limit > 0 {
                let percent = used as f64 / limit as f64 * 100.;
                let summary = usage_text(&area, "div", "usage-main", "");
                let used_block = usage_text(&summary, "div", "usage-number", "");
                usage_text(&used_block, "strong", "", &format!("{percent:.1}%"));
                usage_text(&used_block, "span", "", "used");
                let left_block = usage_text(&summary, "div", "usage-number usage-left", "");
                usage_text(
                    &left_block,
                    "strong",
                    "",
                    &credit_count(limit.saturating_sub(used)),
                );
                usage_text(&left_block, "span", "", &format!("{unit} left"));
                let bar = document()
                    .create_element("progress")
                    .expect("usage progress");
                let _ = bar.set_attribute("max", "100");
                let _ = bar.set_attribute("value", &percent.min(100.).to_string());
                let _ = bar.set_attribute("aria-label", label);
                let _ = bar.set_attribute(
                    "aria-valuetext",
                    &format!(
                        "{percent:.1}% used; {} of {} {unit}",
                        credit_count(used),
                        credit_count(limit)
                    ),
                );
                let _ = area.append_child(&bar);
                usage_text(
                    &area,
                    "p",
                    "usage-total",
                    &format!("{} of {} {unit}", credit_count(used), credit_count(limit)),
                );
            } else {
                usage_text(
                    &area,
                    "p",
                    "usage-notice",
                    &format!(
                        "{} {unit} used. Allowance not reported.",
                        credit_count(used)
                    ),
                );
            }
            usage_text(
                &area,
                "p",
                "usage-reset",
                &format!("Resets {}", usage_date(&meter["reset_unix"])),
            );
            let billing = match meter["overage"].as_str() {
                Some("enabled") => {
                    "Your provider allows extra billed usage. notdot will not change this setting."
                }
                Some("disabled") => "Extra usage billing is off.",
                _ => "Extra usage billing: not reported.",
            };
            usage_text(&more, "p", "", billing);
            usage_text(
                &more,
                "p",
                "",
                &format!("Full reset time: {}", usage_time(&meter["reset_unix"])),
            );
        }
        if let Some(note) = quota["report"]["note"].as_str() {
            usage_text(&more, "p", "", note);
        }
    }
    if stale && has_report {
        usage_text(
            &more,
            "p",
            "",
            quota["message"]
                .as_str()
                .unwrap_or("Balance may be out of date."),
        );
        usage_text(
            &more,
            "p",
            "",
            &format!(
                "Last successful check: {}",
                usage_time(&quota["last_success"])
            ),
        );
    }
    if let Some(fault) = quota["fault"]["message"].as_str() {
        usage_text(&more, "p", "", &format!("Last voice error: {fault}"));
    }
    usage_text(
        &area,
        "p",
        "usage-checked",
        &format!("Checked {}", usage_time(&quota["last_checked"])),
    );
    let _ = area.append_child(&more);
}
fn message(error: impl AsRef<str>) {
    element("error").set_text_content(Some(error.as_ref()));
    if let Some(reply) = document().get_element_by_id("reply") {
        reply.set_text_content(Some(error.as_ref()));
    }
}
fn js_error(v: JsValue) -> String {
    v.as_string().unwrap_or_else(|| format!("{v:?}"))
}
async fn call(command: &str, args: Value) -> Result<JsValue, String> {
    let args = args
        .serialize(&serde_wasm_bindgen::Serializer::json_compatible())
        .map_err(|e| e.to_string())?;
    JsFuture::from(invoke(command, args))
        .await
        .map_err(js_error)
}
fn on(target: &web_sys::EventTarget, event: &str, f: impl FnMut(Event) + 'static) {
    let c = Closure::<dyn FnMut(Event)>::new(f);
    target
        .add_event_listener_with_callback(event, c.as_ref().unchecked_ref())
        .expect("event registration");
    c.forget();
}
fn render_wake_phrase() {
    let name = input("name").value();
    let phrase = if select("wake-phrase").value() == "name" {
        name.trim().to_owned()
    } else {
        format!("Hey {}", name.trim())
    };
    element("wake-phrase-preview").set_text_content(Some(&format!("Say “{phrase}”, then speak after the orb lights up. Name-only phrases may trigger accidentally.")));
}
fn wake_hint(snapshot: &Snapshot) -> &str {
    if snapshot.settings.microphone_muted {
        "Microphone muted. Open Settings to turn it on."
    } else if snapshot.key_checking {
        "Wake listening is paused while saved key access is checked."
    } else if !snapshot.key_configured {
        "Wake listening is paused. Open Settings → Voice to allow saved key access or add a key."
    } else if snapshot.settings.voice_id.is_empty() {
        "Wake listening is paused. Choose a voice in Settings."
    } else {
        snapshot.wake["message"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("Preparing wake listening…")
    }
}
fn wake_ready(view: &Shared, snapshot: &Snapshot) -> bool {
    let v = view.borrow();
    snapshot.phase == "idle"
        && snapshot.window_visible
        && (snapshot.settings.wake_enabled || v.native_manual)
        && !snapshot.settings.microphone_muted
        && snapshot.settings.onboarding_complete
        && snapshot.key_configured
        && snapshot.stt_ready
        && !snapshot.key_checking
        && !snapshot.settings.voice_id.is_empty()
        && !v.voice_pending
        && (v.local_phase.is_none() || (v.native_manual && v.local_phase == Some("listening")))
        && v.recording.is_none()
        && ((v.update_listening.is_none() && v.memory_listening.is_none()) || v.native_manual)
        && !dialog("panel").open()
        && !dialog("about").open()
        && !matches!(
            snapshot.updates.phase.as_str(),
            "downloading" | "installing"
        )
        && v.audio.as_ref().is_none_or(|a| a.paused() || a.ended())
}
async fn wake_request(view: Shared, id: u64, generation: u64) -> Result<(), String> {
    let text = call("transcribe_wake", json!({"id":id}))
        .await?
        .as_string()
        .ok_or("Transcription returned no text")?;
    submit_transcript(view, text, generation).await
}
fn close_intelligence_picker() {
    let _ = element("intelligence-section").remove_attribute("open");
    let _ = element("intelligence-toggle").set_attribute("aria-expanded", "false");
}
fn effort_label(value: &str) -> String {
    let mut label = match value {
        "xhigh" => "Extra high",
        "max" => "Maximum",
        _ => value,
    }
    .to_string();
    if let Some(first) = label.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    label
}
fn render_intelligence_summary(view: &Shared) {
    let model = select("model").value();
    let effort = select("effort").value();
    let v = view.borrow();
    let name = v
        .models
        .iter()
        .find(|m| m["model"] == model)
        .and_then(|m| m["displayName"].as_str())
        .unwrap_or("Choose intelligence");
    element("intelligence-model").set_text_content(Some(name));
    let summary = if effort.is_empty() {
        "Choose a model and thinking level".to_string()
    } else {
        format!(
            "{} thinking{}",
            effort_label(&effort),
            if model == "gpt-6.1-terra" && effort == "medium" {
                " · notdot default"
            } else {
                ""
            }
        )
    };
    element("intelligence-effort").set_text_content(Some(&summary));
}
fn render_model_cards(view: &Shared) {
    element("model-options").set_text_content(None);
    let selected = select("model").value();
    for (i, model) in view.borrow().models.iter().enumerate() {
        let Some(value) = model["model"].as_str() else {
            continue;
        };
        let card = document().create_element("label").expect("label");
        card.set_id(&format!("model-card-{i}"));
        card.set_class_name("intelligence-option model-card");
        let radio: HtmlInputElement = document()
            .create_element("input")
            .expect("input")
            .unchecked_into();
        radio.set_type("radio");
        radio.set_name("intelligence-model");
        radio.set_id(&format!("model-choice-{value}"));
        radio.set_value(value);
        radio.set_checked(value == selected);
        let _ = card.append_child(&radio);
        let name = document().create_element("strong").expect("model name");
        name.set_text_content(Some(model["displayName"].as_str().unwrap_or(value)));
        let _ = card.append_child(&name);
        let description = document()
            .create_element("span")
            .expect("model description");
        description.set_text_content(model["description"].as_str());
        let _ = card.append_child(&description);
        let _ = element("model-options").append_child(&card);
    }
    filter_model_cards(view);
}
fn filter_model_cards(view: &Shared) {
    let query = input("model-search").value().to_lowercase();
    let mut count = 0;
    for i in 0..view.borrow().models.len() {
        if let Some(card) = document().get_element_by_id(&format!("model-card-{i}")) {
            let visible = card
                .text_content()
                .unwrap_or_default()
                .to_lowercase()
                .contains(&query);
            let _ = card.toggle_attribute_with_force("hidden", !visible);
            if visible {
                count += 1;
            }
        }
    }
    element("model-search-status").set_text_content(Some(if count == 0 {
        "No matching models. Try another name or refresh."
    } else {
        "Choose a model, then its thinking level below."
    }));
}
fn render_effort_options(view: &Shared) {
    element("effort-options").set_text_content(None);
    let model = select("model").value();
    let effort = select("effort").value();
    if let Some(choices) = view
        .borrow()
        .models
        .iter()
        .find(|m| m["model"] == model)
        .and_then(|m| m["supportedReasoningEfforts"].as_array())
    {
        for choice in choices {
            let Some(value) = choice["reasoningEffort"].as_str() else {
                continue;
            };
            let label = document().create_element("label").expect("effort label");
            label.set_class_name("intelligence-option effort-chip");
            let radio: HtmlInputElement = document()
                .create_element("input")
                .expect("effort input")
                .unchecked_into();
            radio.set_type("radio");
            radio.set_name("intelligence-effort");
            radio.set_id(&format!("effort-choice-{value}"));
            radio.set_value(value);
            radio.set_checked(value == effort);
            let _ = radio.set_attribute("aria-describedby", "effort-description");
            let _ = label.append_child(&radio);
            let text = document().create_element("span").expect("effort text");
            text.set_text_content(Some(&effort_label(value)));
            let _ = label.append_child(&text);
            let _ = element("effort-options").append_child(&label);
        }
    }
}
fn model_option(id: &str, value: &str, label: &str) {
    if let Ok(option) = document().create_element("option") {
        let _ = option.set_attribute("value", value);
        option.set_text_content(Some(label));
        let _ = element(id).append_child(&option);
    }
}
fn render_efforts(view: &Shared, preferred: &str, choose_default: bool) {
    let model = select("model").value();
    let choice = view
        .borrow()
        .models
        .iter()
        .find(|m| m["model"] == model)
        .cloned();
    element("effort").set_text_content(None);
    let Some(choice) = choice else {
        model_option("effort", "", "Choose an available model");
        select("effort").set_disabled(true);
        element("model-description").set_text_content(None);
        element("effort-description").set_text_content(None);
        render_effort_options(view);
        render_intelligence_summary(view);
        return;
    };
    element("model-description").set_text_content(choice["description"].as_str());
    let efforts = choice["supportedReasoningEfforts"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let valid = efforts.iter().any(|e| e["reasoningEffort"] == preferred);
    let selected = if valid {
        preferred.to_string()
    } else if choose_default {
        choice["defaultReasoningEffort"]
            .as_str()
            .unwrap_or("")
            .to_string()
    } else {
        String::new()
    };
    if selected.is_empty() {
        model_option("effort", "", "Choose an available effort");
    }
    for effort in efforts {
        if let Some(value) = effort["reasoningEffort"].as_str() {
            let mut label = match value {
                "xhigh" => "Extra high",
                "max" => "Maximum",
                _ => value,
            }
            .to_string();
            if let Some(first) = label.get_mut(..1) {
                first.make_ascii_uppercase();
            }
            model_option("effort", value, &label);
        }
    }
    select("effort").set_value(&selected);
    select("effort").set_disabled(false);
    render_effort_options(view);
    render_effort_description(view);
}
fn render_effort_description(view: &Shared) {
    let model = select("model").value();
    let effort = select("effort").value();
    let description = view
        .borrow()
        .models
        .iter()
        .find(|m| m["model"] == model)
        .and_then(|m| m["supportedReasoningEfforts"].as_array())
        .and_then(|es| es.iter().find(|e| e["reasoningEffort"] == effort))
        .and_then(|e| e["description"].as_str())
        .unwrap_or("Choose an effort supported by this model.")
        .to_string();
    element("effort-description").set_text_content(Some(&description));
    render_intelligence_summary(view);
}
fn refresh_models(view: &Shared, model: String, effort: String) {
    if view.borrow().model_refreshing {
        return;
    }
    let focus = document()
        .active_element()
        .map(|e| e.id())
        .filter(|id| id.starts_with("model-choice-") || id.starts_with("effort-choice-"));
    for group in ["model-choices-group", "effort-choices-group"] {
        let _ = element(group).set_attribute("disabled", "");
    }
    let _ = element("default-model").set_attribute("disabled", "");
    view.borrow_mut().model_refreshing = true;
    let _ = element("intelligence-toggle").set_attribute("disabled", "");
    element("intelligence-model").set_text_content(Some("Checking available models…"));
    view.borrow_mut().model_refresh_at = js_sys::Date::now() + 60_000.;
    select("model").set_disabled(true);
    select("effort").set_disabled(true);
    element("effort").set_text_content(None);
    model_option("effort", "", "Checking available efforts…");
    let _ = element("refresh-models").set_attribute("disabled", "");
    element("model").set_text_content(None);
    model_option("model", "", "Checking available models…");
    element("model-status")
        .set_text_content(Some("Checking your runtime’s current model offering…"));
    let view = view.clone();
    spawn_local(async move {
        let result = async {
            let value = call("model_catalog", json!({})).await?;
            serde_wasm_bindgen::from_value::<Vec<Value>>(value).map_err(|e| e.to_string())
        }
        .await;
        view.borrow_mut().model_refreshing = false;
        let _ = element("intelligence-toggle").remove_attribute("disabled");
        let _ = element("refresh-models").remove_attribute("disabled");
        for group in ["model-choices-group", "effort-choices-group"] {
            let _ = element(group).remove_attribute("disabled");
        }
        element("model").set_text_content(None);
        match result {
            Ok(models) => {
                let valid = models.iter().any(|m| m["model"] == model);
                model_option(
                    "model",
                    "",
                    if valid {
                        "Choose a model"
                    } else {
                        "Select an available model"
                    },
                );
                for m in &models {
                    if let Some(value) = m["model"].as_str() {
                        let label = m["displayName"].as_str().unwrap_or(value);
                        model_option(
                            "model",
                            value,
                            &format!(
                                "{label}{}",
                                if value == "gpt-6.1-terra" {
                                    " · notdot default"
                                } else {
                                    ""
                                }
                            ),
                        );
                    }
                }
                let default_available = models.iter().any(|m| {
                    m["model"] == "gpt-6.1-terra"
                        && m["supportedReasoningEfforts"]
                            .as_array()
                            .is_some_and(|es| es.iter().any(|e| e["reasoningEffort"] == "medium"))
                });
                let _ = element("default-model")
                    .toggle_attribute_with_force("disabled", !default_available);
                view.borrow_mut().models = models;
                select("model").set_value(if valid { &model } else { "" });
                select("model").set_disabled(false);
                render_model_cards(&view);
                render_efforts(&view, &effort, false);
                let available = valid && !select("effort").value().is_empty();
                element("model-status").set_text_content(Some(if available {
                    "Up to date. Save to use this choice."
                } else {
                    "Your saved choice is no longer available. Open the picker to choose another."
                }));
            }
            Err(e) => {
                view.borrow_mut().models.clear();
                render_model_cards(&view);
                render_effort_options(&view);
                render_intelligence_summary(&view);
                model_option("model", "", "Model catalog unavailable");
                element("effort").set_text_content(None);
                model_option("effort", "", "Unavailable");
                element("model-description").set_text_content(None);
                element("effort-description").set_text_content(None);
                element("model-status").set_text_content(Some(&format!(
                    "{e}. Retry Refresh; existing preferences are kept."
                )));
            }
        }
        if document()
            .active_element()
            .is_some_and(|e| e.tag_name() == "BODY")
            && let Some(id) = focus
            && let Some(control) = document().get_element_by_id(&id)
        {
            let _ = control.unchecked_into::<web_sys::HtmlElement>().focus();
        }
    });
}
fn open_panel(view: &Shared) {
    if view.borrow().capture_token.is_some() || view.borrow().memory_listening.is_some() {
        stop_audio(view);
    }
    if !view.borrow().snapshot.settings.onboarding_complete {
        let _ = element("appearance-section").set_attribute("open", "");
        let _ = element("voice-section").set_attribute("open", "");
    }
    close_intelligence_picker();
    input("model-search").set_value("");
    let _ = element("reset-memory-confirm").set_attribute("hidden", "");
    element("reset-memory-status").set_text_content(None);
    let s = view.borrow().snapshot.settings.clone();
    refresh_models(view, s.model.clone(), s.effort.clone());
    close_voice_picker();
    input("name").set_value(&s.name);
    input("hue").set_value(&s.hue.to_string());
    select("provider").set_value(&s.voice_provider);
    select("speech-language").set_value(&s.speech_language);
    select("stt-provider").set_value(&s.stt_provider);
    show_stt_fields();
    render_voice_options(view, &s.voice_id, &s.voice_name);
    apply_theme(s.hue);
    input("wake-enabled").set_checked(s.wake_enabled);
    input("microphone-muted").set_checked(s.microphone_muted);
    select("wake-phrase").set_value(if s.wake_name_only { "name" } else { "hey" });
    render_wake_phrase();
    spawn_local(async {
        let _ = call("wake_activity", json!({"ready":false})).await;
    });
    input("memory").set_checked(s.memory_enabled);
    select("behaviour").set_value(s.initiative["behaviour"].as_str().unwrap_or("reactive"));
    for (id, key) in [("quiet-start", "quiet_start"), ("quiet-end", "quiet_end")] {
        let minutes = s.initiative[key].as_u64().unwrap_or(0);
        input(id).set_value(&format!("{:02}:{:02}", minutes / 60, minutes % 60));
    }
    input("daily-limit").set_value(
        &s.initiative["daily_limit"]
            .as_u64()
            .unwrap_or(3)
            .to_string(),
    );
    input("followup-gap").set_value(
        &s.initiative["gap_minutes"]
            .as_u64()
            .unwrap_or(60)
            .to_string(),
    );
    input("start-at-login").set_checked(s.startup != "manual");
    select("startup").set_value(if s.startup == "with_codex" {
        "with_codex"
    } else {
        "login"
    });
    select("startup").set_disabled(s.startup == "manual");
    input("open-desktop").set_checked(s.open_desktop);
    let supported = view.borrow().snapshot.desktop_launch_supported;
    let _ = element("open-desktop").toggle_attribute_with_force("disabled", !supported);
    element("desktop-launch-description").set_text_content(Some(if supported {
        "Open Codex / ChatGPT in the background too."
    } else {
        "Opening Codex / ChatGPT automatically is available on macOS."
    }));
    input("automatic-updates").set_checked(s.automatic_updates);
    input("key").set_value("");
    input("key").set_placeholder(if view.borrow().snapshot.key_configured {
        "Key saved · leave blank to keep"
    } else {
        "New key, or use a saved key below"
    });
    element("heading").set_text_content(Some(if s.onboarding_complete {
        "Settings"
    } else {
        "Meet your companion"
    }));
    let _ = resize(450., 680.);
    let provider = s.voice_provider.clone();
    spawn_local(async move {
        if let Ok(value) = call("voice_providers", json!({})).await
            && let Ok(items) = serde_wasm_bindgen::from_value::<Value>(value)
            && let Some(items) = items.as_array()
        {
            let dropdown = element("provider");
            dropdown.set_text_content(None);
            for item in items {
                if let (Some(id), Some(name)) = (item["id"].as_str(), item["name"].as_str())
                    && let Ok(option) = document().create_element("option")
                {
                    let _ = option.set_attribute("value", id);
                    option.set_text_content(Some(name));
                    let _ = dropdown.append_child(&option);
                }
            }
            select("provider").set_value(&provider);
        }
    });
    let _ = dialog("panel").show_modal();
    if view.borrow().snapshot.key_authorized {
        let target = view.clone();
        spawn_local(async move {
            if let Err(e) = load_catalog(target, false).await {
                message(e);
            }
        });
    }
}
fn close_panel(view: &Shared) {
    view.borrow_mut().preview_hue = None;
    apply_theme(view.borrow().snapshot.settings.hue);
    dialog("panel").close();
    let _ = resize(360., 360.);
}
fn mute_capture(view: &Shared) {
    let capturing = {
        let v = view.borrow();
        v.recording.is_some()
            || matches!(v.local_phase, Some("listening" | "transcribing"))
            || v.snapshot.wake["phase"] == "capturing"
            || v.snapshot.wake["request_id"].is_number()
    };
    if capturing {
        stop_audio(view);
    }
}
fn stop_audio(view: &Shared) {
    spawn_local(async {
        let _ = call("wake_activity", json!({"ready":false})).await;
    });
    let mut v = view.borrow_mut();
    if let Some(token) = v.capture_token.take() {
        spawn_local(async move {
            let _ = call("cancel_capture", json!({"token":token})).await;
        });
    }
    v.native_manual = false;
    if let Some(id) = v.memory_listening.take() {
        spawn_local(async move {
            let _ = call("cancel_memory", json!({"id":id})).await;
        });
    }
    if let Some(request) = v.request_id.take() {
        spawn_local(async move {
            let _ = call("stop", json!({"requestId":request})).await;
        });
    }
    v.generation += 1;
    v.voice_pending = false;
    v.reply_turn.clear();
    v.speech_queue = speech_queue::SpeechQueue::default();
    v.speech_worker = false;
    v.speech_end = None;
    v.discard_capture = false;
    v.local_phase = None;
    v.meter.clear_microphone();
    v.meter.clear_playback();
    if let Some(recording) = v.recording.take() {
        let _ = recording.stop();
    }
    if let Some(stream) = v.stream.take() {
        for track in stream.get_tracks() {
            let track: web_sys::MediaStreamTrack = track.unchecked_into();
            track.stop();
        }
    }
    if let Some(audio) = v.audio.take() {
        let _ = audio.pause();
    }
    if let Some(url) = v.audio_url.take() {
        let _ = web_sys::Url::revoke_object_url(&url);
    }
}
async fn speech_tick() -> Result<(), String> {
    let promise = js_sys::Promise::new(&mut |resolve, reject| {
        if let Err(error) =
            window().set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 80)
        {
            let _ = reject.call1(&JsValue::NULL, &error);
        }
    });
    JsFuture::from(promise).await.map_err(js_error)?;
    Ok(())
}
async fn wait_for_audio(
    view: &Shared,
    generation: u64,
    audio: &web_sys::HtmlAudioElement,
) -> Result<(), String> {
    let started = js_sys::Date::now();
    loop {
        if view.borrow().generation != generation {
            return Ok(());
        }
        if js_sys::Date::now() - started > 120_000. {
            return Err("Voice playback took too long. Try again.".into());
        }
        if audio.error().is_some() {
            return Err("Voice playback failed".into());
        }
        if audio.ended() || audio.paused() {
            return Ok(());
        }
        speech_tick().await?;
    }
}
async fn stream_speech(view: Shared, generation: u64) -> Result<(), String> {
    let mut previous: Option<web_sys::HtmlAudioElement> = None;
    let started = js_sys::Date::now();
    loop {
        let next = loop {
            if js_sys::Date::now() - started > 240_000. {
                return Err("The spoken response took too long. Please try again.".into());
            }
            let (next, finished) = {
                let mut v = view.borrow_mut();
                if v.generation != generation {
                    return Ok(());
                }
                if v.snapshot.speech.turn == v.reply_turn
                    && (v.snapshot.speech.failed || v.snapshot.phase == "error")
                {
                    return Err("The response was interrupted".into());
                }
                let next = v.speech_queue.next();
                (next, v.speech_queue.finished())
            };
            if next.is_some() || finished {
                break next;
            }
            speech_tick().await?;
        };
        let Some(text) = next else {
            if let Some(audio) = previous {
                wait_for_audio(&view, generation, &audio).await?;
            }
            return Ok(());
        };
        // At most one next clip is synthesized while the current clip plays.
        let value = call("speak", json!({"text":text})).await?;
        if view.borrow().generation != generation {
            return Ok(());
        }
        if let Some(audio) = previous {
            wait_for_audio(&view, generation, &audio).await?;
        }
        if view.borrow().generation != generation {
            return Ok(());
        }
        play_encoded(view.clone(), value, generation).await?;
        previous = view.borrow().audio.clone();
    }
}
async fn play_encoded(view: Shared, value: JsValue, generation: u64) -> Result<(), String> {
    call("wake_activity", json!({"ready":false})).await?;
    if view.borrow().generation != generation {
        return Ok(());
    }
    let clip: EncodedAudio = serde_wasm_bindgen::from_value(value).map_err(|e| e.to_string())?;
    let encoded = clip.data;
    let binary = window().atob(&encoded).map_err(js_error)?;
    let bytes: Vec<u8> = binary.chars().map(|c| c as u8).collect();
    let array = js_sys::Array::new();
    array.push(&js_sys::Uint8Array::from(bytes.as_slice()));
    let options = web_sys::BlobPropertyBag::new();
    options.set_type(&clip.mime);
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&array, &options)
        .map_err(js_error)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(js_error)?;
    let audio = web_sys::HtmlAudioElement::new_with_src(&url).map_err(js_error)?;
    {
        let mut v = view.borrow_mut();
        if let Some(old) = v.audio_url.replace(url) {
            let _ = web_sys::Url::revoke_object_url(&old);
        }
        v.audio = Some(audio.clone());
    }
    view.borrow_mut().meter.playback(&audio).map_err(js_error)?;
    JsFuture::from(audio.play().map_err(js_error)?)
        .await
        .map_err(js_error)?;
    Ok(())
}
async fn submit_clip(
    view: Shared,
    bytes: Vec<u8>,
    mime: String,
    generation: u64,
) -> Result<(), String> {
    if view.borrow().generation != generation || view.borrow().snapshot.settings.microphone_muted {
        return Ok(());
    }
    let text = call("transcribe", json!({"audio":bytes,"mime":mime}))
        .await?
        .as_string()
        .ok_or("Transcription returned no text")?;
    submit_transcript(view, text, generation).await
}
async fn submit_transcript(view: Shared, text: String, generation: u64) -> Result<(), String> {
    if view.borrow().generation != generation {
        return Ok(());
    }
    view.borrow_mut().capture_token = None;
    view.borrow_mut().native_manual = false;
    let memory_confirmation = view.borrow_mut().memory_listening.take();
    if let Some(id) = memory_confirmation {
        view.borrow_mut().local_phase = Some("synthesizing");
        let result = call("answer_memory", json!({"id":id,"text":text})).await?;
        if view.borrow().generation != generation {
            return Ok(());
        }
        let answer = result
            .as_string()
            .ok_or("Memory check returned no answer")?;
        let audio = call("speak", json!({"text":answer})).await?;
        if view.borrow().generation != generation {
            return Ok(());
        }
        play_encoded(view.clone(), audio, generation).await?;
        let playback = view.borrow().audio.clone();
        if let Some(audio) = playback {
            wait_for_audio(&view, generation, &audio).await?;
        }
        view.borrow_mut().local_phase = None;
        return Ok(());
    }
    let confirmation = view.borrow_mut().update_listening.take();
    if let Some(version) = confirmation {
        view.borrow_mut().local_phase = None;
        call("answer_update", json!({"text":text,"version":version})).await?;
        return Ok(());
    }
    let request = format!("voice-{:.0}-{generation}", js_sys::Date::now());
    view.borrow_mut().request_id = Some(request.clone());
    let turn = call("send_message", json!({"text":text,"requestId":request}))
        .await?
        .as_string()
        .ok_or("Runtime did not identify the submitted turn")?;
    if view.borrow().generation != generation {
        return Ok(());
    }
    {
        let mut v = view.borrow_mut();
        v.voice_pending = true;
        v.reply_turn = turn;
        v.speech_queue = speech_queue::SpeechQueue::default();
    }
    view.borrow_mut().local_phase = None;
    Ok::<(), String>(())
}
async fn voice(view: Shared) -> Result<(), String> {
    if dialog("panel").open() || dialog("about").open() || !view.borrow().snapshot.window_visible {
        return Err("Close Settings before speaking.".into());
    }
    if view.borrow().native_manual && view.borrow().local_phase == Some("listening") {
        let token = view
            .borrow()
            .capture_token
            .clone()
            .ok_or("Recording is not ready")?;
        call("finish_capture", json!({"token":token})).await?;
        return Ok(());
    }
    let providers: Vec<Value> =
        serde_wasm_bindgen::from_value(call("voice_providers", json!({})).await?)
            .map_err(|e| e.to_string())?;
    let selected = view.borrow().snapshot.settings.voice_provider.clone();
    if !providers
        .iter()
        .any(|p| p["id"] == selected && p["streaming_transcription"] == true)
    {
        return browser_voice(view).await;
    }
    if view.borrow().snapshot.settings.microphone_muted {
        return Err("Microphone is muted".into());
    }
    if view.borrow().local_phase == Some("announcing") {
        return Ok(());
    }
    if matches!(
        view.borrow().snapshot.updates.phase.as_str(),
        "downloading" | "installing"
    ) {
        return Err("An update is being installed".into());
    }
    if let Some(error) = interaction::voice_setup_error(
        view.borrow().snapshot.stt_ready,
        view.borrow().snapshot.key_configured,
        !view.borrow().snapshot.settings.voice_id.is_empty(),
    ) {
        return Err(error.into());
    }
    if view.borrow().snapshot.phase == "thinking" {
        let request_id = view.borrow().snapshot.request_id.clone();
        call("stop", json!({"requestId":request_id})).await?;
    }
    let memory_confirmation = view.borrow_mut().memory_listening.take();
    stop_audio(&view);
    view.borrow_mut().memory_listening = memory_confirmation;
    let generation = view.borrow().generation;
    let token = format!("capture-{:.0}-{generation}", js_sys::Date::now());
    let limit =
        if view.borrow().update_listening.is_some() || view.borrow().memory_listening.is_some() {
            12
        } else {
            60
        };
    {
        let mut v = view.borrow_mut();
        v.native_manual = true;
        v.capture_token = Some(token.clone());
        v.local_phase = Some("listening");
    }
    let result = call("start_capture", json!({"token":token,"limit":limit})).await;
    if view.borrow().generation != generation {
        let _ = call("cancel_capture", json!({"token":token})).await;
        return Ok(());
    }
    if let Err(error) = result {
        view.borrow_mut().native_manual = false;
        view.borrow_mut().capture_token = None;
        view.borrow_mut().local_phase = None;
        return Err(error);
    }
    Ok(())
}
async fn browser_voice(view: Shared) -> Result<(), String> {
    if view.borrow().snapshot.settings.microphone_muted {
        return Err("Microphone is muted. Unmute in Settings or the tray menu.".into());
    }
    call("wake_activity", json!({"ready":false})).await?;
    if view.borrow().local_phase == Some("announcing") {
        return Ok(());
    }

    if matches!(
        view.borrow().snapshot.updates.phase.as_str(),
        "downloading" | "installing"
    ) {
        return Err("An update is being installed".into());
    }
    if let Some(recorder) = view.borrow().recording.clone() {
        recorder.stop().map_err(js_error)?;
        return Ok(());
    }
    if view.borrow().local_phase == Some("listening") {
        return Ok(());
    }
    if view.borrow().local_phase.is_some() {
        let memory_confirmation = view.borrow_mut().memory_listening.take();
        stop_audio(&view);
        view.borrow_mut().memory_listening = memory_confirmation;
    }
    if view.borrow().snapshot.phase == "thinking" {
        let request_id = view.borrow().snapshot.request_id.clone();
        call("stop", json!({"requestId":request_id})).await?;
    }
    let memory_confirmation = view.borrow_mut().memory_listening.take();
    stop_audio(&view);
    view.borrow_mut().memory_listening = memory_confirmation;
    if view.borrow().snapshot.key_checking {
        return Err(
            "Waiting for OS keychain access. Check for a system permission prompt, then try again."
                .into(),
        );
    }
    if let Some(error) = interaction::voice_setup_error(
        view.borrow().snapshot.stt_ready,
        view.borrow().snapshot.key_configured,
        !view.borrow().snapshot.settings.voice_id.is_empty(),
    ) {
        return Err(error.into());
    }
    let generation = view.borrow().generation;
    view.borrow_mut().local_phase = Some("listening");
    let constraints = web_sys::MediaStreamConstraints::new();
    constraints.set_audio(&JsValue::TRUE);
    let result = JsFuture::from(
        window()
            .navigator()
            .media_devices()
            .map_err(js_error)?
            .get_user_media_with_constraints(&constraints)
            .map_err(js_error)?,
    )
    .await;
    if view.borrow().generation != generation {
        if let Ok(value) = result
            && let Ok(stream) = value.dyn_into::<web_sys::MediaStream>()
        {
            for track in stream.get_tracks() {
                let track: web_sys::MediaStreamTrack = track.unchecked_into();
                track.stop();
            }
        }
        return Ok(());
    }
    let stream: web_sys::MediaStream = result.map_err(js_error)?.dyn_into().map_err(js_error)?;
    if view.borrow().generation != generation {
        for track in stream.get_tracks() {
            let track: web_sys::MediaStreamTrack = track.unchecked_into();
            track.stop();
        }
        return Ok(());
    }
    if let Err(error) = view.borrow_mut().meter.microphone(&stream) {
        for track in stream.get_tracks() {
            track.unchecked_into::<web_sys::MediaStreamTrack>().stop();
        }
        return Err(js_error(error));
    }
    let options = web_sys::MediaRecorderOptions::new();
    let mime = if web_sys::MediaRecorder::is_type_supported("audio/mp4") {
        "audio/mp4"
    } else {
        "audio/webm"
    };
    options.set_mime_type(mime);
    let recorder = match web_sys::MediaRecorder::new_with_media_stream_and_media_recorder_options(
        &stream, &options,
    ) {
        Ok(r) => r,
        Err(e) => {
            for t in stream.get_tracks() {
                let t: web_sys::MediaStreamTrack = t.unchecked_into();
                t.stop();
            }
            return Err(js_error(e));
        }
    };
    let chunks = Rc::new(RefCell::new(Vec::<web_sys::Blob>::new()));
    let collected = chunks.clone();
    on(recorder.as_ref(), "dataavailable", move |e| {
        if let Ok(event) = e.dyn_into::<web_sys::BlobEvent>()
            && let Some(blob) = event.data()
        {
            collected.borrow_mut().push(blob);
        }
    });
    let target = view.clone();
    let media_type = mime.to_string();
    let stream_to_stop = stream.clone();
    on(recorder.as_ref(), "stop", move |_| {
        for track in stream_to_stop.get_tracks() {
            let track: web_sys::MediaStreamTrack = track.unchecked_into();
            track.stop();
        }
        if target.borrow().generation != generation {
            return;
        }
        {
            let mut v = target.borrow_mut();
            v.recording = None;
            v.stream = None;
            v.meter.clear_microphone();
            let discard = v.discard_capture
                || ((v.update_listening.is_none() && v.memory_listening.is_none())
                    && v.speech_end
                        .as_ref()
                        .is_some_and(|capture| !capture.heard_speech()));
            v.speech_end = None;
            v.discard_capture = false;
            v.local_phase = if discard { None } else { Some("transcribing") };
            if discard {
                return;
            }
        }
        let pieces = js_sys::Array::new();
        for blob in chunks.borrow_mut().drain(..) {
            pieces.push(&blob);
        }
        let view = target.clone();
        let mime = media_type.clone();
        spawn_local(async move {
            let result = async {
                let blob = web_sys::Blob::new_with_blob_sequence(&pieces).map_err(js_error)?;
                if blob.size() > 10_000_000. {
                    return Err("Recording exceeded 10 MB".into());
                }
                let bytes = js_sys::Uint8Array::new(
                    &JsFuture::from(blob.array_buffer())
                        .await
                        .map_err(js_error)?,
                )
                .to_vec();
                submit_clip(view.clone(), bytes, mime, generation).await
            }
            .await;
            if let Err(e) = result
                && view.borrow().generation == generation
            {
                message(&e);
                view.borrow_mut().local_phase = None;
                view.borrow_mut().snapshot.phase = "error".into();
                open_panel(&view);
            }
        });
    });
    if let Err(error) = recorder.start() {
        for track in stream.get_tracks() {
            let track: web_sys::MediaStreamTrack = track.unchecked_into();
            track.stop();
        }
        return Err(js_error(error));
    }
    {
        let mut v = view.borrow_mut();
        v.speech_end = Some(interaction::SpeechEnd::new(
            window().performance().map_or(0., |p| p.now()),
        ));
        v.discard_capture = false;
        v.recording = Some(recorder.clone());
        v.stream = Some(stream);
        v.snapshot.phase = "listening".into();
    }
    let recording_limit =
        if view.borrow().update_listening.is_some() || view.borrow().memory_listening.is_some() {
            12_000
        } else {
            60_000
        };
    let deadline = Closure::once(move || {
        if recorder.state() == web_sys::RecordingState::Recording {
            let _ = recorder.stop();
        }
    });
    window()
        .set_timeout_with_callback_and_timeout_and_arguments_0(
            deadline.as_ref().unchecked_ref(),
            recording_limit,
        )
        .map_err(js_error)?;
    deadline.forget();
    Ok(())
}
#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    let view = Rc::new(RefCell::new(View {
        snapshot: Snapshot {
            settings: Settings {
                name: "notdot".into(),
                hue: 192,
                ..Settings::default()
            },
            phase: "connecting".into(),
            ..Snapshot::default()
        },
        ..View::default()
    }));
    let canvas: HtmlCanvasElement = element("orb").unchecked_into();
    let target = view.clone();
    on(canvas.as_ref(), "contextmenu", move |e| {
        e.prevent_default();
        open_panel(&target);
    });
    let origin = Rc::new(RefCell::new(interaction::Gesture::default()));
    let point = origin.clone();
    on(canvas.as_ref(), "mousedown", move |e| {
        if let Ok(e) = e.dyn_into::<web_sys::MouseEvent>() {
            point.borrow_mut().press(e.client_x(), e.client_y());
        }
    });
    let down = origin.clone();
    on(canvas.as_ref(), "mousemove", move |event| {
        if let Ok(event) = event.dyn_into::<web_sys::MouseEvent>()
            && down
                .borrow_mut()
                .moved(event.client_x(), event.client_y(), event.buttons() == 1)
        {
            let _ = drag();
        }
    });
    let target = view.clone();
    on(canvas.as_ref(), "click", move |e| {
        if !target.borrow().initialized || target.borrow().snapshot.settings.wake_enabled {
            return;
        }
        let Ok(e) = e.dyn_into::<web_sys::MouseEvent>() else {
            return;
        };
        if !origin
            .borrow()
            .activates(e.client_x(), e.client_y(), e.button(), e.ctrl_key())
        {
            return;
        }
        let _ = target.borrow_mut().meter.unlock();
        let target = target.clone();
        spawn_local(async move {
            if let Err(e) = voice(target.clone()).await {
                target.borrow_mut().local_phase = None;
                message(&e);
                open_panel(&target);
            }
        });
    });
    on(element("close-about").as_ref(), "click", |_| {
        dialog("about").close();
        if !dialog("panel").open() {
            let _ = resize(360., 360.);
        }
    });
    on(window().as_ref(), "notdot-about", |_| {
        let _ = resize(430., 540.);
        let _ = dialog("about").show_modal();
    });
    on(element("show-about").as_ref(), "click", |_| {
        let _ = dialog("about").show_modal();
    });
    on(element("refresh-usage").as_ref(), "click", |_| {
        spawn_local(async {
            match call("refresh_usage", json!({})).await {
                Err(e) => message(e),
                Ok(_) => {
                    if element("error")
                        .text_content()
                        .is_some_and(|text| text.starts_with("Usage unavailable:"))
                    {
                        element("error").set_text_content(None);
                    }
                }
            }
        });
    });
    on(element("check-updates").as_ref(), "click", |_| {
        spawn_local(async {
            if let Err(e) = call("check_updates", json!({})).await {
                message(e);
            }
        });
    });
    on(element("defer-update").as_ref(), "click", |_| {
        spawn_local(async {
            if let Err(e) = call("defer_update", json!({})).await {
                message(e);
            }
        });
    });
    let update_target = view.clone();
    on(element("install-update").as_ref(), "click", move |_| {
        stop_audio(&update_target);
        spawn_local(async {
            if let Err(e) = call("install_update", json!({})).await {
                message(e);
            }
        });
    });
    let target = view.clone();
    on(window().as_ref(), "notdot-mute", move |_| {
        mute_capture(&target);
        target.borrow_mut().snapshot.settings.microphone_muted = true;
        input("microphone-muted").set_checked(true);
    });
    let target = view.clone();
    on(window().as_ref(), "notdot-settings", move |_| {
        open_panel(&target)
    });
    let updates_view = view.clone();
    on(window().as_ref(), "notdot-updates", move |_| {
        open_panel(&updates_view);
        let section = element("startup-section");
        let _ = section.set_attribute("open", "");
        element("check-updates").scroll_into_view();
        let _ = element("check-updates")
            .dyn_into::<web_sys::HtmlElement>()
            .map(|button| button.focus());
    });
    let target = view.clone();
    let focus_view = view.clone();
    on(document().as_ref(), "keydown", move |event| {
        if let Ok(key) = event.dyn_into::<web_sys::KeyboardEvent>()
            && matches!(
                key.key().as_str(),
                "Tab"
                    | "Enter"
                    | " "
                    | "Escape"
                    | ","
                    | "ArrowUp"
                    | "ArrowDown"
                    | "ArrowLeft"
                    | "ArrowRight"
            )
        {
            focus_view.borrow_mut().keyboard_navigation = true;
        }
    });
    let focus_view = view.clone();
    on(document().as_ref(), "pointerdown", move |_| {
        focus_view.borrow_mut().keyboard_navigation = false;
    });
    let focus_view = view.clone();
    on(window().as_ref(), "blur", move |_| {
        focus_view.borrow_mut().keyboard_navigation = false;
    });
    on(window().as_ref(), "keydown", move |e| {
        if let Ok(e) = e.dyn_into::<web_sys::KeyboardEvent>() {
            let modal = dialog("panel").open()
                || dialog("about").open()
                || document()
                    .get_element_by_id("composer")
                    .is_some_and(|e| e.unchecked_into::<HtmlDialogElement>().open());
            let action = interaction::keyboard_action(
                &e.key(),
                e.repeat(),
                e.meta_key() || e.ctrl_key(),
                e.alt_key(),
                modal,
                cfg!(feature = "debug-ui"),
                target.borrow().snapshot.settings.wake_enabled,
            );
            if action == Some(interaction::KeyboardAction::Settings) {
                e.prevent_default();
                open_panel(&target);
                return;
            }
            if action == Some(interaction::KeyboardAction::Cancel) {
                stop_audio(&target);
            }
            if action == Some(interaction::KeyboardAction::Voice) {
                e.prevent_default();
                let _ = target.borrow_mut().meter.unlock();
                let target = target.clone();
                spawn_local(async move {
                    if let Err(e) = voice(target.clone()).await {
                        target.borrow_mut().local_phase = None;
                        message(e);
                        open_panel(&target);
                    }
                });
            }
            #[cfg(feature = "debug-ui")]
            if e.key() == "Enter" && !dialog("panel").open() && !dialog("composer").open() {
                e.prevent_default();
                let _ = resize(430., 430.);
                let _ = dialog("composer").show_modal();
                let _ = element("text")
                    .unchecked_into::<web_sys::HtmlTextAreaElement>()
                    .focus();
            }
        }
    });
    let target = view.clone();
    on(element("close").as_ref(), "click", move |_| {
        close_panel(&target)
    });
    let target = view.clone();
    on(element("panel").as_ref(), "cancel", move |_| {
        target.borrow_mut().preview_hue = None;
        apply_theme(target.borrow().snapshot.settings.hue);
        let _ = resize(360., 360.);
    });
    #[cfg(feature = "debug-ui")]
    mount_debug(&view);
    let target = view.clone();
    on(input("hue").as_ref(), "input", move |_| {
        let hue = input("hue").value().parse::<u16>().unwrap_or(192).min(360);
        target.borrow_mut().preview_hue = Some(hue);
        apply_theme(hue);
    });
    let target = view.clone();
    on(element("stt-provider").as_ref(), "change", |_| {
        show_stt_fields();
    });
    on(element("authorize-stt-key").as_ref(), "click", |_| {
        let _ = element("authorize-stt-key").set_attribute("disabled", "");
        element("stt-key-status").set_text_content(Some(
            "Approve your computer’s saved-key request. You do not need to enter the API key again.",
        ));
        spawn_local(async {
            match call("authorize_voice_key", json!({"provider":"deepgram"})).await {
                Ok(_) => message("Deepgram key access allowed."),
                Err(e) => message(e),
            }
            let _ = element("authorize-stt-key").remove_attribute("disabled");
        });
    });
    on(element("authorize-key").as_ref(), "click", |_| {
        let provider = select("provider").value();
        let _ = element("authorize-key").set_attribute("disabled", "");
        element("key-access-status")
            .set_text_content(Some("Approve your computer’s saved-key request. You do not need to enter the API key again."));
        spawn_local(async move {
            let result = call("authorize_voice_key", json!({"provider":provider})).await;
            let _ = element("authorize-key").remove_attribute("disabled");
            match result {
                Ok(_) => message("Your saved key is ready."),
                Err(e) => message(e),
            }
        });
    });
    on(element("load-voices").as_ref(), "click", move |_| {
        let view = target.clone();
        spawn_local(async move {
            if let Err(e) = load_catalog(view, false).await {
                message(e);
            }
        });
    });
    let target = view.clone();
    on(element("more-voices").as_ref(), "click", move |_| {
        let view = target.clone();
        spawn_local(async move {
            if let Err(e) = load_catalog(view, true).await {
                message(e);
            }
        });
    });
    let target = view.clone();
    on(element("provider").as_ref(), "change", move |_| {
        target.borrow_mut().voices.clear();
        target.borrow_mut().next_cursor = None;
        render_voice_options(&target, "", "");
        input("voice-search").set_value("");
        message("Enter this backend's key, then load voices.");
    });
    let target = view.clone();
    on(element("voice-results").as_ref(), "click", |event| {
        let Some(button) = event
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        else {
            return;
        };
        let Some(id) = button.get_attribute("data-voice-id") else {
            return;
        };
        select("voice").set_value(&id);
        element("voice-value").set_text_content(button.text_content().as_deref());
        let _ = element("preview-voice").remove_attribute("disabled");
        element("voice-info").set_text_content(button.get_attribute("data-description").as_deref());
        close_voice_picker();
        let _ = element("voice-toggle")
            .unchecked_into::<web_sys::HtmlElement>()
            .focus();
    });
    on(element("voice-toggle").as_ref(), "click", |_| {
        if element("voice-popup").has_attribute("hidden") {
            let _ = element("voice-popup").remove_attribute("hidden");
            let _ = element("voice-toggle").set_attribute("aria-expanded", "true");
            if !element("voice-search-label").has_attribute("hidden") {
                let _ = input("voice-search").focus();
            }
        } else {
            close_voice_picker();
        }
    });
    on(element("voice-popup").as_ref(), "keydown", |event| {
        if let Ok(key) = event.dyn_into::<web_sys::KeyboardEvent>() {
            if key.key() == "Escape" {
                key.prevent_default();
                key.stop_propagation();
                close_voice_picker();
                let _ = element("voice-toggle")
                    .unchecked_into::<web_sys::HtmlElement>()
                    .focus();
            } else if key.key() == "ArrowDown"
                && key
                    .target()
                    .is_some_and(|t| t == element("voice-search").into())
            {
                key.prevent_default();
                if let Some(first) = element("voice-results").first_element_child() {
                    let _ = first.unchecked_into::<web_sys::HtmlElement>().focus();
                }
            }
        }
    });
    on(element("voice-search").as_ref(), "input", move |_| {
        let generation = {
            let mut v = target.borrow_mut();
            v.catalog_generation += 1;
            v.catalog_generation
        };
        let view = target.clone();
        let callback = Closure::once(move || {
            if view.borrow().catalog_generation == generation {
                spawn_local(async move {
                    if let Err(e) = load_catalog(view, false).await {
                        message(e);
                    }
                });
            }
        });
        let _ = window().set_timeout_with_callback_and_timeout_and_arguments_0(
            callback.as_ref().unchecked_ref(),
            300,
        );
        callback.forget();
    });
    let target = view.clone();
    on(element("voice").as_ref(), "change", move |_| {
        let id = select("voice").value();
        let view = target.borrow();
        element("voice-info").set_text_content(
            view.voices
                .iter()
                .find(|v| v.id == id)
                .map(|v| v.description.as_str()),
        );
    });
    let target = view.clone();
    on(element("preview-voice").as_ref(), "click", move |_| {
        let id = select("voice").value();
        if id.is_empty() {
            message("Choose a voice first");
            return;
        }
        stop_audio(&target);
        let _ = target.borrow_mut().meter.unlock();
        let generation = target.borrow().generation;
        target.borrow_mut().local_phase = Some("synthesizing");
        let view = target.clone();
        let provider = select("provider").value();
        element("voice-info").set_text_content(Some("Preparing preview…"));
        spawn_local(async move {
            let result = async {
                let clip = call("preview_voice", json!({"provider":provider,"voiceId":id})).await?;
                play_encoded(view.clone(), clip, generation).await
            }
            .await;
            if view.borrow().generation == generation {
                view.borrow_mut().local_phase = None;
            }
            if view.borrow().generation == generation {
                match result {
                    Ok(()) => element("voice-info").set_text_content(Some("Playing preview")),
                    Err(e) => message(e),
                }
            }
        });
    });
    on(element("panel-drag").as_ref(), "pointerdown", |event| {
        if let Ok(pointer) = event.dyn_into::<web_sys::MouseEvent>()
            && pointer.button() == 0
        {
            pointer.prevent_default();
            spawn_local(async {
                let _ = JsFuture::from(drag()).await;
            });
        }
    });
    for field in [
        "name",
        "daily-limit",
        "followup-gap",
        "quiet-start",
        "quiet-end",
    ] {
        on(element(field).as_ref(), "invalid", |event| {
            if let Some(target) = event
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                && let Ok(Some(section)) = target.closest("details.settings-section")
            {
                let _ = section.set_attribute("open", "");
            }
        });
    }
    on(element("settings").as_ref(), "keydown", |event| {
        if let Ok(key) = event.dyn_into::<web_sys::KeyboardEvent>()
            && key.key() == "Enter"
            && !key
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                .is_some_and(|e| matches!(e.tag_name().as_str(), "BUTTON" | "SUMMARY"))
        {
            key.prevent_default();
        }
    });
    let target = view.clone();
    let models_view = view.clone();
    on(element("default-model").as_ref(), "click", move |_| {
        select("model").set_value("gpt-6.1-terra");
        render_model_cards(&models_view);
        render_efforts(&models_view, "medium", false);
        element("model-status").set_text_content(Some("Terra · Medium selected. Save to apply."));
    });
    on(element("intelligence-section").as_ref(), "toggle", |_| {
        let open = element("intelligence-section").has_attribute("open");
        let _ = element("intelligence-toggle")
            .set_attribute("aria-expanded", if open { "true" } else { "false" });
        if open {
            let _ = input("model-search").focus();
        }
    });
    on(element("intelligence-done").as_ref(), "click", |_| {
        close_intelligence_picker();
        let _ = element("intelligence-toggle")
            .unchecked_into::<web_sys::HtmlElement>()
            .focus();
    });
    on(
        element("intelligence-picker").as_ref(),
        "keydown",
        |event| {
            if let Ok(key) = event.dyn_into::<web_sys::KeyboardEvent>()
                && key.key() == "Escape"
            {
                key.prevent_default();
                key.stop_propagation();
                close_intelligence_picker();
                let _ = element("intelligence-toggle")
                    .unchecked_into::<web_sys::HtmlElement>()
                    .focus();
            }
        },
    );
    let models_view = view.clone();
    on(element("model-search").as_ref(), "input", move |_| {
        filter_model_cards(&models_view)
    });
    let models_view = view.clone();
    on(element("model-options").as_ref(), "change", move |event| {
        if let Some(radio) = event
            .target()
            .and_then(|t| t.dyn_into::<HtmlInputElement>().ok())
        {
            select("model").set_value(&radio.value());
            let effort = select("effort").value();
            render_efforts(&models_view, &effort, true);
        }
    });
    let models_view = view.clone();
    on(element("effort-options").as_ref(), "change", move |event| {
        if let Some(radio) = event
            .target()
            .and_then(|t| t.dyn_into::<HtmlInputElement>().ok())
        {
            select("effort").set_value(&radio.value());
            render_effort_description(&models_view);
        }
    });
    let models_view = view.clone();
    on(element("refresh-models").as_ref(), "click", move |_| {
        let model = select("model").value();
        let effort = select("effort").value();
        refresh_models(&models_view, model, effort);
    });
    on(element("start-at-login").as_ref(), "change", |_| {
        select("startup").set_disabled(!input("start-at-login").checked());
    });
    on(element("settings").as_ref(), "submit", move |e| {
        e.prevent_default();
        let mut settings = target.borrow().snapshot.settings.clone();
        settings.name = input("name").value();
        if !select("model").disabled() {
            let model = select("model").value();
            let effort = select("effort").value();
            if model.is_empty() || effort.is_empty() {
                message("Choose an available model and effort before saving.");
                return;
            }
            settings.model = model;
            settings.effort = effort;
        }
        settings.hue = input("hue").value().parse().unwrap_or(192);
        settings.voice_id = select("voice").value();
        settings.voice_provider = select("provider").value();
        settings.voice_name = element("voice-value").text_content().unwrap_or_default();
        if settings.voice_id.is_empty() {
            settings.voice_name.clear();
        }
        settings.speech_language = select("speech-language").value();
        settings.stt_provider = select("stt-provider").value();
        settings.memory_enabled = input("memory").checked();
        let minutes = |id: &str| {
            let value = input(id).value();
            let (h, m) = value.split_once(':').unwrap_or(("0", "0"));
            h.parse::<u16>().unwrap_or(0) * 60 + m.parse::<u16>().unwrap_or(0)
        };
        settings.initiative = json!({"behaviour":select("behaviour").value(),"quiet_start":minutes("quiet-start"),"quiet_end":minutes("quiet-end"),"daily_limit":input("daily-limit").value().parse::<u8>().unwrap_or(3),"gap_minutes":input("followup-gap").value().parse::<u16>().unwrap_or(60)});
        settings.startup = if input("start-at-login").checked() {
            select("startup").value()
        } else {
            "manual".into()
        };
        settings.open_desktop = input("open-desktop").checked();
        settings.automatic_updates = input("automatic-updates").checked();
        settings.wake_enabled = input("wake-enabled").checked();
        settings.wake_name_only = select("wake-phrase").value() == "name";
        settings.microphone_muted = input("microphone-muted").checked();
        settings.onboarding_complete = true;
        let key = input("key").value();
        let stt_key = input("stt-key").value();
        let target = target.clone();
        spawn_local(async move {
            match call(
                "save_settings",
                json!({"settings":settings,"key":key,"sttKey":stt_key,"validateModel":!select("model").disabled()}),
            )
            .await
            {
                Ok(_) => {
                    input("key").set_value("");
                    input("stt-key").set_value("");
                    message("");
                    target.borrow_mut().snapshot.settings = settings;
                    close_panel(&target);
                }
                Err(e) => message(e),
            }
        });
    });
    on(element("memories").as_ref(), "click", |_| {
        spawn_local(async {
            if let Err(e) = review_memories().await {
                message(e);
            }
        });
    });
    on(element("review-followups").as_ref(), "click", |_| {
        spawn_local(async {
            if let Err(e) = review_followups().await {
                message(e);
            }
        });
    });
    on(element("followups").as_ref(), "click", |e| {
        let Some(t) = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        else {
            return;
        };
        let (Some(id), Some(action)) = (
            t.get_attribute("data-followup-id"),
            t.get_attribute("data-action"),
        ) else {
            return;
        };
        spawn_local(async move {
            match call("followup_action", json!({"id":id,"action":action})).await {
                Ok(_) => {
                    if let Err(e) = review_followups().await {
                        message(e);
                    }
                }
                Err(e) => message(e),
            }
        });
    });
    on(element("notes").as_ref(), "click", |e| {
        let Some(target) = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        else {
            return;
        };
        let Some(id) = target.get_attribute("data-memory-id") else {
            return;
        };
        let action = target
            .get_attribute("data-action")
            .unwrap_or_else(|| "forget".into());
        if action == "edit" {
            if let Some(editor) = document().get_element_by_id(&format!("edit-{id}")) {
                let _ = editor.remove_attribute("hidden");
                if let Some(editor) = editor.dyn_ref::<web_sys::HtmlTextAreaElement>() {
                    let _ = editor.focus();
                }
            }
            return;
        }
        let content = document()
            .get_element_by_id(&format!("edit-{id}"))
            .and_then(|e| e.dyn_into::<web_sys::HtmlTextAreaElement>().ok())
            .map(|e| e.value())
            .unwrap_or_default();
        spawn_local(async move {
            match call(
                if action == "save" {
                    "edit_memory"
                } else {
                    "forget"
                },
                json!({"id":id,"content":content}),
            )
            .await
            {
                Ok(_) => {
                    message(if action == "save" {
                        "Memory corrected. Your version takes precedence."
                    } else {
                        "Note and derived summaries forgotten."
                    });
                    if let Err(e) = review_memories().await {
                        message(e);
                    }
                }
                Err(e) => message(e),
            }
        });
    });
    on(element("maintain").as_ref(), "click", |_| {
        spawn_local(async {
            match call("maintain_memory", json!({})).await {
                Ok(_) => {
                    message("Notes tidied. Checking for useful summaries…");
                    if let Err(e) = review_memories().await {
                        message(e);
                    }
                }
                Err(e) => message(e),
            }
        });
    });
    on(element("reset-memory").as_ref(), "click", |_| {
        let _ = element("reset-memory-confirm").remove_attribute("hidden");
        if let Some(button) = element("cancel-memory-reset").dyn_ref::<web_sys::HtmlElement>() {
            let _ = button.focus();
        }
    });
    on(element("cancel-memory-reset").as_ref(), "click", |_| {
        let _ = element("reset-memory-confirm").set_attribute("hidden", "");
        if let Some(button) = element("reset-memory").dyn_ref::<web_sys::HtmlElement>() {
            let _ = button.focus();
        }
    });
    let reset_view = view.clone();
    on(
        element("confirm-memory-reset").as_ref(),
        "click",
        move |_| {
            stop_audio(&reset_view);
            let _ = element("confirm-memory-reset").set_attribute("disabled", "");
            let _ = element("cancel-memory-reset").set_attribute("disabled", "");
            element("reset-memory-status")
                .set_text_content(Some("Resetting memory and restarting notdot…"));
            reset_view.borrow_mut().local_phase = Some("resetting");
            let view = reset_view.clone();
            spawn_local(async move {
                if let Err(e) = call("reset_memory", json!({"confirmed":true})).await {
                    view.borrow_mut().local_phase = None;
                    let _ = element("confirm-memory-reset").remove_attribute("disabled");
                    let _ = element("cancel-memory-reset").remove_attribute("disabled");
                    element("reset-memory-status").set_text_content(Some(&e));
                }
            });
        },
    );
    on(element("clear-key").as_ref(), "click", |_| {
        spawn_local(async {
            match call("clear_voice_key", json!({})).await {
                Ok(_) => message("Voice key removed"),
                Err(e) => message(e),
            }
        });
    });
    let reconnect_view = view.clone();
    on(element("retry").as_ref(), "click", move |_| {
        stop_audio(&reconnect_view);
        spawn_local(async {
            if let Err(e) = call("reconnect", json!({})).await {
                message(e);
            }
        });
    });
    on(element("name").as_ref(), "input", |_| render_wake_phrase());
    on(element("wake-phrase").as_ref(), "change", |_| {
        render_wake_phrase()
    });
    let muted_view = view.clone();
    on(element("microphone-muted").as_ref(), "change", move |_| {
        let muted = input("microphone-muted").checked();
        if muted {
            mute_capture(&muted_view);
        }
        muted_view.borrow_mut().snapshot.settings.microphone_muted = muted;
        spawn_local(async move {
            if let Err(e) = call("set_microphone_muted", json!({"muted":muted})).await {
                message(e);
            }
        });
    });
    let capture_view = view.clone();
    let capture_callback = Closure::<dyn FnMut(JsValue)>::new(move |payload: JsValue| {
        let Ok(event) = serde_wasm_bindgen::from_value::<Value>(payload) else {
            return;
        };
        let owner = capture_view.clone();
        let generation = owner.borrow().generation;
        spawn_local(async move {
            let Ok(value) = call("capture_status", json!({})).await else {
                return;
            };
            let Ok(wake) = serde_wasm_bindgen::from_value::<Value>(value) else {
                return;
            };
            if owner.borrow().generation != generation {
                return;
            }
            let token = event["token"].as_str().unwrap_or("");
            if matches!(event["phase"].as_str(), Some("empty" | "cancelled"))
                && owner.borrow().capture_token.as_deref() == Some(token)
            {
                let failure =
                    (wake["capture_token"] == token && wake["phase"] == "error").then(|| {
                        wake["message"]
                            .as_str()
                            .unwrap_or("Microphone could not continue")
                            .to_owned()
                    });
                stop_audio(&owner);
                owner.borrow_mut().update_listening = None;
                if let Some(failure) = failure {
                    message(failure);
                    open_panel(&owner);
                }
                return;
            }
            if wake["capture_token"] != token {
                return;
            }
            let ready = {
                let snapshot = owner.borrow().snapshot.clone();
                wake_ready(&owner, &snapshot)
            };
            if wake["phase"] == "capturing" && ready {
                let mut v = owner.borrow_mut();
                if wake["manual"] != true && v.last_capture_cue != token {
                    let _ = v.meter.wake_cue();
                    v.last_capture_cue = token.into();
                }
                v.capture_feedback_events += 1;
                v.capture_token = Some(token.into());
                v.snapshot.wake = wake;
                v.last_poll_at = 0.;
            } else if wake["phase"] == "ready" {
                owner.borrow_mut().last_poll_at = 0.;
            }
        });
    });
    let subscription = listen_capture(capture_callback.as_ref().unchecked_ref());
    spawn_local(async move {
        let _ = JsFuture::from(subscription).await;
    });
    capture_callback.forget();
    let target = view.clone();
    let first = Rc::new(RefCell::new(true));
    let tick = Closure::<dyn FnMut()>::new(move || {
        {
            let mut v = target.borrow_mut();
            let now = js_sys::Date::now();
            let fast = (v.snapshot.settings.wake_enabled || v.native_manual)
                && v.snapshot.window_visible
                && matches!(
                    v.snapshot.wake["phase"].as_str(),
                    Some("listening" | "capturing")
                );
            let interval = if fast { 200. } else { 500. };
            if v.polling || (now >= v.last_poll_at && now - v.last_poll_at < interval) {
                return;
            }
            v.last_poll_at = now;
            v.polling = true;
        }
        let target = target.clone();
        let first = first.clone();
        spawn_local(async move {
            match call("snapshot", json!({})).await.and_then(|v| {
                serde_wasm_bindgen::from_value::<Snapshot>(v).map_err(|e| e.to_string())
            }) {
                Ok(mut snapshot) => {
                    if !snapshot.window_visible && target.borrow().memory_listening.is_some() {
                        stop_audio(&target);
                    }
                    if snapshot.settings.microphone_muted
                        != target.borrow().snapshot.settings.microphone_muted
                    {
                        if snapshot.settings.microphone_muted {
                            mute_capture(&target);
                        }
                        input("microphone-muted").set_checked(snapshot.settings.microphone_muted);
                    }
                    if snapshot.wake["phase"] == "capturing" && !snapshot.settings.microphone_muted
                    {
                        let token = snapshot.wake["capture_token"]
                            .as_str()
                            .unwrap_or("")
                            .to_owned();
                        let mut v = target.borrow_mut();
                        if snapshot.wake["manual"] != true && v.last_capture_cue != token {
                            let _ = v.meter.wake_cue();
                            v.last_capture_cue = token.clone();
                        }
                        v.capture_token = Some(token);
                    }
                    if target.borrow().native_manual && snapshot.wake["phase"] == "error" {
                        let error = snapshot.wake["message"]
                            .as_str()
                            .unwrap_or("Microphone could not start")
                            .to_owned();
                        stop_audio(&target);
                        message(error);
                        open_panel(&target);
                    }
                    let ready = wake_ready(&target, &snapshot);
                    let wake_id = if ready {
                        snapshot.wake["request_id"].as_u64()
                    } else {
                        None
                    };
                    element("wake-status").set_text_content(Some(wake_hint(&snapshot)));
                    let orb = element("orb");
                    let wake_mode = snapshot.settings.wake_enabled;
                    let _ = orb.set_attribute("role", if wake_mode { "img" } else { "button" });
                    let _ = orb.set_attribute(
                        "aria-keyshortcuts",
                        if wake_mode {
                            "Meta+, Control+, Escape"
                        } else {
                            "Enter Space Meta+, Control+, Escape"
                        },
                    );
                    let _ = orb.set_attribute("aria-label", if wake_mode { "notdot wake-word companion. Say your wake phrase to speak. Right-click or Command/Control plus comma opens Settings. Escape cancels." } else { "notdot voice companion. Enter or Space to speak. Command or Control plus comma opens Settings. Escape stops." });
                    let _ = element("orb").set_attribute(
                        "title",
                        if snapshot.settings.microphone_muted {
                            "Microphone muted"
                        } else if snapshot.wake["phase"] == "listening" {
                            snapshot.wake["phrase"].as_str().unwrap_or("Wake listening")
                        } else if snapshot.settings.wake_enabled {
                            wake_hint(&snapshot)
                        } else {
                            "Click to speak · right-click for Settings"
                        },
                    );
                    if let Some(id) = wake_id {
                        target.borrow_mut().local_phase = Some("transcribing");
                        let generation = target.borrow().generation;
                        let companion = target.clone();
                        spawn_local(async move {
                            if let Err(e) = wake_request(companion.clone(), id, generation).await
                                && companion.borrow().generation == generation
                            {
                                companion.borrow_mut().local_phase = None;
                                message(e);
                                open_panel(&companion);
                            }
                        });
                    } else {
                        let _ = call("wake_activity", json!({"ready":ready})).await;
                    }
                    let speak = {
                        let v = target.borrow();
                        if v.recording.is_some() {
                            snapshot.phase = "listening".into();
                        }
                        v.voice_pending && !v.speech_worker
                    };
                    if let Some(reply) = document().get_element_by_id("reply") {
                        reply.set_text_content(Some(
                            snapshot.error.as_deref().unwrap_or(&snapshot.reply),
                        ));
                    }
                    if let Some(error) = &snapshot.error {
                        element("error").set_text_content(Some(error));
                    }
                    if input("key").value().is_empty() {
                        input("key").set_placeholder(if snapshot.key_checking {
                            "Checking OS keychain…"
                        } else if snapshot.key_configured {
                            "Key saved · leave blank to keep"
                        } else {
                            "New key, or use a saved key below"
                        });
                    }
                    element("followup-status").set_text_content(Some(&format!(
                        "{} available follow-ups. {}",
                        snapshot.initiative["available"].as_u64().unwrap_or(0),
                        snapshot.initiative["error"].as_str().unwrap_or("")
                    )));
                    let _ = element("authorize-key")
                        .toggle_attribute_with_force("hidden", snapshot.key_authorized);
                    if !element("authorize-stt-key").has_attribute("disabled") {
                        element("stt-key-status").set_text_content(Some(if snapshot.stt_ready && snapshot.settings.stt_provider=="deepgram" {"Deepgram is ready."}else{"Already saved a Deepgram key? Allow access above. Otherwise, enter a new key."}));
                    }
                    if !element("authorize-key").has_attribute("disabled") {
                        element("key-access-status").set_text_content(Some(if snapshot.key_authorized {"Your saved key is ready."} else if snapshot.key_checking {"Checking your saved key…"} else {"Already saved a key? Allow access above; you do not need to enter it again."}));
                    }
                    element("memory-status")
                        .set_text_content(snapshot.learning["message"].as_str());
                    element("about-version")
                        .set_text_content(Some(&format!("Version {}", snapshot.version)));
                    element("update-status").set_text_content(Some(&snapshot.updates.message));
                    let available = snapshot.updates.phase == "available";
                    let _ =
                        element("install-update").toggle_attribute_with_force("hidden", !available);
                    let _ =
                        element("defer-update").toggle_attribute_with_force("hidden", !available);
                    let updating = matches!(
                        snapshot.updates.phase.as_str(),
                        "downloading" | "installing"
                    );
                    let _ =
                        element("update-progress").toggle_attribute_with_force("hidden", !updating);
                    element("update-progress-label")
                        .set_text_content(Some(&snapshot.updates.message));
                    if let Some(total) = snapshot.updates.total.filter(|t| *t > 0) {
                        let percent =
                            (snapshot.updates.received as f64 / total as f64 * 100.).min(100.);
                        let _ = element("update-bar").set_attribute("value", &percent.to_string());
                    } else {
                        let _ = element("update-bar").remove_attribute("value");
                    }
                    if target.borrow().quota_display.as_ref() != Some(&snapshot.quota) {
                        render_usage(&snapshot.quota);
                        target.borrow_mut().quota_display = Some(snapshot.quota.clone());
                    }
                    let usage_notice = {
                        let v = target.borrow();
                        let ready = snapshot.phase == "idle"
                            && snapshot.wake["phase"] != "capturing"
                            && snapshot.window_visible
                            && !speak
                            && !v.voice_pending
                            && v.local_phase.is_none()
                            && v.recording.is_none()
                            && !dialog("about").open()
                            && (snapshot.quota["fault"].is_object() || !dialog("panel").open())
                            && v.audio.as_ref().is_none_or(|a| a.paused() || a.ended())
                            && js_sys::Date::now() >= v.quota_retry_at;
                        if ready {
                            snapshot.quota["notice_id"].as_str().map(str::to_owned)
                        } else {
                            None
                        }
                    };
                    let announce = {
                        let v = target.borrow();
                        usage_notice.is_none()
                            && snapshot.wake["phase"] != "capturing"
                            && snapshot.updates.prompt
                            && snapshot.phase == "idle"
                            && !speak
                            && !v.voice_pending
                            && v.local_phase.is_none()
                            && v.recording.is_none()
                            && !dialog("panel").open()
                            && !dialog("about").open()
                            && v.audio.as_ref().is_none_or(|a| a.paused() || a.ended())
                            && v.update_announced != snapshot.updates.version
                    };
                    let followup = {
                        let v = target.borrow();
                        if usage_notice.is_none()
                            && !announce
                            && snapshot.phase == "idle"
                            && snapshot.wake["phase"] != "capturing"
                            && !speak
                            && !v.voice_pending
                            && v.local_phase.is_none()
                            && v.recording.is_none()
                            && !dialog("panel").open()
                            && !dialog("about").open()
                            && v.audio.as_ref().is_none_or(|a| a.paused() || a.ended())
                        {
                            snapshot.initiative["announce"].as_str().map(str::to_owned)
                        } else {
                            None
                        }
                    };
                    let memory_check = {
                        let v = target.borrow();
                        if followup.is_none()
                            && usage_notice.is_none()
                            && !announce
                            && snapshot.phase == "idle"
                            && snapshot.window_visible
                            && snapshot.key_configured
                            && !matches!(
                                snapshot.updates.phase.as_str(),
                                "downloading" | "installing"
                            )
                            && !snapshot.settings.microphone_muted
                            && !v.voice_pending
                            && v.local_phase.is_none()
                            && v.recording.is_none()
                            && !v.native_manual
                            && v.memory_listening.is_none()
                            && snapshot.wake["phase"] != "capturing"
                            && !dialog("panel").open()
                            && !dialog("about").open()
                            && v.audio.as_ref().is_none_or(|a| a.paused() || a.ended())
                        {
                            snapshot.learning["confirmation"]
                                .as_str()
                                .map(str::to_owned)
                        } else {
                            None
                        }
                    };
                    if dialog("panel").open()
                        && !target.borrow().model_refreshing
                        && js_sys::Date::now() >= target.borrow().model_refresh_at
                    {
                        let model = select("model").value();
                        let effort = select("effort").value();
                        refresh_models(&target, model, effort);
                    }
                    let onboard = !snapshot.settings.onboarding_complete;
                    target.borrow_mut().snapshot = snapshot;
                    if !target.borrow().initialized {
                        apply_theme(target.borrow().snapshot.settings.hue);
                        target.borrow_mut().initialized = true;
                        let _ = element("orb").set_attribute("data-ready", "true");
                    }
                    if *first.borrow() {
                        *first.borrow_mut() = false;
                        if onboard {
                            open_panel(&target);
                        }
                    }
                    if let Some(id) = memory_check {
                        let companion = target.clone();
                        let generation = companion.borrow().generation;
                        {
                            let mut v = companion.borrow_mut();
                            v.memory_listening = Some(id.clone());
                            v.consent_heard_voice = false;
                            v.consent_silence = None;
                            v.local_phase = Some("announcing");
                        }
                        spawn_local(async move {
                            let result = async {
                                let question = call("memory_question", json!({"id":id}))
                                    .await?
                                    .as_string()
                                    .ok_or("Memory check expired")?;
                                let audio = call("speak", json!({"text":question})).await?;
                                if companion.borrow().generation != generation {
                                    return Ok(());
                                }
                                play_encoded(companion.clone(), audio, generation).await?;
                                let audio = companion.borrow().audio.clone();
                                if let Some(audio) = audio {
                                    wait_for_audio(&companion, generation, &audio).await?;
                                }
                                if companion.borrow().generation != generation {
                                    return Ok(());
                                }
                                companion.borrow_mut().local_phase = None;
                                voice(companion.clone()).await
                            }
                            .await;
                            if let Err(error) = result {
                                stop_audio(&companion);
                                message(error);
                            }
                        });
                    }
                    if let Some(id) = followup {
                        target.borrow_mut().local_phase = Some("announcing");
                        let companion = target.clone();
                        let generation = companion.borrow().generation;
                        spawn_local(async move {
                            if let Err(e) = call("announce_followup", json!({"id":id})).await {
                                message(e);
                            }
                            if companion.borrow().generation == generation {
                                companion.borrow_mut().local_phase = None;
                            }
                        });
                    }
                    if let Some(id) = usage_notice {
                        target.borrow_mut().local_phase = Some("announcing");
                        let companion = target.clone();
                        let generation = companion.borrow().generation;
                        spawn_local(async move {
                            if let Err(error) = call("announce_usage", json!({"id":id})).await {
                                message(error);
                                companion.borrow_mut().quota_retry_at = js_sys::Date::now() + 5000.;
                            }
                            if companion.borrow().generation == generation {
                                companion.borrow_mut().local_phase = None;
                            }
                        });
                    }
                    if announce {
                        let version = target.borrow().snapshot.updates.version.clone();
                        {
                            let mut v = target.borrow_mut();
                            v.update_announced = version;
                            v.local_phase = Some("announcing");
                        }
                        let companion = target.clone();
                        spawn_local(async move {
                            let result = async {
                                let version = call("announce_update", json!({}))
                                    .await?
                                    .as_string()
                                    .ok_or("Update announcement returned no version")?;
                                {
                                    let mut v = companion.borrow_mut();
                                    v.local_phase = None;
                                    v.update_listening = Some(version);
                                    v.consent_heard_voice = false;
                                    v.consent_silence = None;
                                }
                                voice(companion.clone()).await
                            }
                            .await;
                            if let Err(e) = result {
                                companion.borrow_mut().local_phase = None;
                                companion.borrow_mut().update_listening = None;
                                message(e);
                            }
                        });
                    }
                    let stream_error = {
                        let mut v = target.borrow_mut();
                        if v.voice_pending && v.snapshot.speech.turn == v.reply_turn {
                            let speech = v.snapshot.speech.clone();
                            if speech.failed {
                                Some("The response was interrupted before it finished".to_string())
                            } else {
                                v.speech_queue
                                    .update(&speech.text, speech.done)
                                    .err()
                                    .map(str::to_owned)
                            }
                        } else {
                            None
                        }
                    };
                    if let Some(error) = stream_error {
                        stop_audio(&target);
                        message(error);
                    } else if speak {
                        let generation = {
                            let mut v = target.borrow_mut();
                            v.speech_worker = true;
                            v.local_phase = Some("synthesizing");
                            v.generation
                        };
                        let voice_target = target.clone();
                        spawn_local(async move {
                            let result = stream_speech(voice_target.clone(), generation).await;
                            if voice_target.borrow().generation == generation {
                                if let Err(error) = result {
                                    stop_audio(&voice_target);
                                    message(error);
                                    open_panel(&voice_target);
                                } else {
                                    let mut v = voice_target.borrow_mut();
                                    v.voice_pending = false;
                                    v.speech_worker = false;
                                    v.request_id = None;
                                    v.local_phase = None;
                                }
                            }
                        });
                    }
                }
                Err(e) => message(e),
            }
            target.borrow_mut().polling = false;
        });
    });
    window().set_interval_with_callback_and_timeout_and_arguments_0(
        tick.as_ref().unchecked_ref(),
        100,
    )?;
    tick.forget();
    animate(canvas, view)?;
    Ok(())
}
fn animate(canvas: HtmlCanvasElement, view: Shared) -> Result<(), JsValue> {
    let reduced_query = window().match_media("(prefers-reduced-motion: reduce)")?;
    let context: Context = canvas
        .get_context("2d")?
        .ok_or("Canvas 2D unavailable")?
        .dyn_into()?;
    type Frame = Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>>;
    let callback: Frame = Rc::new(RefCell::new(None));
    let next = callback.clone();
    *callback.borrow_mut() = Some(Closure::new(move |now: f64| {
        let reduced = reduced_query.as_ref().is_some_and(|m| m.matches());
        let mut v = view.borrow_mut();
        if v.initialized
            && v.snapshot.window_visible
            && now - v.last_frame >= if reduced { 100. } else { 33. }
        {
            let frame_seconds = (now - v.last_frame) / 1000.;
            v.last_frame = now;
            let w = canvas.client_width() as f64;
            let h = canvas.client_height() as f64;
            let dpr = window().device_pixel_ratio().min(2.);
            let width = (w * dpr) as u32;
            let height = (h * dpr) as u32;
            if canvas.width() != width || canvas.height() != height {
                canvas.set_width(width);
                canvas.set_height(height);
            }
            let _ = context.set_transform(dpr, 0., 0., dpr, 0., 0.);
            context.clear_rect(0., 0., w, h);
            let playing = v
                .audio
                .as_ref()
                .is_some_and(|audio| !audio.paused() && !audio.ended());
            let mode = visual_state::Mode::resolve(
                v.local_phase
                    .unwrap_or(if v.snapshot.wake["phase"] == "capturing" {
                        "listening"
                    } else {
                        v.snapshot.phase.as_str()
                    }),
                playing,
                v.voice_pending,
            );
            let mut level = v.meter.sample(mode == visual_state::Mode::Listening);
            if v.snapshot.wake["phase"] == "capturing" && v.recording.is_none() {
                level.rms = v.snapshot.wake["rms"].as_f64().unwrap_or(0.);
            }
            if (v.update_listening.is_none() && v.memory_listening.is_none())
                && v.recording.is_some()
                && let Some(capture) = &mut v.speech_end
            {
                match capture.observe(now, level.rms, level.metered) {
                    interaction::CaptureDecision::Continue => {}
                    decision => {
                        v.discard_capture = decision == interaction::CaptureDecision::Discard;
                        if let Some(recorder) = &v.recording {
                            let _ = recorder.stop();
                        }
                    }
                }
            }
            if (v.update_listening.is_some() || v.memory_listening.is_some())
                && v.recording.is_some()
            {
                if level.rms > 0.025 {
                    v.consent_heard_voice = true;
                    v.consent_silence = None;
                } else if v.consent_heard_voice {
                    let since = *v.consent_silence.get_or_insert(now);
                    if now - since > 1400.
                        && let Some(recorder) = &v.recording
                    {
                        let _ = recorder.stop();
                    }
                }
            }
            let energy = (level.rms * 4.5).min(1.);
            let speech_target = if matches!(
                mode,
                visual_state::Mode::Speaking | visual_state::Mode::Listening
            ) {
                if level.metered { energy } else { 0.25 }
            } else {
                0.
            };
            v.speech_energy =
                visual_state::smooth_speech(v.speech_energy, speech_target, frame_seconds);
            let target = match mode {
                visual_state::Mode::Thinking => 0.65,
                visual_state::Mode::Speaking => 0.22 + v.speech_energy * 0.78,
                visual_state::Mode::Listening => 0.22 + v.speech_energy * 0.78,
                _ => 0.08,
            };
            v.amplitude += (target - v.amplitude) * 0.22;
            let hue = if mode == visual_state::Mode::Error {
                12.
            } else {
                v.preview_hue.unwrap_or(v.snapshot.settings.hue) as f64
            };
            let time = if reduced { 0. } else { now / 1000. };
            let scale = match mode {
                visual_state::Mode::Speaking => {
                    1. + v.speech_energy * if reduced { 0.01 } else { 0.07 }
                }
                visual_state::Mode::Listening => {
                    1. + v.speech_energy * if reduced { 0.01 } else { 0.07 }
                }
                visual_state::Mode::Thinking => {
                    1. + if reduced {
                        0.
                    } else {
                        0.045 * (time * 3.).sin()
                    }
                }
                _ => 1.,
            };
            let radius = w.min(h) * 0.17;
            draw(
                &context,
                w / 2.,
                h / 2.,
                radius * scale,
                time,
                hue,
                v.amplitude,
            );
            draw_activity(
                &context,
                ActivityFrame {
                    center: (w / 2., h / 2.),
                    radius,
                    time,
                    hue,
                    mode,
                    reduced,
                    speech_energy: v.speech_energy,
                },
            );
            if v.keyboard_navigation
                && document()
                    .active_element()
                    .is_some_and(|e| e == canvas.clone().into())
            {
                context.save();
                context.set_line_width(1.5);
                context.set_stroke_style_str("rgba(220,245,255,.7)");
                context.begin_path();
                let _ = context.arc(w / 2., h / 2., radius * 1.24, 0., TAU);
                context.stroke();
                context.restore();
            }

            if matches!(
                v.snapshot.quota["level"].as_str(),
                Some("warning" | "exhausted")
            ) {
                context.save();
                context.set_stroke_style_str(if v.snapshot.quota["level"] == "exhausted" {
                    "#f08c7a"
                } else {
                    "#edb347"
                });
                context.set_line_width(2.5);
                context.begin_path();
                let _ = context.arc(w / 2., h / 2., radius * 1.8, 0., TAU);
                context.stroke();
                context.restore();
            }
            if v.snapshot.initiative["available"].as_u64().unwrap_or(0) > 0 {
                context.save();
                context.set_fill_style_str(&format!("hsl({hue},90%,75%)"));
                context.begin_path();
                let _ = context.arc(w / 2. + radius * 1.55, h / 2. - radius * 1.55, 5., 0., TAU);
                context.fill();
                context.restore();
            }
            if v.snapshot.settings.microphone_muted {
                context.save();
                context.set_stroke_style_str("#a7aeb8");
                context.set_line_width(2.5);
                context.begin_path();
                context.move_to(w / 2. - 7., h / 2. + radius * 1.8);
                context.line_to(w / 2. + 7., h / 2. + radius * 1.8);
                context.stroke();
                context.restore();
            } else if v.snapshot.wake["phase"] == "listening" {
                context.save();
                context.set_fill_style_str("#c7f9e1");
                context.begin_path();
                let _ = context.arc(w / 2., h / 2. + radius * 1.8, 3., 0., TAU);
                context.fill();
                context.restore();
            }
            if mode == visual_state::Mode::Speaking {
                v.peak_speaking_rms = v.peak_speaking_rms.max(level.rms);
            }
            v.frames += 1;
            if mode != v.last_mode
                || v.frames == 1
                || v.last_wake_enabled != v.snapshot.settings.wake_enabled
                || v.last_wake_status != wake_hint(&v.snapshot)
            {
                v.last_wake_status = wake_hint(&v.snapshot).to_owned();
                v.last_wake_enabled = v.snapshot.settings.wake_enabled;
                if v.snapshot.settings.wake_enabled {
                    let _ = canvas.remove_attribute("aria-pressed");
                } else {
                    let _ = canvas.set_attribute(
                        "aria-pressed",
                        if mode == visual_state::Mode::Listening {
                            "true"
                        } else {
                            "false"
                        },
                    );
                }
                let _ = canvas.set_attribute(
                    "aria-busy",
                    if mode == visual_state::Mode::Thinking {
                        "true"
                    } else {
                        "false"
                    },
                );
                element("orb-status").set_text_content(Some(match mode {
                    visual_state::Mode::Idle if v.snapshot.settings.wake_enabled => {
                        wake_hint(&v.snapshot)
                    }
                    visual_state::Mode::Idle => "Ready. Press Enter or Space to speak.",
                    visual_state::Mode::Listening => {
                        "Listening. Speak, then pause to send. Escape cancels."
                    }
                    visual_state::Mode::Thinking => "Thinking.",
                    visual_state::Mode::Speaking => "Speaking. Press Escape to stop.",
                    visual_state::Mode::Error => {
                        "Unable to complete the request. Open Settings for details."
                    }
                }));
            }
            if v.last_mode == visual_state::Mode::Speaking
                && mode != visual_state::Mode::Speaking
                && v.audio.as_ref().is_some_and(|audio| audio.ended())
            {
                element("voice-info").set_text_content(Some("Playback finished"));
            }
            if v.frames == 1
                || v.last_mode != mode
                || (matches!(
                    mode,
                    visual_state::Mode::Thinking
                        | visual_state::Mode::Speaking
                        | visual_state::Mode::Listening
                ) && now - v.last_report >= 1000.)
            {
                v.last_mode = mode;
                v.last_report = now;
                let report = json!({"mode":mode.name(),"reduced":reduced,"rms":level.rms,"metered":level.metered,"frames":v.frames,"peak_speaking_rms":v.peak_speaking_rms,"capture_feedback_events":v.capture_feedback_events});
                spawn_local(async move {
                    let _ = call("report_visual", json!({"report":report})).await;
                });
            }
        }
        drop(v);
        if let Some(f) = next.borrow().as_ref() {
            let _ = window().request_animation_frame(f.as_ref().unchecked_ref());
        }
    }));
    window().request_animation_frame(
        callback
            .borrow()
            .as_ref()
            .ok_or("Animation callback missing")?
            .as_ref()
            .unchecked_ref(),
    )?;
    Ok(())
}
fn draw(c: &Context, x: f64, y: f64, r: f64, t: f64, hue: f64, amp: f64) {
    let color = |alpha: f64| format!("hsla({hue},85%,70%,{alpha})");
    if let Ok(g) = c.create_radial_gradient(x, y, r * 0.1, x, y, r * 2.9) {
        let _ = g.add_color_stop(0., &color(0.18));
        let _ = g.add_color_stop(1., &color(0.));
        c.set_fill_style_canvas_gradient(&g);
        c.fill_rect(x - r * 3., y - r * 3., r * 6., r * 6.);
    }
    for i in 0..70 {
        let a = i as f64 * 2.39996 + t * 0.025;
        let radius = r * (1.3 + ((i * 37 % 67) as f64 / 67.) * 1.15);
        let sparkle = 0.2 + ((t * 0.6 + i as f64).sin() + 1.) * 0.22;
        c.set_fill_style_str(&color(sparkle));
        c.begin_path();
        let _ = c.arc(
            x + a.cos() * radius,
            y + a.sin() * radius,
            if i % 7 == 0 { 1.2 } else { 0.65 },
            0.,
            TAU,
        );
        c.fill();
    }
    c.set_stroke_style_str(&color(0.45));
    c.set_line_width(1.);
    c.begin_path();
    for i in 0..=180 {
        let a = i as f64 / 180. * TAU;
        let rr =
            r * (1.85 + 0.035 * (a * 3. + t * 0.7).sin() + amp * 0.06 * (a * 2. - t * 0.8).sin());
        let (px, py) = (x + a.cos() * rr, y + a.sin() * rr);
        if i == 0 {
            c.move_to(px, py)
        } else {
            c.line_to(px, py)
        }
    }
    c.close_path();
    c.stroke();
    c.save();
    let _ = c.translate(x, y);
    c.begin_path();
    for i in 0..=120 {
        let a = i as f64 / 120. * TAU;
        let rr =
            r * (1. + amp * (0.08 * (a * 2. + t * 0.7).sin() + 0.05 * (a * 3. - t * 0.8).cos()));
        let (px, py) = (a.cos() * rr, a.sin() * rr);
        if i == 0 {
            c.move_to(px, py)
        } else {
            c.line_to(px, py)
        }
    }
    c.close_path();
    if let Ok(g) = c.create_radial_gradient(-r * 0.28, -r * 0.35, r * 0.1, 0., 0., r * 1.15) {
        let _ = g.add_color_stop(0., "rgba(255,255,255,.94)");
        let _ = g.add_color_stop(0.4, &color(0.7));
        let _ = g.add_color_stop(1., &format!("hsla({},80%,30%,.15)", hue + 40.));
        c.set_fill_style_canvas_gradient(&g);
        c.set_shadow_blur(30. + amp * 40.);
        c.set_shadow_color(&color(0.65));
        c.fill();
    }
    c.set_shadow_blur(0.);
    c.begin_path();
    let _ = c.arc(0., 0., r * 0.65, 0., TAU);
    if let Ok(g) = c.create_radial_gradient(-r * 0.1, -r * 0.1, r * 0.05, 0., 0., r * 0.8) {
        let _ = g.add_color_stop(0., &color(0.2));
        let _ = g.add_color_stop(1., "rgba(0,0,0,.68)");
        c.set_fill_style_canvas_gradient(&g);
        c.fill();
    }
    c.begin_path();
    let _ = c.arc(-r * 0.18, -r * 0.18, r * 0.18, 0., TAU);
    c.set_fill_style_str("rgba(255,255,255,.22)");
    c.fill();
    c.restore();
}

#[cfg(feature = "debug-ui")]
fn mount_debug(view: &Shared) {
    let node = document().create_element("div").expect("debug container");
    node.set_inner_html(r#"<dialog id="composer"><form id="message"><label for="text">Debug spoken request</label><textarea id="text" rows="4"></textarea><p id="reply" role="status"></p><div class="actions"><button type="button" id="dismiss">Close</button><button type="button" id="stop">Stop</button><button type="submit">Send</button></div></form></dialog>"#);
    document()
        .body()
        .expect("debug body")
        .append_child(&node)
        .expect("debug panel");
    on(element("dismiss").as_ref(), "click", |_| {
        dialog("composer").close();
        let _ = resize(360., 360.);
    });
    on(element("composer").as_ref(), "cancel", |_| {
        let _ = resize(360., 360.);
    });
    let target = view.clone();
    on(element("stop").as_ref(), "click", move |_| {
        stop_audio(&target);
    });
    let target = view.clone();
    on(element("message").as_ref(), "submit", move |e| {
        e.prevent_default();
        stop_audio(&target);
        let _ = target.borrow_mut().meter.unlock();
        let text = element("text")
            .unchecked_into::<web_sys::HtmlTextAreaElement>()
            .value();
        let view = target.clone();
        let generation = view.borrow().generation;
        dialog("composer").close();
        let _ = resize(360., 360.);
        spawn_local(async move {
            if let Err(error) = submit_transcript(view.clone(), text, generation).await {
                if view.borrow().generation == generation {
                    message(error);
                    open_panel(&view);
                }
            } else {
                element("text")
                    .unchecked_into::<web_sys::HtmlTextAreaElement>()
                    .set_value("");
            }
        });
    });
}

async fn review_followups() -> Result<(), String> {
    let value = call("followups", json!({})).await?;
    let entries: Value = serde_wasm_bindgen::from_value(value).map_err(|e| e.to_string())?;
    let container = element("followups");
    container.set_text_content(None);
    for e in entries.as_array().into_iter().flatten() {
        let card = document().create_element("article").map_err(js_error)?;
        let p = document().create_element("p").map_err(js_error)?;
        p.set_text_content(e["message"].as_str());
        card.append_child(&p).map_err(js_error)?;
        let detail = document().create_element("p").map_err(js_error)?;
        detail.set_class_name("hint");
        let due = e["due"].as_f64().unwrap_or(0.);
        let date = js_sys::Date::new(&JsValue::from_f64(due * 1000.));
        detail.set_text_content(Some(&format!(
            "{} · {} · {}",
            date.to_locale_string("en-GB", &JsValue::UNDEFINED),
            if e["permitted"] == true {
                "Spoken permission granted"
            } else {
                "Silent until allowed"
            },
            if e["delivered"] == true {
                "Announced"
            } else {
                "Pending"
            }
        )));
        card.append_child(&detail).map_err(js_error)?;
        let evidence = document().create_element("p").map_err(js_error)?;
        evidence.set_class_name("hint");
        evidence.set_text_content(e["evidence"].as_str());
        card.append_child(&evidence).map_err(js_error)?;
        for (label, action) in [
            ("Allow spoken check-in", "allow"),
            ("Snooze one hour", "snooze"),
            ("Dismiss", "dismiss"),
        ] {
            if action == "allow" && e["permitted"] == true {
                continue;
            }
            let b = document().create_element("button").map_err(js_error)?;
            b.set_attribute("type", "button").map_err(js_error)?;
            b.set_attribute(
                "data-followup-id",
                e["id"].as_str().ok_or("Follow-up ID missing")?,
            )
            .map_err(js_error)?;
            b.set_attribute("data-action", action).map_err(js_error)?;
            b.set_text_content(Some(label));
            card.append_child(&b).map_err(js_error)?;
        }
        container.append_child(&card).map_err(js_error)?;
    }
    Ok(())
}
async fn review_memories() -> Result<(), String> {
    let value = call("memories", json!({})).await?;
    let records: Value = serde_wasm_bindgen::from_value(value).map_err(|e| e.to_string())?;
    let container = element("notes");
    container.set_text_content(None);
    if let Some(records) = records.as_array() {
        if records.is_empty() {
            container.set_text_content(Some("No saved memories yet."));
        }
        for note in records {
            let card = document().create_element("article").map_err(js_error)?;
            let text = document().create_element("p").map_err(js_error)?;
            text.set_text_content(note["content"].as_str());
            card.append_child(&text).map_err(js_error)?;
            let origin = document().create_element("p").map_err(js_error)?;
            origin.set_class_name("hint");
            let lineage = note["lineage"].as_array().map_or(0, Vec::len);
            origin.set_text_content(Some(&format!(
                "{}{}",
                note["metadata"]["origin"]
                    .as_str()
                    .unwrap_or("Saved memory"),
                if lineage > 0 {
                    format!(" · based on {lineage} saved notes")
                } else {
                    String::new()
                }
            )));
            card.append_child(&origin).map_err(js_error)?;
            if let Some(evidence) = note["metadata"]["evidence"].as_str() {
                let details = document().create_element("details").map_err(js_error)?;
                let summary = document().create_element("summary").map_err(js_error)?;
                summary.set_text_content(Some("Why this was remembered"));
                details.append_child(&summary).map_err(js_error)?;
                let quote = document().create_element("p").map_err(js_error)?;
                quote.set_text_content(Some(evidence));
                details.append_child(&quote).map_err(js_error)?;
                card.append_child(&details).map_err(js_error)?;
            }
            let id = note["id"].as_str().ok_or("Memory ID missing")?;
            let state = document().create_element("p").map_err(js_error)?;
            state.set_class_name("hint");
            let confidence = note["metadata"]["confidence"]
                .as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .filter(|n| n.is_finite() && (0. ..=1.).contains(n))
                .map(|n| format!(" · AI confidence estimate: {:.0}%", n * 100.))
                .unwrap_or_default();
            state.set_text_content(Some(&format!(
                "{}{} · updated {}",
                match note["metadata"]["learning_state"].as_str().unwrap_or("") {
                    "provisional" => "Needs confirmation",
                    "corroborated" => "Repeated by you",
                    "corrected" | "explicit correction" => "Corrected",
                    "confirmed" | "confirmed by user" => "Confirmed by you",
                    _ if note["metadata"]["origin"] == "semantic distillation" => "Summary",
                    _ => "Saved by you",
                },
                confidence,
                js_sys::Date::new(&JsValue::from_f64(
                    note["updated_at_unix_ms"].as_f64().unwrap_or(0.)
                ))
                .to_locale_string("en-GB", &JsValue::UNDEFINED)
            )));
            card.append_child(&state).map_err(js_error)?;
            let editor = document()
                .create_element("textarea")
                .map_err(js_error)?
                .dyn_into::<web_sys::HtmlTextAreaElement>()
                .map_err(|_| "Memory editor unavailable")?;
            editor.set_id(&format!("edit-{id}"));
            editor.set_class_name("note-editor");
            editor.set_value(note["content"].as_str().unwrap_or(""));
            editor
                .set_attribute("aria-label", "Correct this memory")
                .map_err(js_error)?;
            editor.set_attribute("hidden", "").map_err(js_error)?;
            card.append_child(&editor).map_err(js_error)?;
            for (label, action) in [("Edit", "edit"), ("Confirm / save correction", "save")] {
                let b = document().create_element("button").map_err(js_error)?;
                b.set_attribute("type", "button").map_err(js_error)?;
                b.set_attribute("data-memory-id", id).map_err(js_error)?;
                b.set_attribute("data-action", action).map_err(js_error)?;
                b.set_text_content(Some(label));
                card.append_child(&b).map_err(js_error)?;
            }
            let button = document().create_element("button").map_err(js_error)?;
            button.set_attribute("type", "button").map_err(js_error)?;
            button.set_text_content(Some("Forget this note"));
            let id = note["id"].as_str().ok_or("Memory ID missing")?;
            button
                .set_attribute("data-memory-id", id)
                .map_err(js_error)?;
            card.append_child(&button).map_err(js_error)?;
            container.append_child(&card).map_err(js_error)?;
        }
    }
    Ok(())
}

struct ActivityFrame {
    center: (f64, f64),
    radius: f64,
    time: f64,
    hue: f64,
    mode: visual_state::Mode,
    reduced: bool,
    speech_energy: f64,
}
fn draw_activity(c: &Context, frame: ActivityFrame) {
    let ActivityFrame {
        center: (x, y),
        radius: r,
        time: t,
        hue,
        mode,
        reduced,
        speech_energy,
    } = frame;
    use visual_state::Mode;
    c.save();
    c.set_line_cap("round");
    if mode == Mode::Thinking {
        for i in 0..28 {
            let a = t * 2.9 + i as f64 * 0.075;
            let alpha = (i as f64 / 27.).powf(1.5) * 0.9 + 0.05;
            c.set_line_width(3. + i as f64 / 28.);
            c.set_stroke_style_str(&format!("hsla({hue},95%,47%,{alpha})"));
            c.begin_path();
            let _ = c.arc(x, y, r * 2.02, a, a + 0.06);
            c.stroke();
        }
    } else if matches!(mode, Mode::Speaking | Mode::Listening) {
        let layers = if mode == Mode::Listening { 1 } else { 2 };
        for layer in 0..layers {
            c.set_line_width(if layer == 0 { 2.2 } else { 1.2 });
            c.set_shadow_color(&format!("hsla({hue},85%,65%,.4)"));
            c.set_shadow_blur(if reduced { 0. } else { 10. });
            c.set_stroke_style_str(&format!(
                "hsla({hue},85%,65%,{})",
                0.30 + speech_energy * 0.25
            ));
            c.set_fill_style_str(&format!("hsla({hue},75%,60%,.025)"));
            c.begin_path();
            for i in 0..=180 {
                let angle = i as f64 / 180. * TAU;
                let radius =
                    r * visual_state::speaking_shell(angle, t, speech_energy, layer, reduced);
                let (px, py) = (x + angle.cos() * radius, y + angle.sin() * radius);
                if i == 0 {
                    c.move_to(px, py);
                } else {
                    c.line_to(px, py);
                }
            }
            c.close_path();
            c.fill();
            c.stroke();
        }
    }
    c.restore();
}
