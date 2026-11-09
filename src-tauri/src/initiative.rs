use super::AppState;
use chrono::{Local, Timelike};
use notdot_core::initiative::{Behaviour, Entry, Journal};
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::State;
use tokio::sync::Mutex;
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
#[derive(Clone, Default, Serialize)]
pub struct Status {
    pub available: usize,
    pub announce: Option<String>,
    pub task_report: bool,
    pub error: Option<String>,
    pub tasks: notdot_core::task_awareness::Awareness,
}
fn visible_to(entry: &Entry, profile: &notdot_core::profiles::ProfileId) -> bool {
    entry.origin_allowed()
        && entry
            .profile_origin
            .as_ref()
            .is_none_or(|origin| &origin.turn.profile == profile)
}
pub struct Initiative {
    journal: Mutex<Journal>,
    path: PathBuf,
    error: Mutex<Option<String>>,
    refresh: Mutex<()>,
    cancellation: AtomicU64,
}
impl Initiative {
    pub fn open(path: PathBuf) -> Result<Self, String> {
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1024 * 1024) {
            return Err("Follow-up journal is too large".into());
        }
        let journal = match std::fs::read(&path) {
            Ok(bytes) if bytes.len() <= 1024 * 1024 => serde_json::from_slice::<Journal>(&bytes)
                .map_err(|e| format!("Read follow-ups: {e}"))?,
            Ok(_) => return Err("Follow-up journal is too large".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Journal::default(),
            Err(e) => return Err(format!("Read follow-ups: {e}")),
        };
        if journal.entries.len() > 256 {
            return Err("Too many follow-ups in journal".into());
        }
        Ok(Self {
            journal: Mutex::new(journal),
            path,
            error: Mutex::new(None),
            refresh: Mutex::new(()),
            cancellation: AtomicU64::new(0),
        })
    }
    async fn save(&self, journal: &Journal) -> Result<(), String> {
        let bytes = serde_json::to_vec(journal).map_err(|e| e.to_string())?;
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            use std::io::Write;
            let temp = path.with_extension("tmp");
            let mut o = std::fs::OpenOptions::new();
            o.write(true).truncate(true).create(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                o.mode(0o600);
            }
            let mut f = o.open(&temp)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
            std::fs::rename(temp, path)
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("Save follow-ups: {e}"))
    }
    pub fn cancel(&self) {
        self.cancellation.fetch_add(1, Ordering::Relaxed);
    }
    pub async fn remove_inferred(&self, id: Option<&str>) -> Result<(), String> {
        let mut j = self.journal.lock().await;
        let mut updated = j.clone();
        updated.entries.retain(|e| {
            !e.source.starts_with("memory:")
                || id.is_some_and(|id| e.source != format!("memory:{id}"))
        });
        if updated != *j {
            self.save(&updated).await?;
            *j = updated;
        }
        Ok(())
    }
    pub async fn status(
        &self,
        policy: &notdot_core::initiative::Policy,
        profile: &notdot_core::profiles::ProfileId,
    ) -> Status {
        let mut j = self.journal.lock().await.clone();
        j.entries.retain(|entry| visible_to(entry, profile));
        let date = Local::now();
        let next = j.next(
            policy,
            now(),
            &date.format("%Y-%m-%d").to_string(),
            (date.hour() * 60 + date.minute()) as u16,
        );
        Status {
            available: j.available(policy, now()).len(),
            announce: next.map(|e| e.id.clone()),
            task_report: next.is_some_and(|e| e.task_report()),
            error: self.error.lock().await.clone(),
            tasks: notdot_core::task_awareness::Awareness::new(
                j.entries
                    .iter()
                    .filter_map(|entry| entry.task_cue(now()).cloned())
                    .collect(),
            ),
        }
    }
    async fn add(&self, entry: Entry) -> Result<(), String> {
        let mut j = self.journal.lock().await;
        let mut updated = j.clone();
        updated.insert(entry, now())?;
        if updated != *j {
            self.save(&updated).await?;
            *j = updated;
        }
        Ok(())
    }
    pub async fn task_completed(
        &self,
        completion: &notdot_core::session_runtime::Completion,
    ) -> Result<(), String> {
        let time = now();
        let entry = Entry {
            id: uuid::Uuid::new_v4().to_string(), key: completion.key(),
            message: completion.message(), source: completion.key(),
            evidence: "Status from the exact Codex turn started by notdot. A completed turn does not independently verify external work.".into(),
            created: time, due: time, expires: time + 7 * 86400,
            permitted: completion.announce, delivered: false, dismissed: false,
            profile_origin: completion.origin.clone(),
            task: Some(completion.task()),
        };
        let mut journal = self.journal.lock().await;
        let mut updated = journal.clone();
        if let Some(previous) = updated.entries.iter_mut().find(|e| {
            e.key == entry.key
                && e.expires > time
                && e.task
                    .as_ref()
                    .is_some_and(|t| t.activity == notdot_core::task_awareness::Activity::Attention)
                && entry
                    .task
                    .as_ref()
                    .is_some_and(|t| t.activity == notdot_core::task_awareness::Activity::Finished)
        }) {
            // A terminal result replaces the earlier attention notice. Preserve
            // acknowledgement and identity; duplicate terminal events remain no-ops.
            let id = previous.id.clone();
            let dismissed = previous.dismissed;
            *previous = entry;
            previous.id = id;
            previous.dismissed = dismissed;
        } else {
            updated.insert(entry, time)?;
        }
        if updated != *journal {
            self.save(&updated).await?;
            *journal = updated;
        }
        Ok(())
    }

    pub async fn task_command(
        &self,
        text: &str,
        live: Vec<notdot_core::task_awareness::Task>,
    ) -> Option<String> {
        let request = text
            .trim()
            .trim_end_matches(['.', '?'])
            .to_lowercase()
            .replace('’', "'");
        let request = request.strip_prefix("please ").unwrap_or(&request);
        let filter = match request {
            "what needs me" | "what needs my attention" | "which tasks need me" => 1,
            "what's still running"
            | "what is still running"
            | "what's running"
            | "which tasks are still running" => 2,
            "what are my task updates"
            | "read my task updates"
            | "any task updates"
            | "how are my tasks doing" => 0,
            "clear task updates"
            | "dismiss task updates"
            | "clear my task updates"
            | "dismiss my task updates" => 3,
            _ => return None,
        };
        let mut journal = self.journal.lock().await;
        if filter == 3 {
            let mut updated = journal.clone();
            for entry in &mut updated.entries {
                if entry.task.is_some() {
                    entry.dismissed = true;
                }
            }
            return Some(match self.save(&updated).await {
                Ok(()) => {
                    *journal = updated;
                    "Cleared saved task updates. Running tasks are unchanged.".into()
                }
                Err(e) => e,
            });
        }
        let mut tasks = live;
        for task in journal
            .entries
            .iter()
            .filter(|e| !e.dismissed && e.expires > now() && e.origin_allowed())
            .filter_map(|e| e.task.clone())
        {
            if !tasks.iter().any(|t| t.key == task.key) {
                tasks.push(task);
            }
        }
        if filter == 2 {
            tasks.retain(|t| t.activity != notdot_core::task_awareness::Activity::Finished);
        }
        let messages: Vec<_> = tasks
            .iter()
            .filter(|task| {
                filter != 1
                    || task.activity == notdot_core::task_awareness::Activity::Attention
                    || (task.activity == notdot_core::task_awareness::Activity::Finished
                        && task.outcome != notdot_core::task_awareness::Outcome::Success)
            })
            .take(8)
            .map(|task| {
                journal
                    .entries
                    .iter()
                    .find(|entry| {
                        entry.key == task.key
                            && !entry.dismissed
                            && entry.expires > now()
                            && entry.origin_allowed()
                    })
                    .map_or_else(|| task.description(), |entry| entry.message.clone())
            })
            .collect();
        Some(if messages.is_empty() {
            notdot_core::task_awareness::Awareness::new(tasks).answer(filter == 1)
        } else {
            messages.join(" ")
        })
    }
    pub async fn command(&self, text: &str, behaviour: Behaviour) -> Option<String> {
        if [
            "what are my reminders",
            "what are my suggestions",
            "read my follow-ups",
            "read my reminders",
        ]
        .contains(
            &text
                .trim()
                .trim_end_matches(['.', '?'])
                .to_lowercase()
                .as_str(),
        ) {
            let j = self.journal.lock().await;
            let messages: Vec<_> = j
                .entries
                .iter()
                .filter(|e| !e.dismissed && !e.delivered && e.expires > now() && e.origin_allowed())
                .take(5)
                .map(|e| e.message.as_str())
                .collect();
            return Some(if messages.is_empty() {
                "You have no pending reminders or follow-ups.".into()
            } else {
                messages.join(". ")
            });
        }
        let result = notdot_core::initiative::reminder(text, now())?;
        let (message, due) = match result {
            Ok(r) => r,
            Err(e) => return Some(e),
        };
        let time = now();
        let result = self
            .add(Entry {
                id: uuid::Uuid::new_v4().to_string(),
                key: uuid::Uuid::new_v4().to_string(),
                message,
                source: "Explicit spoken reminder".into(),
                evidence: text.into(),
                created: time,
                due,
                expires: due + 7 * 86400,
                permitted: true,
                delivered: false,
                dismissed: false,
                profile_origin: None,
                task: None,
            })
            .await;
        Some(match result { Err(e)=>e,Ok(())=>match behaviour { Behaviour::Reactive=>"Reminder saved. Automatic announcements are off in Reactive mode; choose Suggestions or Proactive in Settings to surface it.".into(),Behaviour::Suggestions=>"Reminder saved. It will appear silently on the orb when due. Proactive mode is needed for a spoken reminder.".into(),Behaviour::Proactive=>"Reminder saved. I'll announce it when you're idle, respecting your quiet hours and frequency limits.".into() } })
    }
}
#[tauri::command]
pub async fn followups(state: State<'_, Arc<AppState>>) -> Result<Vec<Entry>, String> {
    let context = state.context();
    Ok(context
        .initiative
        .journal
        .lock()
        .await
        .entries
        .iter()
        .filter(|e| !e.dismissed && e.expires > now() && visible_to(e, &context.id))
        .cloned()
        .collect())
}
#[tauri::command]
pub fn cancel_followup_speech(state: State<'_, Arc<AppState>>) {
    let context = state.context();
    context.initiative.cancel();
}
#[tauri::command]
pub async fn followup_action(
    state: State<'_, Arc<AppState>>,
    id: String,
    action: String,
) -> Result<(), String> {
    let context = state.context();
    let mut j = context.initiative.journal.lock().await;
    let mut updated = j.clone();
    updated.action(&id, &action, now())?;
    context.initiative.save(&updated).await?;
    *j = updated;
    Ok(())
}
#[tauri::command]
pub async fn announce_followup(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<(), String> {
    let context = state.context();
    if !state.can_announce_private() {
        return Err("Please speak to notdot before hearing a private follow-up".into());
    }
    let generation = context.initiative.cancellation.load(Ordering::Relaxed);
    if !window.is_visible().unwrap_or(false) || window.is_minimized().unwrap_or(true) {
        return Err("Show notdot to hear a follow-up".into());
    }
    let _turn = state
        .send_gate
        .try_lock()
        .map_err(|_| "Wait for the current submission")?;
    let _voice = state
        .voice_gate
        .try_acquire_many(4)
        .map_err(|_| "Wait for voice processing")?;
    let policy = {
        let s = state.snapshot.lock().await;
        if s.phase != "idle"
            || s.active_turn.is_some()
            || s.settings.microphone_muted
            || !s.window_visible
            || state.wake.status().phase == "capturing"
        {
            return Err("Wait for the current turn".into());
        }
        s.settings.initiative.clone()
    };
    let source = context
        .initiative
        .journal
        .lock()
        .await
        .entries
        .iter()
        .find(|e| e.id == id)
        .and_then(|e| e.source.strip_prefix("memory:").map(str::to_owned));
    let memory_ticket = context.memory.learning_ticket();
    if let Some(source) = &source
        && (memory_ticket.is_none()
            || !context.memory.review("").await?.iter().any(|n| {
                n.id == *source
                    && !n
                        .metadata
                        .get("learning_state")
                        .is_some_and(|s| s == "provisional")
            }))
    {
        context.initiative.remove_inferred(Some(source)).await?;
        return Err("The source for this follow-up is no longer available".into());
    }
    let date = Local::now();
    let time = now();
    let message = {
        let mut j = context.initiative.journal.lock().await;
        let mut updated = j.clone();
        let entry = updated
            .entries
            .iter()
            .find(|e| e.id == id)
            .ok_or("Follow-up unavailable")?;
        if !visible_to(entry, &context.id) {
            return Err("Follow-up permission changed".into());
        }
        if !state.can_announce_private() {
            return Err("Private follow-up permission expired".into());
        }
        let message = updated.claim(
            &id,
            &policy,
            time,
            &date.format("%Y-%m-%d").to_string(),
            (date.hour() * 60 + date.minute()) as u16,
        )?;
        // Persist before audio. A crash can lose an alert but cannot repeat it.
        context.initiative.save(&updated).await?;
        *j = updated;
        message
    };
    state.wake.ready(false);
    if !state.can_announce_private()
        || state.context().id != context.id
        || context.initiative.cancellation.load(Ordering::Relaxed) != generation
    {
        return Err("Follow-up cancelled".into());
    }
    let task_report = source.is_none()
        && context
            .initiative
            .journal
            .lock()
            .await
            .entries
            .iter()
            .any(|entry| entry.id == id && entry.task_report());
    let interrupt = async {
        let mut ticks = tokio::time::interval(std::time::Duration::from_millis(100));
        loop {
            ticks.tick().await;
            if !state.can_announce_private()
                || state.context().id != context.id
                || !window.is_visible().unwrap_or(false)
                || window.is_minimized().unwrap_or(true)
                || (source.is_some() && context.memory.learning_ticket() != memory_ticket)
                || context.initiative.cancellation.load(Ordering::Relaxed) != generation
            {
                break;
            }
            let s = state.snapshot.lock().await;
            if s.settings.microphone_muted
                || !s.window_visible
                || (!task_report && s.settings.initiative.behaviour != Behaviour::Proactive)
            {
                break;
            }
            let j = context.initiative.journal.lock().await;
            if j.entries.iter().find(|e| e.id == id).is_none_or(|e| {
                e.dismissed
                    || !visible_to(e, &context.id)
                    || (e.source.starts_with("memory:") && !s.settings.memory_enabled)
            }) {
                break;
            }
        }
    };
    let result = tokio::select! {
        r = super::system_speech::speak(&message) => r,
        _ = interrupt => return Err("Follow-up cancelled".into()),
    };
    if let Err(ref e) = result {
        *context.initiative.error.lock().await =
            Some(format!("Spoken follow-up failed; read it in Settings: {e}"));
    }
    result
}
/// Bounded, event-driven runtime queue; polling only moves completed receipts to
/// the existing durable journal. It never polls Codex or starts another turn.
pub fn spawn_task_reports(state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        let mut ticks = tokio::time::interval(std::time::Duration::from_millis(500));
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticks.tick().await;
            let Ok(contexts) = state.profiles.all() else {
                continue;
            };
            for context in contexts {
                let runtime = context.session_runtime.lock().await.clone();
                let Some(runtime) = runtime else { continue };
                let Some(completion) = runtime.completion().await else {
                    continue;
                };
                match context.initiative.task_completed(&completion).await {
                    Ok(()) => {
                        let mut outcomes = context.session_outcomes.lock().await;
                        outcomes.push_back(format!(
                            "Task report for chat {}: {}",
                            completion.thread_id,
                            completion.message()
                        ));
                        while outcomes.len() > 8 {
                            outcomes.pop_front();
                        }
                        drop(outcomes);
                        runtime.acknowledge_completion(&completion.key()).await;
                    }
                    Err(error) => {
                        *context.initiative.error.lock().await =
                            Some(format!("Could not save a task report: {error}"));
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                }
            }
        }
    });
}
/// Discover only source-grounded ongoing goals/projects. Discovery grants no
/// interruption permission; each inferred check-in requires an explicit Allow.
fn ongoing_key(key: &str) -> bool {
    key.split(['_', '.'])
        .any(|word| matches!(word, "project" | "goal" | "commitment" | "ongoing"))
}
pub fn spawn(state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        let mut ticks = tokio::time::interval(std::time::Duration::from_secs(30));
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticks.tick().await;
            let context = state.context();
            let _refresh = context.initiative.refresh.lock().await;
            let enabled = {
                let s = state.snapshot.lock().await;
                s.settings.memory_enabled && s.settings.initiative.behaviour != Behaviour::Reactive
            };
            if !enabled {
                continue;
            }
            let Some(ticket) = context.memory.learning_ticket() else {
                continue;
            };
            let result = async {
                let notes = context.memory.review("").await?;
                let ids: std::collections::BTreeSet<_> =
                    notes.iter().map(|n| n.id.clone()).collect();
                // Forget/correct/disable must not leave an inferred reminder behind.
                let mut j = context.initiative.journal.lock().await;
                let mut updated = j.clone();
                updated.entries.retain(|e| {
                    !e.source.starts_with("memory:")
                        || ids.contains(e.source.trim_start_matches("memory:"))
                });
                for note in notes {
                    let key = note
                        .metadata
                        .get("semantic_key")
                        .map(String::as_str)
                        .unwrap_or("");
                    if !ongoing_key(key) {
                        continue;
                    }
                    if note
                        .metadata
                        .get("learning_state")
                        .is_some_and(|s| s == "provisional")
                    {
                        continue;
                    }
                    let time = now();
                    let expires = note.updated_at_unix_ms / 1000 + 7 * 86400;
                    if expires <= time || note.content.len() > 950 {
                        continue;
                    }
                    updated.insert(
                        Entry {
                            id: uuid::Uuid::new_v4().to_string(),
                            key: format!("memory:{}", note.id),
                            message: format!(
                                "Would you like to follow up on this? {}",
                                note.content
                            ),
                            source: format!("memory:{}", note.id),
                            evidence: note.metadata.get("evidence").cloned().unwrap_or_default(),
                            created: time,
                            due: time,
                            expires,
                            permitted: false,
                            delivered: false,
                            dismissed: false,
                            profile_origin: None,
                            task: None,
                        },
                        time,
                    )?;
                }
                if state.context().id != context.id
                    || context.memory.learning_ticket() != Some(ticket)
                {
                    return Ok::<_, String>(());
                }
                if updated != *j {
                    context.initiative.save(&updated).await?;
                    *j = updated;
                }
                Ok(())
            }
            .await;
            if let Err(e) = result {
                *context.initiative.error.lock().await = Some(e);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_report_is_hidden_from_another_profile_even_with_valid_origin() {
        use notdot_core::{
            chat_tools::ChatPolicy,
            profiles::{ProfileId, Role, Roster, TurnLock},
        };
        let directory = tempfile::tempdir().unwrap();
        let roster = Roster::new("Owner").unwrap();
        let path = directory.path().join("profiles.json");
        roster.save(&path).unwrap();
        let owner = roster.owner().clone();
        let entry = Entry {
            id: "private".into(),
            key: "private".into(),
            message: "Private reminder".into(),
            source: "reminder".into(),
            evidence: String::new(),
            created: 1,
            due: 1,
            expires: u64::MAX,
            permitted: true,
            delivered: false,
            dismissed: false,
            profile_origin: Some(ChatPolicy {
                roster_path: path,
                turn: TurnLock {
                    capture_token: "test-capture".into(),
                    profile: owner.clone(),
                    role: Role::Owner,
                    epoch: 0,
                    open_fallback: false,
                },
                global_mode: notdot_core::session_control::Mode::ReadOnly,
                companion_thread: None,
            }),
            task: None,
        };
        assert!(visible_to(&entry, &owner));
        assert!(!visible_to(&entry, &ProfileId::random()));
    }

    #[test]
    fn ongoing_category_matches_complete_words_in_any_key_position() {
        for key in [
            "project_birdhouse",
            "garden_birdhouse_project",
            "garden.project.birdhouse",
            "goal_fitness",
            "fitness_goal",
            "commitment_call",
            "ongoing_task",
        ] {
            assert!(ongoing_key(key), "{key}");
        }
        for key in [
            "",
            "preferred_drink",
            "projection",
            "goalkeeper",
            "commitmentphobia",
            "ongoingly",
        ] {
            assert!(!ongoing_key(key), "{key}");
        }
    }
    #[tokio::test]
    async fn spoken_reminder_and_inferred_permission_survive_restart_without_memory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("followups.json");
        let service = Initiative::open(path.clone()).unwrap();
        let response = service
            .command(
                "Remind me in two minutes to drink water",
                Behaviour::Suggestions,
            )
            .await
            .unwrap();
        assert!(response.contains("silently"));
        assert!(
            service
                .command("ordinary conversation", Behaviour::Reactive)
                .await
                .is_none()
        );
        let list = service.journal.lock().await.clone();
        assert_eq!(list.entries.len(), 1);
        assert!(list.entries[0].permitted);
        assert!(list.entries[0].evidence.contains("two minutes"));
        drop(service);
        let restored = Initiative::open(path).unwrap();
        assert_eq!(
            restored.journal.lock().await.entries[0].id,
            list.entries[0].id
        );
        let time = now();
        restored
            .add(Entry {
                id: "inferred".into(),
                key: "inferred".into(),
                message: "Follow up?".into(),
                source: "memory:note".into(),
                evidence: "I am building a companion".into(),
                created: time,
                due: time,
                expires: time + 86400,
                permitted: false,
                delivered: false,
                dismissed: false,
                profile_origin: None,
                task: None,
            })
            .await
            .unwrap();
        let p = notdot_core::initiative::Policy {
            behaviour: Behaviour::Proactive,
            quiet_start: 0,
            quiet_end: 0,
            ..Default::default()
        };
        assert_eq!(
            restored
                .status(&p, &notdot_core::profiles::ProfileId::random())
                .await
                .available,
            1
        );
        assert!(
            restored
                .status(&p, &notdot_core::profiles::ProfileId::random())
                .await
                .announce
                .is_none()
        );
        restored.remove_inferred(None).await.unwrap();
        assert_eq!(restored.journal.lock().await.entries.len(), 1);
        assert!(
            restored
                .command("what are my reminders?", Behaviour::Reactive)
                .await
                .unwrap()
                .contains("drink water")
        );
    }
    #[tokio::test]
    async fn persistence_failure_never_reports_a_scheduled_reminder() {
        let dir = tempfile::tempdir().unwrap();
        let service = Initiative::open(dir.path().join("missing").join("journal.json")).unwrap();
        let response = service
            .command("Remind me in ten minutes to rest", Behaviour::Proactive)
            .await
            .unwrap();
        assert!(response.contains("Save follow-ups"));
        assert!(service.journal.lock().await.entries.is_empty());
    }
}

#[cfg(test)]
mod task_completion_tests {
    use super::*;
    #[tokio::test]
    #[ignore = "Plays four short task reports through the computer's system voice"]
    async fn task_reports_play_through_system_speech_without_provider_keys() {
        use notdot_core::task_awareness::{Check, CheckResult, Outcome, Report};
        for (title, outcome, result, reason) in [
            (
                "Draft",
                Outcome::Success,
                CheckResult::Passed,
                "The requested draft is prepared.",
            ),
            (
                "Website",
                Outcome::Failed,
                CheckResult::Failed,
                "Two tests failed.",
            ),
            (
                "Research",
                Outcome::Partial,
                CheckResult::Passed,
                "Two of three prices were checked.",
            ),
            (
                "Booking",
                Outcome::Unverified,
                CheckResult::NotRun,
                "No booking receipt is available.",
            ),
        ] {
            let completion = notdot_core::session_runtime::Completion {
                thread_id: "synthetic".into(),
                turn_id: title.into(),
                title: title.into(),
                status: "completed".into(),
                summary: String::new(),
                report: Some(Report {
                    attention: None,
                    outcome,
                    summary: reason.into(),
                    checks: vec![Check {
                        name: "Requested result".into(),
                        result,
                        evidence: reason.into(),
                    }],
                    remaining: if outcome == Outcome::Partial {
                        vec!["One price still needs checking".into()]
                    } else {
                        vec![]
                    },
                }),
                announce: true,
                origin: None,
            };
            let message = completion.message();
            assert!(message.chars().count() <= 301);
            super::super::system_speech::speak(&message).await.unwrap();
        }
    }
    #[tokio::test]
    async fn task_outcomes_persist_without_granting_speech_and_acknowledgement_is_durable() {
        use notdot_core::task_awareness::{Check, CheckResult, Outcome, Report};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.json");
        let initiative = Initiative::open(path.clone()).unwrap();
        let completion = notdot_core::session_runtime::Completion {
            thread_id: "draft".into(),
            turn_id: "one".into(),
            title: "Draft".into(),
            status: "completed".into(),
            summary: String::new(),
            report: Some(Report {
                attention: None,
                outcome: Outcome::Partial,
                summary: "Draft prepared, delivery not requested".into(),
                checks: vec![Check {
                    name: "Draft".into(),
                    result: CheckResult::Passed,
                    evidence: "Prepared document".into(),
                }],
                remaining: vec!["Review the draft".into()],
            }),
            announce: false,
            origin: None,
        };
        initiative.task_completed(&completion).await.unwrap();
        let restored = Initiative::open(path.clone()).unwrap();
        let policy = notdot_core::initiative::Policy {
            quiet_start: 0,
            quiet_end: 0,
            ..Default::default()
        };
        let profile = notdot_core::profiles::ProfileId::random();
        let status = restored.status(&policy, &profile).await;
        assert_eq!(status.tasks.cue, "review");
        assert!(status.announce.is_none());
        let waiting = notdot_core::task_awareness::Task {
            key: "live-task".into(),
            title: "Appointment".into(),
            activity: notdot_core::task_awareness::Activity::Attention,
            outcome: Outcome::Unverified,
            attention: Some(notdot_core::task_awareness::Attention::Approval),
        };
        let running = restored
            .task_command("Please what's still running?", vec![waiting])
            .await
            .unwrap();
        assert!(running.contains("Appointment needs attention"));
        assert!(!running.contains("Draft"));
        assert!(
            restored
                .task_command("What needs me?", vec![])
                .await
                .unwrap()
                .contains("partly complete")
        );
        restored
            .task_command("clear task updates", vec![])
            .await
            .unwrap();
        let restarted = Initiative::open(path).unwrap();
        assert_eq!(restarted.status(&policy, &profile).await.tasks.cue, "none");
        restarted.task_completed(&completion).await.unwrap();
        assert_eq!(
            restarted.status(&policy, &profile).await.tasks.cue,
            "none",
            "duplicate cannot resurrect an acknowledged report"
        );
    }
    #[tokio::test]
    async fn cancelling_an_attention_task_updates_its_saved_result_without_resurrection() {
        use notdot_core::task_awareness::{Activity, Outcome};
        for dismissed in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("tasks.json");
            let initiative = Initiative::open(path.clone()).unwrap();
            let mut completion = notdot_core::session_runtime::Completion {
                thread_id: "synthetic".into(),
                turn_id: "owned".into(),
                title: "Draft".into(),
                status: "approval_required".into(),
                summary: String::new(),
                report: None,
                announce: true,
                origin: None,
            };
            initiative.task_completed(&completion).await.unwrap();
            {
                let mut journal = initiative.journal.lock().await;
                journal.entries[0].delivered = true;
                journal.entries[0].dismissed = dismissed;
                initiative.save(&journal).await.unwrap();
            }
            let restored = Initiative::open(path.clone()).unwrap();
            completion.status = "interrupted".into();
            restored.task_completed(&completion).await.unwrap();
            restored.task_completed(&completion).await.unwrap();
            let restarted = Initiative::open(path).unwrap();
            let journal = restarted.journal.lock().await;
            assert_eq!(journal.entries.len(), 1);
            let entry = &journal.entries[0];
            assert_eq!(entry.task.as_ref().unwrap().activity, Activity::Finished);
            assert_eq!(entry.task.as_ref().unwrap().outcome, Outcome::Failed);
            assert_eq!(entry.dismissed, dismissed);
            assert!(entry.message.contains("interrupted"));
            assert!(!entry.message.contains("needs attention"));
            drop(journal);
            {
                let mut journal = restarted.journal.lock().await;
                journal.entries[0].delivered = true;
                restarted.save(&journal).await.unwrap();
            }
            restarted.task_completed(&completion).await.unwrap();
            assert!(
                restarted.journal.lock().await.entries[0].delivered,
                "duplicate terminal events cannot requeue speech"
            );
        }
    }
    #[tokio::test]
    async fn completed_report_survives_restart_and_duplicate_events_without_redelivery() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("followups.json");
        let initiative = Initiative::open(path.clone()).unwrap();
        let completion = notdot_core::session_runtime::Completion {
            thread_id: "synthetic".into(),
            turn_id: "owned-turn".into(),
            title: "Synthetic task".into(),
            status: "completed".into(),
            summary: "The synthetic result is ready.".into(),
            report: None,
            announce: true,
            origin: None,
        };
        initiative.task_completed(&completion).await.unwrap();
        initiative.task_completed(&completion).await.unwrap();
        let restored = Initiative::open(path.clone()).unwrap();
        let mut journal = restored.journal.lock().await;
        assert_eq!(journal.entries.len(), 1);
        let id = journal.entries[0].id.clone();
        let policy = notdot_core::initiative::Policy {
            quiet_start: 0,
            quiet_end: 0,
            ..Default::default()
        };
        let time = now();
        assert!(
            journal
                .claim(&id, &policy, time, "today", 720)
                .unwrap()
                .contains("synthetic result")
        );
        restored.save(&journal).await.unwrap();
        drop(journal);
        let restarted = Initiative::open(path).unwrap();
        restarted.task_completed(&completion).await.unwrap();
        let journal = restarted.journal.lock().await;
        assert_eq!(journal.entries.len(), 1);
        assert!(journal.next(&policy, time + 10, "today", 720).is_none());
    }
}
