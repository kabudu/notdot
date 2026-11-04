//! Trusted voices and private profiles: Tauri commands for notdot's own
//! window. Every command here is a deliberate local action (Actor::LocalDevice)
//! and requires the window to be visible and focused. None is exposed to the
//! model, MCP tools or voice; a recognised voice never reaches these paths.
//!
//! The user flow and permission boundaries are described in docs/settings.md.
mod keychain;
mod service;

pub use service::Speakers;

use crate::AppState;
use notdot_core::profiles::{Actor, ChatAccess, IdentityMode, ProfileId};
use notdot_speakers::enrol::Progress;
use serde_json::Value;
use std::sync::Arc;
use tauri::State;
static LOCAL_OPERATIONS: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn local(window: &tauri::WebviewWindow) -> Result<Actor, String> {
    if !window.is_visible().unwrap_or(false)
        || window.is_minimized().unwrap_or(true)
        || !window.is_focused().unwrap_or(false)
    {
        return Err("Open notdot on this computer to change voices and sharing".into());
    }
    Ok(Actor::LocalDevice)
}
fn id(text: &str) -> Result<ProfileId, String> {
    ProfileId::parse(text)
}

#[tauri::command]
pub fn speaker_summary(state: State<'_, Arc<AppState>>) -> Value {
    let mut summary = state.speakers.summary();
    summary["active_profile"] = serde_json::json!(state.context().id);
    summary
}

#[tauri::command]
pub fn speaker_add_person(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    name: String,
) -> Result<String, String> {
    let _operation = LOCAL_OPERATIONS
        .lock()
        .map_err(|_| "Voice settings are busy")?;
    let actor = local(&window)?;
    if !state.speakers.roster()?.owner_enrolled() {
        return Err("Teach your own voice first so everyone's context stays separate.".into());
    }
    // A household starts with private, recognised contexts. Returning to the
    // shared owner fallback is a separate explicit local mode change.
    state
        .speakers
        .set_mode(&actor, IdentityMode::TrustedGroup)?;
    Ok(state.speakers.add_member(&actor, &name)?.to_string())
}

#[tauri::command]
pub fn speaker_rename_person(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    profile: String,
    name: String,
) -> Result<(), String> {
    let _operation = LOCAL_OPERATIONS
        .lock()
        .map_err(|_| "Voice settings are busy")?;
    state
        .speakers
        .rename(&local(&window)?, &id(&profile)?, &name)
}

/// Removes the person, their voice, shares and private memories. The UI must
/// show what is removed and require a confirming click before calling this.
/// Returns their companion chat ID (if any) for root to archive in Codex.
#[tauri::command]
pub async fn speaker_remove_person(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    profile: String,
) -> Result<Option<String>, String> {
    let actor = local(&window)?;
    let profile = id(&profile)?;
    let gate = state.send_gate.lock().await;
    if state.snapshot.lock().await.active_turn.is_some() {
        return Err("Wait for the current reply before removing a person".into());
    }
    let thread = {
        let _operation = LOCAL_OPERATIONS
            .lock()
            .map_err(|_| "Voice settings are busy")?;
        state.speakers.remove_member(&actor, &profile)?
    };
    if state.context().id == profile {
        state
            .generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        *state.active_context.write().expect("active context") = state.profiles.owner();
        *state.identity.lock().expect("voice identity") = None;
        let mut snapshot = state.snapshot.lock().await;
        snapshot.reply.clear();
        snapshot.speech = Default::default();
        snapshot.phase = "idle".into();
        snapshot.error = None;
    }
    state.profiles.remove_member(&profile).await?;
    drop(gate);
    Ok(thread)
}

#[tauri::command]
pub fn speaker_enrol_begin(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    profile: String,
) -> Result<Progress, String> {
    let _operation = LOCAL_OPERATIONS
        .lock()
        .map_err(|_| "Voice settings are busy")?;
    state.speakers.enrol_begin(&local(&window)?, &id(&profile)?)
}

/// Register `token` for the current phrase, then start the capture with the
/// existing `start_capture(token, limit)`. Root's capture path diverts that
/// token's PCM to the enrolment buffer and emits `speaker-enrol` with the
/// resulting [`Progress`] when the capture ends.
#[tauri::command]
pub fn speaker_enrol_capture(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    token: String,
) -> Result<(), String> {
    local(&window)?;
    state.speakers.enrol_capture(&token)
}

#[tauri::command]
pub fn speaker_enrol_cancel(state: State<'_, Arc<AppState>>) {
    state.speakers.enrol_cancel();
}

#[tauri::command]
pub fn speaker_enrol_save(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let _operation = LOCAL_OPERATIONS
        .lock()
        .map_err(|_| "Voice settings are busy")?;
    state.speakers.enrol_save(&local(&window)?)
}

#[tauri::command]
pub fn speaker_forget_voice(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    profile: String,
) -> Result<(), String> {
    let _operation = LOCAL_OPERATIONS
        .lock()
        .map_err(|_| "Voice settings are busy")?;
    state
        .speakers
        .forget_voice(&local(&window)?, &id(&profile)?)
}

#[tauri::command]
pub async fn speaker_forget_all_voices(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    {
        let _operation = LOCAL_OPERATIONS
            .lock()
            .map_err(|_| "Voice settings are busy")?;
        state.speakers.forget_all_voices(&local(&window)?)?;
    }
    speaker_use_owner(window, state).await
}

/// `mode`: "open" | "owner_only" | "trusted_group".
#[tauri::command]
pub fn speaker_set_mode(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    mode: IdentityMode,
) -> Result<(), String> {
    let _operation = LOCAL_OPERATIONS
        .lock()
        .map_err(|_| "Voice settings are busy")?;
    state.speakers.set_mode(&local(&window)?, mode)
}

/// `access`: "read" (first share) or "read_write" (second, separate step).
#[tauri::command]
pub fn speaker_share_chat(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    chat: String,
    profile: String,
    access: ChatAccess,
) -> Result<(), String> {
    let _operation = LOCAL_OPERATIONS
        .lock()
        .map_err(|_| "Voice settings are busy")?;
    state
        .speakers
        .grant_chat(&local(&window)?, &chat, &id(&profile)?, access)
}

#[tauri::command]
pub fn speaker_unshare_chat(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    chat: String,
    profile: String,
) -> Result<(), String> {
    let _operation = LOCAL_OPERATIONS
        .lock()
        .map_err(|_| "Voice settings are busy")?;
    state
        .speakers
        .revoke_chat(&local(&window)?, &chat, &id(&profile)?)
}

/// Recovery after a damaged roster or unusable voices. Returns leftover member
/// folders (not deleted) so the UI can offer a separate, explicit removal.
#[tauri::command]
pub fn speaker_recover(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    owner_name: String,
) -> Result<Vec<String>, String> {
    let _operation = LOCAL_OPERATIONS
        .lock()
        .map_err(|_| "Voice settings are busy")?;
    let dirs = state.speakers.recover(&local(&window)?, &owner_name)?;
    let _ = dirs;
    // New owner identity and old open store handles must never be mixed.
    app.restart();
}

/// The logged-in device user can explicitly return to the owner context.
#[tauri::command]
pub async fn speaker_use_owner(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    local(&window)?;
    let gate = state.send_gate.lock().await;
    if state.snapshot.lock().await.active_turn.is_some() {
        return Err("Wait for the current reply before switching profile".into());
    }
    state.wake.pause_all();
    let old = state.context();
    old.learning.cancel();
    old.initiative.cancel();
    old.session_consent
        .lock()
        .expect("session consent")
        .cancel();
    old.settings_consent
        .lock()
        .expect("settings consent")
        .cancel();
    state
        .generation
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    if let Some(bridge) = old.bridge.lock().await.take() {
        bridge.shutdown().await;
    }
    let identity = state
        .speakers
        .local_lock(&format!("local-{}", uuid::Uuid::new_v4()))?;
    *state.active_context.write().expect("active context") = state.profiles.owner();
    *state.identity.lock().expect("voice identity") = Some(identity);
    *state.voice_presence.lock().expect("voice presence") = None;
    {
        let mut snapshot = state.snapshot.lock().await;
        snapshot.reply.clear();
        snapshot.speech = Default::default();
        snapshot.request_id = None;
        snapshot.phase = "idle".into();
    }
    drop(gate);
    crate::connect(state.inner().clone(), false).await
}
