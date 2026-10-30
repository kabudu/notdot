//! Voice-derived facts stay in RAM until the user accepts a scoped read-back.
use notdot_core::memory::{LearningBatch, LearningPlan, Memory};
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};
#[derive(Default)]
pub struct Confirmations(Mutex<State>);
#[derive(Default)]
struct State {
    proposal: Option<Proposal>,
    epoch: u64,
}
struct Proposal {
    id: String,
    plan: LearningPlan,
    batch: LearningBatch,
    created: Instant,
    offered: bool,
}
impl Confirmations {
    pub fn reset(&self) -> u64 {
        let mut state = self.0.lock().expect("memory confirmation");
        state.epoch = state.epoch.wrapping_add(1);
        state.proposal = None;
        state.epoch
    }
    pub fn is_current(&self, epoch: u64) -> bool {
        self.0.lock().expect("memory confirmation").epoch == epoch
    }
    pub fn stage(
        &self,
        plan: LearningPlan,
        batch: LearningBatch,
        epoch: u64,
    ) -> Result<(), String> {
        batch.validate_for(&plan)?;
        if batch.notes.is_empty() {
            return Ok(());
        }
        if batch.notes.iter().map(|n| n.content.len()).sum::<usize>() > 512 {
            return Err(
                "Too many details to read back. Ask me to remember one detail at a time.".into(),
            );
        }
        let mut state = self.0.lock().expect("memory confirmation");
        if state.epoch != epoch {
            return Ok(());
        }
        state.proposal = Some(Proposal {
            id: uuid::Uuid::new_v4().to_string(),
            plan,
            batch,
            created: Instant::now(),
            offered: false,
        });
        Ok(())
    }
    pub fn pending(&self, memory: &Memory) -> Option<String> {
        let mut state = self.0.lock().expect("memory confirmation");
        let slot = &mut state.proposal;
        if slot.as_ref().is_some_and(|p| {
            p.created.elapsed() > Duration::from_secs(120) || !memory.plan_is_current(&p.plan)
        }) {
            slot.take();
        }
        slot.as_ref().filter(|p| !p.offered).map(|p| p.id.clone())
    }
    pub fn waiting(&self, memory: &Memory) -> bool {
        let _ = self.pending(memory);
        self.0
            .lock()
            .expect("memory confirmation")
            .proposal
            .is_some()
    }
    pub fn question(&self, memory: &Memory, id: &str) -> Result<String, String> {
        let _ = self.pending(memory);
        let mut state = self.0.lock().expect("memory confirmation");
        let slot = &mut state.proposal;
        let p = slot
            .as_mut()
            .filter(|p| p.id == id && !p.offered)
            .ok_or("That memory check is no longer available")?;
        p.offered = true;
        p.created = Instant::now();
        let details = p
            .batch
            .notes
            .iter()
            .map(|n| n.content.as_str())
            .collect::<Vec<_>>()
            .join(". ");
        Ok(format!(
            "Please check this memory: {details}. Say yes, remember that to keep it, or no to discard it."
        ))
    }
    pub fn cancel(&self, id: Option<&str>) {
        let mut state = self.0.lock().expect("memory confirmation");
        let slot = &mut state.proposal;
        if id.is_none() || slot.as_ref().is_some_and(|p| Some(p.id.as_str()) == id) {
            slot.take();
        }
    }
    pub async fn answer(&self, memory: &Memory, id: &str, text: &str) -> Result<String, String> {
        let _ = self.pending(memory);
        let p = {
            let mut state = self.0.lock().expect("memory confirmation");
            let slot = &mut state.proposal;
            if !slot.as_ref().is_some_and(|p| p.id == id && p.offered) {
                return Err("That memory check expired. Nothing was saved.".into());
            }
            slot.take().expect("checked confirmation")
        };
        if !affirmative(text) {
            return Ok("Nothing saved. Tell me the corrected detail when you're ready.".into());
        }
        let count = memory.apply_confirmed_learning(&p.plan, p.batch).await?;
        Ok(if count > 0 {
            "Memory saved."
        } else {
            "Nothing saved. Memory is off or the detail is no longer current."
        }
        .into())
    }
}
fn affirmative(text: &str) -> bool {
    let words = text
        .split(|c: char| !c.is_alphabetic())
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ");
    matches!(words.as_str(), "yes save that" | "yes remember that")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ambiguous_or_negative_answers_never_authorise_saving() {
        for text in [
            "yes",
            "maybe",
            "no",
            "yes don't save that",
            "yes save that no",
            "I think yes save that",
        ] {
            assert!(!affirmative(text));
        }
        assert!(affirmative("Yes, save that."));
        assert!(affirmative("Yes, remember that!"));
    }
    async fn prepare(manager: &Confirmations, memory: &Memory, text: &str) -> String {
        let epoch = manager.reset();
        let plan = memory
            .plan(
                memory.learning_ticket().unwrap(),
                Some(notdot_core::memory::Interaction {
                    thread: "voice-proof".into(),
                    turn: uuid::Uuid::new_v4().to_string(),
                    user: text.into(),
                }),
            )
            .await
            .unwrap()
            .unwrap();
        let batch=serde_json::from_value(serde_json::json!({"notes":[{"key":"food_preference","content":text,"evidence":text,"confidence":0.99}],"digest":null})).unwrap();
        manager.stage(plan, batch, epoch).unwrap();
        manager.pending(memory).unwrap()
    }
    #[tokio::test]
    async fn misheard_facts_are_not_persisted_until_scoped_positive_readback() {
        let dir = tempfile::tempdir().unwrap();
        let memory = Memory::open_with_learning(dir.path(), true).unwrap();
        let manager = Confirmations::default();
        let id = prepare(&manager, &memory, "My favourite food is mango.").await;
        assert!(memory.review("").await.unwrap().is_empty());
        assert!(manager.answer(&memory, &id, "yes save that").await.is_err()); // never offered
        assert!(
            manager
                .question(&memory, &id)
                .unwrap()
                .contains("food is mango")
        );
        assert!(
            manager
                .answer(&memory, &id, "No, I said fruit")
                .await
                .unwrap()
                .contains("Nothing saved")
        );
        assert!(memory.review("").await.unwrap().is_empty());
        let id = prepare(&manager, &memory, "My favourite fruit is mango.").await;
        manager.question(&memory, &id).unwrap();
        assert!(
            manager
                .answer(&memory, &id, "Yes, save that.")
                .await
                .unwrap()
                .contains("Memory saved")
        );
        let notes = memory.review("").await.unwrap();
        assert_eq!(notes.len(), 1);
        assert!(notes[0].content.contains("fruit"));
        assert_eq!(notes[0].metadata["origin"], "voice confirmation");
        assert!(!memory.recall("mango", 6).await.unwrap().is_empty());
        assert!(manager.answer(&memory, &id, "yes save that").await.is_err()); // one use
        let id = prepare(
            &manager,
            &memory,
            "I now prefer apples as my favourite fruit.",
        )
        .await;
        manager.question(&memory, &id).unwrap();
        manager
            .answer(&memory, &id, "yes remember that")
            .await
            .unwrap();
        let notes = memory.review("").await.unwrap();
        assert_eq!(notes.len(), 1);
        assert!(notes[0].content.contains("apples"));
    }
    #[tokio::test]
    async fn cancellation_expiry_policy_changes_and_stale_workers_cannot_save() {
        let dir = tempfile::tempdir().unwrap();
        let memory = Memory::open_with_learning(dir.path(), true).unwrap();
        let manager = Confirmations::default();
        let id = prepare(&manager, &memory, "I prefer peppermint tea.").await;
        manager.question(&memory, &id).unwrap();
        manager.cancel(Some(&id));
        assert!(manager.answer(&memory, &id, "yes save that").await.is_err());
        let id = prepare(&manager, &memory, "I prefer peppermint tea.").await;
        manager.question(&memory, &id).unwrap();
        manager.0.lock().unwrap().proposal.as_mut().unwrap().created =
            Instant::now() - Duration::from_secs(121);
        assert!(manager.answer(&memory, &id, "yes save that").await.is_err());
        let id = prepare(&manager, &memory, "I prefer peppermint tea.").await;
        manager.question(&memory, &id).unwrap();
        memory.set_learning_enabled(false).await;
        assert!(manager.answer(&memory, &id, "yes save that").await.is_err());
        memory.set_learning_enabled(true).await;
        let epoch = manager.reset();
        let plan = memory
            .plan(
                memory.learning_ticket().unwrap(),
                Some(notdot_core::memory::Interaction {
                    thread: "old".into(),
                    turn: "old".into(),
                    user: "I prefer peppermint tea.".into(),
                }),
            )
            .await
            .unwrap()
            .unwrap();
        let batch=serde_json::from_value(serde_json::json!({"notes":[{"key":"drink","content":"I prefer peppermint tea.","evidence":"I prefer peppermint tea.","confidence":1}],"digest":null})).unwrap();
        manager.reset();
        manager.stage(plan, batch, epoch).unwrap();
        assert!(manager.pending(&memory).is_none());
        assert!(memory.review("").await.unwrap().is_empty());
    }
}
