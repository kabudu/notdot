//! Trusted voices and private profiles: Tauri commands for notdot's own
//! window. Every command here is a deliberate local action (Actor::LocalDevice)
//! and requires the window to be visible and focused. None is exposed to the
//! model, MCP tools or voice; a recognised voice never reaches these paths.
//!
//! The user flow and permission boundaries are described in docs/settings.md.
mod keychain;
pub(crate) mod self_removal;
mod service;

pub use service::{RemovalRoute, Speakers};

use crate::AppState;
use notdot_core::profiles::{Actor, ChatAccess, IdentityMode, ProfileId};
use notdot_speakers::enrol::Progress;
use serde::Serialize;
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

/// What a completed removal did, for accurate status in the UI.
#[derive(Clone, Debug, Serialize)]
pub struct RemovalReport {
    pub name: String,
    pub route: RemovalRoute,
    /// "deleted", or "after_restart" when a store was still closing.
    pub data: &'static str,
    /// "deleted", or "pending" when the voice store was unavailable; the
    /// voice is then never used and is deleted the next time voices load.
    pub voice: &'static str,
    /// "archived", "pending" (notdot keeps retrying) or "none".
    pub chat: &'static str,
}

/// Wakes the archive worker after a removal.
static ARCHIVE_KICK: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// Offboard an authorised member. Order matters for crash safety:
/// 1. record the durable removal marker (from here on, startup finishes the
///    removal even after a crash or power loss);
/// 2. revoke in the roster: profile, grants, receipts, pending self-removal;
///    retire the private chat; delete the voice;
/// 3. switch away if they were the active person and close their runtime;
/// 4. delete their private folder (memory, reminders, reports) and the marker;
/// 5. archive their private chat in Codex, retrying until it succeeds.
///
/// Messages they already added to shared chats are part of those chats and are
/// left as they are.
async fn offboard(
    state: &Arc<AppState>,
    ticket: service::RemovalTicket,
) -> Result<RemovalReport, String> {
    let profile = ticket.profile().clone();
    let name = ticket.name().to_owned();
    let route = ticket.route();
    let gate = state.send_gate.lock().await;
    if state.snapshot.lock().await.active_turn.is_some() {
        return Err("Wait for the current reply before removing a person".into());
    }
    crate::profile_context::record_removal(state.speakers.base(), &profile)?;
    let outcome = {
        let _operation = LOCAL_OPERATIONS
            .lock()
            .map_err(|_| "Voice settings are busy")?;
        state.speakers.remove_member(ticket)?
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
    let data = match state.profiles.remove_member(&profile).await {
        Ok(()) => "deleted",
        Err(_) => "after_restart",
    };
    drop(gate);
    let chat = match &outcome.companion_thread {
        None => "none",
        Some(thread) => {
            let archived = archive_chat(thread).await.is_ok();
            let _ = state.speakers.record_archive(thread, archived);
            ARCHIVE_KICK.notify_one();
            if archived { "archived" } else { "pending" }
        }
    };
    Ok(RemovalReport {
        name,
        route,
        data,
        voice: if outcome.voice_deleted {
            "deleted"
        } else {
            "pending"
        },
        chat,
    })
}

/// Archive one retired chat in Codex. A thread Codex no longer has counts as
/// done: there is nothing left to retain.
async fn archive_chat(thread: &str) -> Result<(), String> {
    let bridge = notdot_core::bridge::Bridge::connect(&crate::startup::runtime()).await?;
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        bridge.call("thread/archive", serde_json::json!({"threadId": thread})),
    )
    .await
    .map_err(|_| "Codex did not answer".to_owned())
    .and_then(|r| r);
    bridge.shutdown().await;
    match result {
        Ok(_) => Ok(()),
        Err(e) if gone(&e) => Ok(()),
        Err(e) => Err(e),
    }
}
fn gone(error: &str) -> bool {
    let e = error.to_lowercase();
    [
        "not found",
        "no such thread",
        "does not exist",
        "unknown thread",
    ]
    .iter()
    .any(|m| e.contains(m))
}

/// Retry archiving retired chats with backoff (30 s doubling to 1 hour),
/// waking early after each removal.
pub fn spawn_archive_worker(state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        loop {
            let now = unix_ms();
            let Ok(roster) = state.speakers.roster() else {
                return;
            };
            for chat in roster.archives_due(now) {
                let archived = archive_chat(&chat).await.is_ok();
                let _ = state.speakers.record_archive(&chat, archived);
            }
            let next = state
                .speakers
                .roster()
                .ok()
                .and_then(|r| {
                    r.retired_chats()
                        .iter()
                        .filter_map(|c| match c.archive {
                            notdot_core::profiles::ArchiveState::Pending {
                                next_attempt_unix_ms,
                                ..
                            } => Some(next_attempt_unix_ms),
                            _ => None,
                        })
                        .min()
                })
                .map_or(3_600_000, |at| {
                    at.saturating_sub(unix_ms()).clamp(1_000, 3_600_000)
                });
            tokio::select! {
                () = tokio::time::sleep(std::time::Duration::from_millis(next)) => {}
                () = ARCHIVE_KICK.notified() => {}
            }
        }
    });
}
fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

/// The principal owner removes a trusted member, at this computer, after a
/// dialog naming the person. `confirm_name` must match their current name.
#[tauri::command]
pub async fn speaker_remove_person(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    profile: String,
    confirm_name: String,
) -> Result<RemovalReport, String> {
    let ticket = state.speakers.authorise_removal(
        &local(&window)?,
        &id(&profile)?,
        &confirm_name,
        RemovalRoute::Owner,
    )?;
    offboard(&state, ticket).await
}

/// A trusted member finishes removing their own profile on this computer,
/// after asking by voice and saying the one-time code.
#[tauri::command]
pub async fn speaker_confirm_self_removal(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    profile: String,
    confirm_name: String,
) -> Result<RemovalReport, String> {
    let ticket = state.speakers.authorise_removal(
        &local(&window)?,
        &id(&profile)?,
        &confirm_name,
        RemovalRoute::SelfService,
    )?;
    offboard(&state, ticket).await
}

#[tauri::command]
pub fn speaker_cancel_self_removal(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    local(&window)?;
    state.speakers.self_removal_cancel();
    Ok(())
}

/// Finish any removal interrupted by a crash or power loss: every durable
/// marker is completed in the roster before private folders are deleted.
pub fn resume_offboarding(speakers: &Speakers) -> Result<(), String> {
    for id in crate::profile_context::pending_removals(speakers.base())? {
        speakers.complete_recorded_removal(&id)?;
    }
    Ok(())
}

#[tauri::command]
pub fn speaker_enrol_begin(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    profile: String,
    name: Option<String>,
) -> Result<Progress, String> {
    let _operation = LOCAL_OPERATIONS
        .lock()
        .map_err(|_| "Voice settings are busy")?;
    let actor = local(&window)?;
    let profile = id(&profile)?;
    if let Some(name) = name.filter(|n| !n.trim().is_empty()) {
        state.speakers.rename(&actor, &profile, &name)?;
    }
    state.speakers.enrol_begin(&actor, &profile)
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
    consent: bool,
) -> Result<(), String> {
    let _operation = LOCAL_OPERATIONS
        .lock()
        .map_err(|_| "Voice settings are busy")?;
    state.speakers.enrol_save(&local(&window)?, consent)
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
