//! Admission binds capture, private storage and runtime authority before upload.
use crate::AppState;
use notdot_core::{
    chat_tools::ChatPolicy,
    profiles::{Actor, Admission, Capability, Role, SpeakerOutcome, TurnLock},
};
use std::sync::Arc;

impl AppState {
    pub fn can_announce_private(&self) -> bool {
        !self.speakers.needs_identification()
            || (self.current_identity().is_ok()
                && self
                    .voice_presence
                    .lock()
                    .ok()
                    .and_then(|time| *time)
                    .is_some_and(|time| time.elapsed() < std::time::Duration::from_secs(60)))
    }
    pub fn current_identity(&self) -> Result<TurnLock, String> {
        let identity = self
            .identity
            .lock()
            .map_err(|_| "Voice identity unavailable")?
            .clone()
            .ok_or("Please speak to notdot first")?;
        self.speakers.validate(&identity)?;
        if identity.profile != self.context().id {
            return Err("The active person changed".into());
        }
        Ok(identity)
    }
    pub fn chat_policy(
        &self,
        identity: TurnLock,
        settings: &crate::Settings,
        companion: Option<String>,
    ) -> ChatPolicy {
        ChatPolicy {
            roster_path: self.path.with_file_name("profiles.json"),
            turn: identity,
            global_mode: settings.session_control,
            companion_thread: companion,
        }
    }
    pub fn require_owner_voice(&self) -> Result<(), String> {
        if !self.speakers.needs_identification() {
            return Ok(());
        }
        let identity = self.current_identity()?;
        self.speakers
            .roster()?
            .authorise(&Actor::Voice(identity), Capability::ChangeDeviceSettings)
            .map_err(|e| e.to_string())
    }
}

pub async fn bind_capture(
    state: Arc<AppState>,
    token: &str,
    rate: u32,
    pcm: &[u8],
    identified: Option<TurnLock>,
) -> Result<TurnLock, String> {
    let identity = if let Some(identity) = identified {
        identity
    } else if state.speakers.needs_identification() {
        state.speakers.identify(token, rate, pcm).await?
    } else {
        match state
            .speakers
            .roster()?
            .admit(token, &SpeakerOutcome::Disabled)
        {
            Admission::Admit(lock) => lock,
            Admission::Refuse(why) => return Err(why.spoken().into()),
        }
    };
    state.speakers.validate(&identity)?;
    let old = state.context();
    if old.id != identity.profile {
        // Never let a different person answer another person's pending approval.
        if old
            .session_consent
            .lock()
            .expect("session consent")
            .waiting()
            || old
                .settings_consent
                .lock()
                .expect("settings consent")
                .waiting()
            || old.learning.confirmations.waiting(&old.memory)
            || old.notes.waiting().await
        {
            return Err(
                "Finish or cancel the current confirmation before changing speaker.".into(),
            );
        }
        let gate = state.send_gate.lock().await;
        let settings = {
            let s = state.snapshot.lock().await;
            if s.active_turn.is_some() || !matches!(s.phase.as_str(), "idle" | "error") {
                return Err("Wait for the current reply before changing speaker".into());
            }
            s.settings.clone()
        };
        let roster = state.speakers.roster()?;
        let context = state.profiles.bind(
            &roster,
            &identity,
            settings.memory_enabled,
            settings.selection(),
        )?;
        old.learning.cancel();
        old.initiative.cancel();
        old.notes.cancel().await;
        // A restore preview belongs to the person who was active.
        state.backups.cancel();
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
        // Previous subscriptions are generation gated; private model processes remain
        // attached to their own context, never reused by the next speaker.
        if let Some(bridge) = old.bridge.lock().await.take() {
            tauri::async_runtime::spawn(async move {
                bridge.shutdown().await;
            });
        }
        *state.active_context.write().expect("active context") = context;
        *state.identity.lock().expect("voice identity") = Some(identity.clone());
        {
            let mut s = state.snapshot.lock().await;
            s.reply.clear();
            s.request_id = None;
            s.speech = Default::default();
            s.error = None;
            s.phase = "idle".into();
        }
        drop(gate);
    } else {
        *state.identity.lock().expect("voice identity") = Some(identity.clone());
    }
    // Codex connection work belongs after capture. It must not stall this
    // bounded microphone channel while a new person's runtime is starting.
    state.speakers.validate(&identity)?;
    *state.voice_presence.lock().expect("voice presence") = Some(std::time::Instant::now());
    Ok(identity)
}

pub fn check_capture(state: &AppState, token: Option<&str>) -> Result<TurnLock, String> {
    let current = state.current_identity()?;
    if state.speakers.needs_identification() {
        let token = token.ok_or("This request needs an identified voice capture")?;
        let receipt = state.speakers.receipt(token)?;
        state.speakers.validate(&receipt)?;
        if receipt.profile != current.profile || receipt.epoch != current.epoch {
            return Err("The speaker changed. Nothing was sent or saved.".into());
        }
        Ok(receipt)
    } else {
        Ok(current)
    }
}

pub fn member(identity: &TurnLock) -> bool {
    identity.role == Role::Member
}

/// Local security changes revoke a live turn without waiting for another capture.
pub fn spawn_revocation_watch(state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        let mut ticks = tokio::time::interval(std::time::Duration::from_millis(100));
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticks.tick().await;
            let identity = state.identity.lock().expect("voice identity").clone();
            let Some(identity) = identity else { continue };
            if state.speakers.validate(&identity).is_ok() {
                continue;
            }
            let _gate = state.send_gate.lock().await;
            // A new admitted capture may have replaced the revoked identity.
            if state.identity.lock().expect("voice identity").as_ref() != Some(&identity) {
                continue;
            }
            let context = state.context();
            let capture_token = state.wake.status().capture_token;
            state.wake.pause_all();
            if let Some(token) = capture_token {
                state.streaming.cancel(&token).await;
            }
            context.learning.cancel();
            context.initiative.cancel();
            context
                .session_consent
                .lock()
                .expect("session consent")
                .cancel();
            context
                .settings_consent
                .lock()
                .expect("settings consent")
                .cancel();
            state
                .generation
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let active = state.snapshot.lock().await.active_turn.clone();
            if let Some(bridge) = context.bridge.lock().await.take() {
                if let (Some(thread), Some(turn)) = (context.thread_id.lock().await.clone(), active)
                {
                    let _ = bridge.interrupt(&thread, &turn).await;
                }
                bridge.shutdown().await;
            }
            *state.identity.lock().expect("voice identity") = None;
            let mut snapshot = state.snapshot.lock().await;
            snapshot.active_turn = None;
            snapshot.reply.clear();
            snapshot.speech = Default::default();
            snapshot.phase = "idle".into();
            snapshot.error = None;
        }
    });
}
