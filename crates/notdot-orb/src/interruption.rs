//! Interrupt a spoken reply without repeating the wake phrase.
//!
//! The webview captures the microphone with echo cancellation, which uses
//! notdot's own playback as its reference, and streams 16 kHz audio to the
//! on-device speech detector. Playback holds on speech onset; the utterance
//! it decides whether the reply stops, is replaced or carries on.
use crate::{Shared, call, message, stop_audio};
use serde_json::json;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{JsFuture, spawn_local};

#[wasm_bindgen(inline_js = r#"
let active = null, guarded = false;
const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);
export async function interruptSupport() {
  try {
    const supported = navigator.mediaDevices?.getSupportedConstraints?.() || {};
    if (!supported.echoCancellation) return 'unsupported: no echoCancellation constraint';
    if (typeof AudioWorkletNode === 'undefined') return 'unsupported: no AudioWorklet';
    let expired = false;
    const request = navigator.mediaDevices.getUserMedia({ audio: { echoCancellation: true } });
    request.then(stream => { if (expired) for (const track of stream.getTracks()) track.stop(); }, () => {});
    let stream;
    try { stream = await bounded('microphone check', request, 5000); }
    catch (error) { expired = true; throw error; }
    try {
      const settings = stream.getAudioTracks()[0]?.getSettings?.() || {};
      return settings.echoCancellation === true ? 'ok' : 'unsupported: track reports echoCancellation=' + settings.echoCancellation;
    } finally {
      for (const track of stream.getTracks()) track.stop();
    }
  } catch (error) {
    return 'unsupported: ' + (error && error.name || error);
  }
}
export function interruptGuard(value) { guarded = !!value; }
const bounded = (label, promise, ms) => Promise.race([
  promise,
  new Promise((_, reject) => setTimeout(() => reject(new Error(label + ' timed out')), ms)),
]);
// Release one listening attempt. Never touches a newer attempt.
async function teardown(state) {
  state.stopped = true;
  if (active === state) active = null;
  if (state.node) { state.node.port.onmessage = null; state.node.disconnect(); }
  if (state.stream) for (const t of state.stream.getTracks()) t.stop();
  if (state.context && state.context.state !== 'closed') await bounded('audio close', state.context.close(), 2000).catch(() => {});
}
export async function interruptStart(session, onDetect) {
  const previous = active;
  const state = { session, stream: null, context: null, node: null, busy: 0, stopped: false };
  active = state;
  if (previous) await teardown(previous);
  if (state.stopped || active !== state) return false;
  try {
    const request = navigator.mediaDevices.getUserMedia({ audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true } });
    // A stream that arrives after this attempt ended is released at once.
    request.then(stream => { if (state.stopped) for (const t of stream.getTracks()) t.stop(); }, () => {});
    state.stream = await bounded('microphone', request, 5000);
    if (state.stopped || state.stream.getAudioTracks()[0]?.getSettings?.().echoCancellation !== true) {
      await teardown(state);
      return false;
    }
    state.context = new AudioContext();
    await bounded('audio start', state.context.resume(), 2000);
    await bounded('worklet', state.context.audioWorklet.addModule('interrupt-worklet.js'), 3000);
    if (state.stopped) { await teardown(state); return false; }
    const source = state.context.createMediaStreamSource(state.stream);
    state.node = new AudioWorkletNode(state.context, 'interrupt-16k');
    source.connect(state.node);
  } catch (error) {
    await teardown(state);
    throw error;
  }
  state.node.port.onmessage = ({ data }) => {
    // One ordered chunk in flight; overload drops audio instead of queuing it.
    if (state.stopped || state.busy >= 1) return;
    state.busy++;
    invoke('interrupt_audio', { session, samples: Array.from(data), guarded })
      .then(heard => { if (heard === 'complete') { teardown(state); } else if (heard === true && !state.stopped && !state.detected) { state.detected = true; onDetect(); } })
      .catch(() => { teardown(state); })
      .finally(() => { state.busy--; });
  };
  return true;
}
export async function interruptStop() {
  if (active) await teardown(active);
}
export function interruptActive() { return active !== null; }
"#)]
extern "C" {
    #[wasm_bindgen(js_name = interruptSupport)]
    fn interrupt_support() -> js_sys::Promise;
    #[wasm_bindgen(js_name = interruptGuard)]
    fn interrupt_guard(value: bool);
    #[wasm_bindgen(js_name = interruptStart)]
    fn interrupt_start(session: &str, on_detect: &js_sys::Function) -> js_sys::Promise;
    #[wasm_bindgen(js_name = interruptStop)]
    fn interrupt_stop() -> js_sys::Promise;
    #[wasm_bindgen(js_name = interruptActive)]
    fn interrupt_active() -> bool;
}

/// Check this webview once per session, then report it natively.
pub async fn check_support() -> Result<bool, String> {
    let result = JsFuture::from(interrupt_support())
        .await
        .map_err(crate::js_error)?
        .as_string()
        .unwrap_or_default();
    let ok = result == "ok";
    call(
        "interrupt_support",
        json!({"echoCancellation": ok, "detail": result}),
    )
    .await?;
    Ok(ok)
}

/// Words of the wake name. A sentence that says them is "guarded": notdot
/// treats a detection then as hearing itself.
fn says_name(text: &str, name: &str) -> bool {
    let words = |t: &str| {
        t.split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_lowercase)
            .collect::<Vec<_>>()
    };
    let name = words(name);
    let text = words(text);
    !name.is_empty() && text.windows(name.len()).any(|w| w == name.as_slice())
}

/// Whether interruption listening should run on this tick.
pub fn wanted(view: &Shared) -> bool {
    let v = view.borrow();
    let s = &v.snapshot;
    v.interrupt_supported == Some(true)
        && s.settings.interrupt_enabled
        && !s.settings.microphone_muted
        && s.window_visible
        && s.settings.onboarding_complete
        && v.interrupt_hold.is_none()
        && v.confirmation_listening.is_none()
        && v.update_listening.is_none()
        && !crate::dialog("panel").open()
        && v.audio.as_ref().is_some_and(|a| !a.paused() && !a.ended())
}

/// Start, keep or stop listening to match the current state. Called each tick.
pub fn sync(view: &Shared) {
    let name = view.borrow().snapshot.settings.name.clone();
    interrupt_guard(says_name(&view.borrow().playing_text, &name));
    let want = wanted(view);
    let running = interrupt_active() || view.borrow().interrupt_starting;
    if want && !running {
        view.borrow_mut().interrupt_starting = true;
        let target = view.clone();
        spawn_local(async move {
            let generation = target.borrow().generation;
            let started = async {
                let turn = target.borrow().snapshot.speech.turn.clone();
                let session = call("interrupt_begin", json!({"turn":turn})).await?;
                let session = session.as_string().ok_or("No session")?;
                if !wanted(&target) {
                    stop_listening().await;
                    return Ok(());
                }
                let detected = std::rc::Rc::downgrade(&target);
                let on_detect = Closure::<dyn FnMut()>::new(move || {
                    if let Some(view) = detected.upgrade() {
                        hold(&view);
                    }
                });
                let ok = JsFuture::from(interrupt_start(
                    &session,
                    on_detect.as_ref().unchecked_ref(),
                ))
                .await
                .map_err(crate::js_error)?;
                if ok.as_bool() != Some(true) {
                    return Err("Echo cancellation was not available".to_string());
                }
                if !wanted(&target) {
                    stop_listening().await;
                    return Ok(());
                }
                target.borrow_mut().interrupt_session = Some(session);
                target.borrow_mut().interrupt_callback = Some(on_detect);
                Ok::<(), String>(())
            }
            .await;
            target.borrow_mut().interrupt_starting = false;
            if let Err(error) = started
                && target.borrow().generation == generation
                && wanted(&target)
            {
                {
                    let mut v = target.borrow_mut();
                    v.interrupt_failures += 1;
                    v.interrupt_supported = Some(false);
                }
                stop_listening().await;
                render_settings(&target);
                message(
                    "Voice interruption couldn't start. Toggle it off and on in Hands-free settings to retry.",
                );
                web_sys::console::warn_1(&JsValue::from_str(&format!(
                    "Interrupt listening: {error}"
                )));
            }
        });
    } else if !want && running && view.borrow().interrupt_hold.is_none() {
        stop(view);
    }
}
pub fn stop(view: &Shared) {
    view.borrow_mut().interrupt_session = None;
    spawn_local(stop_listening());
}
pub async fn stop_listening() {
    let _ = JsFuture::from(interrupt_stop()).await;
    let _ = call("interrupt_end", json!({})).await;
}

/// Per-poll housekeeping: check support once when needed, keep listening in
/// step with playback, and never leave a reply held without a capture.
pub fn tick(view: &Shared, snapshot: &crate::Snapshot) {
    let enabled = snapshot.settings.interrupt_enabled;
    let check = {
        let v = view.borrow();
        enabled
            && v.interrupt_supported.is_none()
            && !v.interrupt_checking
            && v.audio.as_ref().is_some_and(|a| !a.paused() && !a.ended())
    };
    if check {
        view.borrow_mut().interrupt_checking = true;
        let target = view.clone();
        spawn_local(async move {
            let ok = check_support().await.unwrap_or(false);
            let mut v = target.borrow_mut();
            v.interrupt_supported = Some(ok);
            v.interrupt_checking = false;
        });
    }
    let stalled = {
        let v = view.borrow();
        v.interrupt_hold.is_some_and(|since| {
            let age = js_sys::Date::now() - since;
            age > 15_000.
                || (age > 2_500.
                    && v.interrupt_token.is_none()
                    && v.local_phase != Some("transcribing")
                    && v.capture_token.as_deref() != snapshot.wake["capture_token"].as_str()
                    && snapshot.wake["request_id"].is_null())
        })
    };
    if stalled {
        resume(view, Some("I didn't hear a request, so I'll carry on."));
    }
    sync(view);
}
/// The orb's accessible name while interruption listening or a hold is on.
/// Applied after the ordinary labels so it is not overwritten.
pub fn label(view: &Shared, snapshot: &crate::Snapshot) {
    let v = view.borrow();
    let orb = crate::element("orb");
    if v.interrupt_hold.is_some() {
        let _ = orb.set_attribute(
            "aria-label",
            "Listening. Paused my reply. Say your request, or Stop. Escape cancels.",
        );
    } else if v.interrupt_session.is_some() {
        let _ = snapshot;
        let _ = orb.set_attribute(
            "aria-label",
            "Speaking. You can interrupt with Stop or a new request. Escape stops.",
        );
    }
}

/// The wake phrase was heard during a reply: pause playback at once and
/// capture the request after it, exactly like a click.
fn hold(view: &Shared) {
    let Some(session) = view.borrow_mut().interrupt_session.take() else {
        return;
    };
    let token = format!("interrupt-{:.0}", js_sys::Date::now());
    {
        let mut v = view.borrow_mut();
        if let Some(audio) = v.audio.as_ref() {
            let _ = audio.pause();
        }
        v.interrupt_hold = Some(js_sys::Date::now());
        v.interrupt_token = Some(token.clone());
        v.local_phase = Some("listening");
        let _ = v.meter.wake_cue();
    }
    let _ = crate::element("orb").set_attribute(
        "aria-label",
        "Listening. Paused my reply. Say your request, or Stop. Escape cancels.",
    );
    let target = view.clone();
    let generation = target.borrow().generation;
    spawn_local(async move {
        let result = call(
            "interrupt_collect",
            json!({"session":session,"token":token}),
        )
        .await;
        // Keep the same stream until the complete interruption has arrived.
        // Stop capture before any resumed or replacement reply can play.
        stop_listening().await;
        let current = {
            let v = target.borrow();
            v.generation == generation && v.interrupt_token.as_deref() == Some(token.as_str())
        };
        if !current {
            return;
        }
        match result {
            Ok(value) => match serde_wasm_bindgen::from_value::<serde_json::Value>(value) {
                Ok(result) => outcome(target, &result, &token, generation).await,
                Err(_) => resume(
                    &target,
                    Some("I couldn't hear that clearly, so I'll carry on."),
                ),
            },
            Err(_) => resume(
                &target,
                Some("I couldn't hear that clearly, so I'll carry on."),
            ),
        }
    });
}

/// Carry on with the held reply.
pub fn resume(view: &Shared, line: Option<&str>) {
    let audio = {
        let mut v = view.borrow_mut();
        if v.interrupt_hold.take().is_none() {
            return;
        }
        v.interrupt_token = None;
        if let Some(token) = v.capture_token.take() {
            v.ended_capture = Some(token.clone());
            spawn_local(async move {
                let _ = call("cancel_capture", json!({"token":token})).await;
            });
        }
        v.native_manual = false;
        v.local_phase = None;
        v.audio.clone()
    };
    if let Some(line) = line {
        message(line);
    }
    if let Some(audio) = audio
        && !audio.ended()
        && let Ok(promise) = audio.play()
    {
        spawn_local(async move {
            let _ = JsFuture::from(promise).await;
        });
    }
}

/// Handle the recognised request after the wake phrase. `token` and
/// `generation` identify the hold it belongs to; a result for anything else
/// (an older capture, after Escape or a change of person) is ignored.
pub async fn outcome(view: Shared, result: &serde_json::Value, token: &str, generation: u64) {
    let current = |view: &Shared| {
        let v = view.borrow();
        v.generation == generation && v.interrupt_token.as_deref() == Some(token)
    };
    if !current(&view) {
        return;
    }
    match result["interrupt"].as_str() {
        Some("stop") => {
            view.borrow_mut().interrupt_token = None;
            stop_audio(&view);
            message("Stopped.");
        }
        Some("request") => {
            let text = result["text"].as_str().unwrap_or_default().to_owned();
            view.borrow_mut().interrupt_token = None;
            // Cancel the held reply through the normal path: queued sentences,
            // synthesis, the model turn and follow-up timers stop together.
            stop_audio(&view);
            // The request belongs to this cancellation. Escape, a change of
            // person or another request while waiting revokes it.
            let after = view.borrow().generation;
            let still = |view: &Shared| view.borrow().generation == after;
            for _ in 0..50 {
                let busy = call("snapshot", json!({}))
                    .await
                    .ok()
                    .and_then(|v| serde_wasm_bindgen::from_value::<serde_json::Value>(v).ok())
                    .is_none_or(|s| !s["active_turn"].is_null() || s["phase"] == "thinking");
                if !still(&view) {
                    return;
                }
                if !busy {
                    break;
                }
                let _ = crate::speech_delay(100).await;
                if !still(&view) {
                    return;
                }
            }
            if !still(&view) {
                return;
            }
            if let Err(error) = crate::submit_transcript(view.clone(), text, after).await
                && still(&view)
            {
                message(error);
            }
        }
        _ => resume(&view, result["line"].as_str()),
    }
}

/// Explain the optional echo-cancelled interruption control.
pub fn render_settings(view: &Shared) {
    let status = match view.borrow().interrupt_supported {
        Some(false) => {
            "This computer's echo cancellation isn't available, so interrupting stays off."
        }
        Some(true) => {
            "Say Stop, Actually, or a new request while notdot speaks. No wake phrase needed."
        }
        None => "Checks this computer's echo cancellation when enabled.",
    };
    crate::element("interrupt-status").set_text_content(Some(status));
}
pub fn install(view: &Shared) {
    {
        let id = "interrupt-enabled";
        let target = view.clone();
        crate::on(crate::element(id).as_ref(), "change", move |_| {
            // An explicit user toggle is a retry, never a polling loop.
            if crate::input("interrupt-enabled").checked() {
                target.borrow_mut().interrupt_supported = None;
            }
            render_settings(&target);
            if crate::input("interrupt-enabled").checked()
                && target.borrow().interrupt_supported.is_none()
                && !target.borrow().interrupt_checking
            {
                target.borrow_mut().interrupt_checking = true;
                let checked = target.clone();
                spawn_local(async move {
                    let ok = check_support().await.unwrap_or(false);
                    {
                        let mut v = checked.borrow_mut();
                        v.interrupt_supported = Some(ok);
                        v.interrupt_checking = false;
                    }
                    if !ok {
                        crate::input("interrupt-enabled").set_checked(false);
                    }
                    render_settings(&checked);
                });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sentences_that_say_the_name_are_guarded() {
        assert!(super::says_name("Hey Ava, here is the forecast.", "Ava"));
        assert!(super::says_name("I'm notdot.", "notdot"));
        assert!(!super::says_name("Avalanche warnings are in place.", "Ava"));
        assert!(!super::says_name("Anything else?", "Ava"));
    }
}
