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
    pub error: Option<String>,
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
    pub async fn status(&self, policy: &notdot_core::initiative::Policy) -> Status {
        let j = self.journal.lock().await;
        let date = Local::now();
        Status {
            available: j.available(policy, now()).len(),
            announce: j
                .next(
                    policy,
                    now(),
                    &date.format("%Y-%m-%d").to_string(),
                    (date.hour() * 60 + date.minute()) as u16,
                )
                .map(|e| e.id.clone()),
            error: self.error.lock().await.clone(),
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
                .filter(|e| !e.dismissed && !e.delivered && e.expires > now())
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
            })
            .await;
        Some(match result { Err(e)=>e,Ok(())=>match behaviour { Behaviour::Reactive=>"Reminder saved. Automatic announcements are off in Reactive mode; choose Suggestions or Proactive in Settings to surface it.".into(),Behaviour::Suggestions=>"Reminder saved. It will appear silently on the orb when due. Proactive mode is needed for a spoken reminder.".into(),Behaviour::Proactive=>"Reminder saved. I'll announce it when you're idle, respecting your quiet hours and frequency limits.".into() } })
    }
}
#[tauri::command]
pub async fn followups(state: State<'_, Arc<AppState>>) -> Result<Vec<Entry>, String> {
    Ok(state
        .initiative
        .journal
        .lock()
        .await
        .entries
        .iter()
        .filter(|e| !e.dismissed && e.expires > now())
        .cloned()
        .collect())
}
#[tauri::command]
pub async fn followup_action(
    state: State<'_, Arc<AppState>>,
    id: String,
    action: String,
) -> Result<(), String> {
    let mut j = state.initiative.journal.lock().await;
    let mut updated = j.clone();
    updated.action(&id, &action, now())?;
    state.initiative.save(&updated).await?;
    *j = updated;
    Ok(())
}
#[tauri::command]
pub async fn announce_followup(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
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
        if s.phase != "idle" || s.active_turn.is_some() || state.wake.status().phase == "capturing"
        {
            return Err("Wait for the current turn".into());
        }
        s.settings.initiative.clone()
    };
    let source = state
        .initiative
        .journal
        .lock()
        .await
        .entries
        .iter()
        .find(|e| e.id == id)
        .and_then(|e| e.source.strip_prefix("memory:").map(str::to_owned));
    let memory_ticket = state.memory.learning_ticket();
    if let Some(source) = &source
        && (memory_ticket.is_none()
            || !state.memory.review("").await?.iter().any(|n| {
                n.id == *source
                    && !n
                        .metadata
                        .get("learning_state")
                        .is_some_and(|s| s == "provisional")
            }))
    {
        state.initiative.remove_inferred(Some(source)).await?;
        return Err("The source for this follow-up is no longer available".into());
    }
    let date = Local::now();
    let time = now();
    let message = {
        let mut j = state.initiative.journal.lock().await;
        let mut updated = j.clone();
        let message = updated.claim(
            &id,
            &policy,
            time,
            &date.format("%Y-%m-%d").to_string(),
            (date.hour() * 60 + date.minute()) as u16,
        )?;
        // Persist before audio. A crash can lose an alert but cannot repeat it.
        state.initiative.save(&updated).await?;
        *j = updated;
        message
    };
    state.wake.ready(false);
    let generation = state.initiative.cancellation.load(Ordering::Relaxed);
    let interrupt = async {
        let mut ticks = tokio::time::interval(std::time::Duration::from_millis(100));
        loop {
            ticks.tick().await;
            if (source.is_some() && state.memory.learning_ticket() != memory_ticket)
                || state.initiative.cancellation.load(Ordering::Relaxed) != generation
            {
                break;
            }
            let s = state.snapshot.lock().await;
            if s.settings.initiative.behaviour != Behaviour::Proactive {
                break;
            }
            let j = state.initiative.journal.lock().await;
            if j.entries.iter().find(|e| e.id == id).is_none_or(|e| {
                e.dismissed || (e.source.starts_with("memory:") && !s.settings.memory_enabled)
            }) {
                break;
            }
        }
    };
    let result = tokio::select! {r=super::system_speech::speak(&message)=>r,_=interrupt=>Err("Follow-up permission was withdrawn".into())};
    if let Err(ref e) = result {
        *state.initiative.error.lock().await =
            Some(format!("Spoken follow-up failed; read it in Settings: {e}"));
    }
    result
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
            let _refresh = state.initiative.refresh.lock().await;
            let enabled = {
                let s = state.snapshot.lock().await;
                s.settings.memory_enabled && s.settings.initiative.behaviour != Behaviour::Reactive
            };
            if !enabled {
                continue;
            }
            let Some(ticket) = state.memory.learning_ticket() else {
                continue;
            };
            let result = async {
                let notes = state.memory.review("").await?;
                let ids: std::collections::BTreeSet<_> =
                    notes.iter().map(|n| n.id.clone()).collect();
                // Forget/correct/disable must not leave an inferred reminder behind.
                let mut j = state.initiative.journal.lock().await;
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
                        },
                        time,
                    )?;
                }
                if state.memory.learning_ticket() != Some(ticket) {
                    return Ok::<_, String>(());
                }
                if updated != *j {
                    state.initiative.save(&updated).await?;
                    *j = updated;
                }
                Ok(())
            }
            .await;
            if let Err(e) = result {
                *state.initiative.error.lock().await = Some(e);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
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
            })
            .await
            .unwrap();
        let p = notdot_core::initiative::Policy {
            behaviour: Behaviour::Proactive,
            quiet_start: 0,
            quiet_end: 0,
            ..Default::default()
        };
        assert_eq!(restored.status(&p).await.available, 1);
        assert!(restored.status(&p).await.announce.is_none());
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
