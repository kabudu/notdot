use notdot_core::{
    bridge::Bridge,
    memory::{self, Interaction, LearningBatch, Memory},
};
use serde::Serialize;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc;
#[derive(Clone, Default, Serialize)]
pub struct Status {
    pub phase: String,
    pub message: String,
    pub last_review: Option<u64>,
    pub retained: usize,
    pub skipped: usize,
    pub confirmation: Option<String>,
}
struct Job {
    ticket: u64,
    voice: bool,
    epoch: u64,
    source: Option<Interaction>,
}
struct Pending {
    job: Job,
    turn: Option<String>,
    completed: Option<(String, bool)>,
}
pub struct Learning {
    pending: Mutex<Option<Pending>>,
    sender: mpsc::Sender<Job>,
    status: Arc<Mutex<Status>>,
    memory: Arc<Memory>,
    selection: tokio::sync::watch::Sender<notdot_core::models::Selection>,
    pub confirmations: Arc<crate::memory_confirmation::Confirmations>,
}
impl Learning {
    pub fn new(memory: Arc<Memory>, cwd: PathBuf, runtime: PathBuf) -> Self {
        let (sender, mut receiver) = mpsc::channel::<Job>(8);
        let status = Arc::new(Mutex::new(Status::default()));
        let (selection, mut selected) =
            tokio::sync::watch::channel(notdot_core::models::Selection::default());
        let confirmations = Arc::new(crate::memory_confirmation::Confirmations::default());
        let proposals = confirmations.clone();
        let report = status.clone();
        let ledger = memory.clone();
        tauri::async_runtime::spawn(async move {
            // Own a separate runtime: memory inference cannot overflow the companion's
            // event stream or change its connected-tool configuration.
            let mut client: Option<Arc<Bridge>> = None;
            while let Some(job) = receiver.recv().await {
                if ledger.learning_ticket() != Some(job.ticket)
                    || (job.voice && !proposals.is_current(job.epoch))
                {
                    continue;
                }
                {
                    let mut s = report.lock().expect("learning status");
                    s.phase = "learning".into();
                    s.message = "Checking this chat for useful details to remember…".into();
                }
                let current_selection = selected.borrow_and_update().clone();
                let result = async {
                    if !job.voice && let Some(source) = &job.source {
                        ledger.remember_explicit(job.ticket, source).await?;
                    }
                    // Explicit requests already have a durable record. Review the
                    // ledger without extracting a paraphrased duplicate from them.
                    let automatic_source = job.source.filter(|i| job.voice || memory::explicit_note(&i.user).is_none());
                    let Some(plan) = ledger.plan(job.ticket, automatic_source).await? else {
                        return Ok((0, false));
                    };
                    if plan.interaction.is_none() && plan.notes.len() < 2 {
                        return Ok((0, false));
                    }
                    let mut policy = ledger.policy();
                    let ticket = job.ticket;
                    let current_proposal=proposals.clone();
                    let cancel = async move {
                        loop {
                            let current = *policy.borrow();
                            if !current.enabled || current.epoch != ticket || (job.voice && !current_proposal.is_current(job.epoch)) {
                                return;
                            }
                            tokio::select! { result=policy.changed()=>{if result.is_err(){return;}}, _=tokio::time::sleep(std::time::Duration::from_millis(200))=>{} }
                        }
                    };
                    let prompt = plan.prompt()?;
                    let run = async {
                        if client.is_none() { client = Some(Arc::new(Bridge::connect(&runtime).await?)); }
                        client.as_ref().expect("learning client initialized").analyze_with_selection(&cwd, &prompt, memory::schema(), std::future::pending(), &current_selection).await
                    };
                    let response = tokio::select! {
                        result = tokio::time::timeout(std::time::Duration::from_secs(90), run) => result.map_err(|_| "Checking memories took too long. Your chat still works.".to_string()).and_then(|r|r),
                        _ = cancel => Err("Saving memories was cancelled".into()),
                        _ = selected.changed() => Err("The model changed while memories were being checked".into())
                    };
                    if response.is_err() && let Some(bridge) = client.take() { bridge.shutdown().await; }
                    let response = response?;
                    let mut batch: LearningBatch = serde_json::from_str(&response)
                        .map_err(|_| "The AI returned notes that could not be saved. Your chat still works.")?;
                    let skipped_summary = batch.discard_invalid_digest(&plan);
                    if job.voice && !batch.notes.is_empty() {
                        if ledger.plan_is_current(&plan) { proposals.stage(plan,batch,job.epoch)?; }
                        Ok((0, skipped_summary))
                    } else {
                        ledger.apply_learning(&plan, batch).await.map(|count| (count, skipped_summary))
                    }
                }
                .await;
                if job.voice && !proposals.is_current(job.epoch) {
                    continue;
                }
                let mut s = report.lock().expect("learning status");
                s.last_review = Some(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                );
                match result {
                    Ok((count, skipped_summary)) => {
                        s.phase = "idle".into();
                        s.retained += count;
                        s.message = if count == 0 {
                            "Memory check finished. No new details needed saving.".into()
                        } else {
                            format!("Saved or updated {count} memories and summaries.")
                        };
                        if skipped_summary {
                            s.message
                                .push_str(" A summary could not be verified, so it was not saved.");
                        }
                    }
                    Err(e) => {
                        s.phase = "error".into();
                        s.message = format!("Could not update memories: {e}");
                    }
                }
            }
            if let Some(bridge) = client {
                bridge.shutdown().await;
            }
        });
        Self {
            pending: Mutex::new(None),
            sender,
            status,
            memory,
            selection,
            confirmations,
        }
    }
    pub fn configure(&self, selection: notdot_core::models::Selection) {
        self.selection.send_if_modified(|current| {
            if *current == selection {
                false
            } else {
                *current = selection;
                true
            }
        });
    }
    pub fn status(&self) -> Status {
        let mut s = self.status.lock().expect("learning status").clone();
        if self.memory.learning_ticket().is_none() {
            s.phase = "off".into();
            s.message="Memory is off. Existing memories and chat history are kept, but saved memories are not used for replies.".into();
        } else if s.message.is_empty() {
            s.message="Memory is on. Useful details can be saved after each reply. This uses extra AI requests.".into();
        }
        if s.skipped > 0 {
            s.message.push_str(&format!(
                " {} chats were not checked for memories because notdot was busy.",
                s.skipped
            ));
        }
        s.confirmation = self.confirmations.pending(&self.memory);
        if self.confirmations.waiting(&self.memory) {
            s.message =
                "A new memory needs your spoken confirmation. Nothing has been saved yet.".into();
        }
        s
    }
    pub fn begin_voice(&self, ticket: Option<u64>, thread: String, user: String) {
        self.begin(ticket, thread, user);
        if let Some(p) = self.pending.lock().expect("pending learning").as_mut() {
            p.job.voice = true;
        }
    }
    pub fn begin(&self, ticket: Option<u64>, thread: String, user: String) {
        let epoch = self.confirmations.reset();
        *self.pending.lock().expect("pending learning") = ticket
            .filter(|t| self.memory.learning_ticket() == Some(*t))
            .map(|ticket| Pending {
                job: Job {
                    ticket,
                    voice: false,
                    epoch,
                    source: Some(Interaction {
                        thread,
                        turn: String::new(),
                        user,
                    }),
                },
                turn: None,
                completed: None,
            });
    }
    pub fn cancel(&self) {
        self.confirmations.reset();
        self.pending.lock().expect("pending learning").take();
    }
    pub fn bind(&self, turn: &str) {
        let ready = {
            let mut pending = self.pending.lock().expect("pending learning");
            let Some(p) = pending.as_mut() else {
                return;
            };
            p.turn = Some(turn.into());
            if let Some(source) = p.job.source.as_mut() {
                source.turn = turn.into();
            }
            if p.completed.as_ref().is_some_and(|(id, _)| id == turn) {
                pending.take()
            } else {
                None
            }
        };
        if let Some(p) = ready
            && p.completed.is_some_and(|(_, ok)| ok)
        {
            self.enqueue(p.job);
        }
    }
    pub fn complete(&self, turn: &str, successful: bool) {
        let ready = {
            let mut pending = self.pending.lock().expect("pending learning");
            let Some(p) = pending.as_mut() else {
                return;
            };
            match p.turn.as_deref() {
                Some(id) if id == turn => pending.take(),
                Some(_) => None,
                None => {
                    p.completed = Some((turn.into(), successful));
                    None
                }
            }
        };
        if let Some(p) = ready
            && successful
        {
            self.enqueue(p.job);
        }
    }
    pub fn review(&self) -> Result<(), String> {
        let ticket = self
            .memory
            .learning_ticket()
            .ok_or("Turn on memory before tidying notes")?;
        self.enqueue(Job {
            ticket,
            voice: false,
            epoch: 0,
            source: None,
        });
        Ok(())
    }
    fn enqueue(&self, job: Job) {
        if self.memory.learning_ticket() != Some(job.ticket) {
            return;
        }
        if self.sender.try_send(job).is_err() {
            let mut s = self.status.lock().expect("learning status");
            s.skipped += 1;
            s.message = "notdot is busy. This chat was not added to the memory review.".into();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Learning, mpsc::Receiver<Job>) {
        let dir = tempfile::tempdir().unwrap();
        let memory =
            Arc::new(Memory::open_with_learning(&dir.path().join("memory"), true).unwrap());
        let (sender, receiver) = mpsc::channel(8);
        let learning = Learning {
            confirmations: Arc::new(crate::memory_confirmation::Confirmations::default()),
            pending: Mutex::new(None),
            sender,
            status: Arc::new(Mutex::new(Status::default())),
            memory,
            selection: tokio::sync::watch::channel(notdot_core::models::Selection::default()).0,
        };
        (dir, learning, receiver)
    }
    #[tokio::test]
    async fn completions_before_rpc_reply_are_bound_to_the_correct_turn_and_cancelled_turns_never_learn()
     {
        let (_dir, l, mut rx) = fixture();
        l.begin(
            l.memory.learning_ticket(),
            "thread".into(),
            "I prefer concise answers.".into(),
        );
        l.complete("turn-a", true);
        assert!(rx.try_recv().is_err());
        l.bind("turn-a");
        let job = rx.try_recv().unwrap();
        assert_eq!(job.source.unwrap().turn, "turn-a");
        l.begin(
            l.memory.learning_ticket(),
            "thread".into(),
            "I prefer concise answers.".into(),
        );
        l.bind("turn-b");
        l.complete("stale-turn", true);
        assert!(rx.try_recv().is_err());
        l.complete("turn-b", false);
        assert!(rx.try_recv().is_err());
        l.begin(
            l.memory.learning_ticket(),
            "thread".into(),
            "I prefer concise answers.".into(),
        );
        l.bind("turn-c");
        l.cancel();
        l.complete("turn-c", true);
        assert!(rx.try_recv().is_err());
        let ticket = l.memory.learning_ticket();
        l.memory.set_learning_enabled(false).await;
        l.memory.set_learning_enabled(true).await;
        l.begin(ticket, "thread".into(), "I prefer concise answers.".into());
        l.bind("turn-d");
        l.complete("turn-d", true);
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn queue_is_bounded_and_reports_skipped_interactions() {
        let (_dir, l, mut rx) = fixture();
        for n in 0..9 {
            l.begin(
                l.memory.learning_ticket(),
                "thread".into(),
                "I prefer concise answers.".into(),
            );
            l.bind(&format!("turn-{n}"));
            l.complete(&format!("turn-{n}"), true);
        }
        assert_eq!(l.status().skipped, 1);
        let mut count = 0;
        while rx.try_recv().is_ok() {
            count += 1;
        }
        assert_eq!(count, 8);
    }
    #[tokio::test]
    #[ignore = "explicit authenticated native queue validation with synthetic data"]
    async fn live_completed_interaction_flows_through_worker_to_persisted_notes() {
        let dir = tempfile::tempdir().unwrap();
        let memory =
            Arc::new(Memory::open_with_learning(&dir.path().join("memory"), true).unwrap());
        let runtime = std::env::var_os("NOTDOT_MEMORY_LIVE_RUNTIME").expect("set runtime path");
        let learning = Learning::new(memory.clone(), dir.path().into(), runtime.into());
        learning.begin(
            memory.learning_ticket(),
            "synthetic-native-proof".into(),
            "I prefer concise replies with British English spelling.".into(),
        );
        learning.complete("synthetic-turn", true);
        learning.bind("synthetic-turn");
        tokio::time::timeout(std::time::Duration::from_secs(100), async {
            let mut ticks = tokio::time::interval(std::time::Duration::from_millis(50));
            loop {
                ticks.tick().await;
                if learning.status().last_review.is_some() {
                    break;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(
            learning.status().phase,
            "idle",
            "{}",
            learning.status().message
        );
        let notes = memory.review("").await.unwrap();
        assert!(!notes.is_empty());
        assert!(
            notes
                .iter()
                .all(|n| n.source_id.as_deref() == Some("synthetic-native-proof"))
        );
    }
    #[tokio::test]
    #[ignore = "authenticated voice-memory readback lifecycle, synthetic data only"]
    async fn live_voice_learning_waits_for_readback_before_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let memory =
            Arc::new(Memory::open_with_learning(&dir.path().join("memory"), true).unwrap());
        let runtime = std::env::var_os("NOTDOT_MEMORY_LIVE_RUNTIME").expect("set runtime path");
        let learning = Learning::new(memory.clone(), dir.path().into(), runtime.into());
        learning.begin_voice(
            memory.learning_ticket(),
            "synthetic-voice".into(),
            "Remember that I prefer peppermint tea in the afternoon.".into(),
        );
        learning.bind("voice-turn");
        learning.complete("voice-turn", true);
        tokio::time::timeout(std::time::Duration::from_secs(100), async {
            loop {
                if learning.status().last_review.is_some() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
        let status = learning.status();
        assert_ne!(status.phase, "error", "{}", status.message);
        assert!(memory.review("").await.unwrap().is_empty());
        let id = status
            .confirmation
            .expect("live source should propose a memory");
        learning.confirmations.question(&memory, &id).unwrap();
        learning
            .confirmations
            .answer(&memory, &id, "yes save that")
            .await
            .unwrap();
        let notes = memory.review("").await.unwrap();
        assert!(!notes.is_empty());
        assert!(
            notes
                .iter()
                .all(|n| n.source_id.as_deref() == Some("synthetic-voice")
                    && n.metadata["source_turn"] == "voice-turn")
        );
    }
}
