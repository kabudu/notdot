//! A separate writer runtime for explicitly authorised session actions.
use crate::{
    bridge::Bridge,
    session_control::{Action, Authorised},
};
use serde_json::json;
use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::{Mutex, Notify};
#[derive(Clone, Debug)]
pub struct Completion {
    pub thread_id: String,
    pub turn_id: String,
    pub title: String,
    pub status: String,
    pub summary: String,
}
impl Completion {
    pub fn key(&self) -> String {
        format!("task:{}:{}", self.thread_id, self.turn_id)
    }
    pub fn message(&self) -> String {
        let title: String = self.title.chars().take(64).collect();
        let summary: String = self.summary.chars().take(180).collect();
        match self.status.as_str() {
            "completed" if !summary.is_empty() => {
                format!("Codex finished its reply in {title}. It said: {summary}")
            }
            "completed" => format!(
                "Codex finished its reply in {title}. Ask me to read that chat for the result."
            ),
            "interrupted" => format!(
                "The Codex turn in {title} was interrupted. Some background work may continue."
            ),
            "failed" => {
                format!("The Codex turn in {title} failed. Ask me to read that chat for details.")
            }
            "approval_required" => format!(
                "{title} needs attention in Codex. notdot could not handle an approval request; no approval was granted."
            ),
            _ => format!(
                "I lost track of the task in {title}. Check that chat before sending it again. Nothing was retried."
            ),
        }
    }
}
struct Running {
    turn: Option<String>,
    title: String,
    notify: bool,
    summary: String,
    approval_blocked: bool,
}
pub struct SessionRuntime {
    bridge: Arc<Bridge>,
    healthy: AtomicBool,
    gate: Mutex<()>,
    active: Mutex<HashMap<String, Running>>,
    completions: Mutex<VecDeque<Completion>>,
    finished: Mutex<VecDeque<(String, String)>>,
    releasing: Mutex<HashMap<String, Arc<Notify>>>,
}
impl SessionRuntime {
    pub async fn connect(binary: &Path) -> Result<Arc<Self>, String> {
        let bridge = Arc::new(Bridge::connect_for_session_control(binary).await?);
        let mut events = bridge.subscribe();
        let session = Arc::new(Self {
            bridge,
            healthy: AtomicBool::new(true),
            gate: Mutex::new(()),
            active: Mutex::new(HashMap::new()),
            completions: Mutex::new(VecDeque::new()),
            finished: Mutex::new(VecDeque::new()),
            releasing: Mutex::new(HashMap::new()),
        });
        let weak = Arc::downgrade(&session);
        tokio::spawn(async move {
            loop {
                let event = events.recv().await;
                let Some(session) = weak.upgrade() else { break };
                let Ok(event) = event else {
                    session.lost_state().await;
                    break;
                };
                let tid = event["params"]["threadId"].as_str().unwrap_or("");
                match event["method"].as_str() {
                    Some("turn/started") => {
                        if let Some(turn) = event["params"]["turn"]["id"].as_str() {
                            let mut active = session.active.lock().await;
                            if let Some(slot) = active.get_mut(tid) {
                                slot.turn = Some(turn.into());
                            }
                        }
                    }
                    Some("turn/completed") => {
                        let turn = event["params"]["turn"]["id"].as_str();
                        let mut active = session.active.lock().await;
                        if let Some(turn) = turn
                            && active
                                .get(tid)
                                .is_some_and(|t| t.turn.as_deref() == Some(turn))
                        {
                            let notify = Arc::new(Notify::new());
                            session
                                .releasing
                                .lock()
                                .await
                                .insert(tid.into(), notify.clone());
                            let task = active.remove(tid).expect("matched owned task");
                            if task.notify {
                                let status = if task.approval_blocked {
                                    "approval_required"
                                } else {
                                    event["params"]["turn"]["status"]
                                        .as_str()
                                        .unwrap_or("unknown")
                                };
                                session.completions.lock().await.push_back(Completion {
                                    thread_id: tid.into(),
                                    turn_id: turn.into(),
                                    title: task.title,
                                    status: status.into(),
                                    summary: task.summary,
                                });
                            }
                            // Admission reads active before completions. Publish
                            // the receipt before releasing active so that a
                            // concurrent send cannot undercount pending work.
                            drop(active);
                            let mut finished = session.finished.lock().await;
                            finished.retain(|(id, _)| id != tid);
                            finished.push_back((
                                tid.into(),
                                event["params"]["turn"]["status"]
                                    .as_str()
                                    .unwrap_or("finished")
                                    .into(),
                            ));
                            while finished.len() > 20 {
                                finished.pop_front();
                            }
                            drop(finished);
                            let owner = session.clone();
                            let tid = tid.to_owned();
                            tokio::spawn(async move {
                                let _gate = owner.gate.lock().await;
                                if owner.active.lock().await.contains_key(&tid) {
                                    owner.releasing.lock().await.remove(&tid);
                                    return;
                                }
                                if let Ok(reply) = owner
                                    .bridge
                                    .call("thread/unsubscribe", json!({"threadId":tid}))
                                    .await
                                    && reply["status"] == "notLoaded"
                                {
                                    owner.releasing.lock().await.remove(&tid);
                                    return;
                                }
                                // An unsubscribe acknowledgement is not writer release. Wait
                                // for the lifecycle notification before permitting another resume.
                                let _ = tokio::time::timeout(
                                    std::time::Duration::from_secs(5),
                                    notify.notified(),
                                )
                                .await;
                            });
                        }
                    }
                    Some("item/completed") => {
                        let item = &event["params"]["item"];
                        let mut active = session.active.lock().await;
                        if let Some(task) = active.get_mut(tid)
                            && task
                                .turn
                                .as_deref()
                                .is_some_and(|turn| event["params"]["turnId"] == turn)
                            && item["type"] == "agentMessage"
                            && (item["phase"].is_null() || item["phase"] == "final_answer")
                            && let Some(text) = item["text"].as_str()
                        {
                            task.summary = text
                                .chars()
                                .map(|c| if c.is_control() { ' ' } else { c })
                                .take(300)
                                .collect();
                        }
                    }
                    Some("notdot/unsupportedRequest") => {
                        if let Some(task) = session.active.lock().await.get_mut(tid) {
                            task.approval_blocked = true;
                        }
                    }
                    Some("thread/closed") => {
                        let notify = session.releasing.lock().await.remove(tid);
                        if let Some(notify) = notify {
                            notify.notify_one();
                        } else if session.active.lock().await.contains_key(tid) {
                            session.lost_state().await;
                        }
                    }
                    Some("notdot/disconnected") => {
                        session.lost_state().await;
                        break;
                    }
                    _ => {}
                }
            }
        });
        Ok(session)
    }
    async fn lost_state(&self) {
        self.healthy.store(false, Ordering::SeqCst);
        let mut active = self.active.lock().await;
        let mut completions = self.completions.lock().await;
        for (thread_id, task) in active.drain() {
            if task.notify {
                completions.push_back(Completion {
                    thread_id,
                    turn_id: task.turn.unwrap_or_else(|| "unconfirmed".into()),
                    title: task.title,
                    status: "unknown".into(),
                    summary: String::new(),
                });
            }
        }
    }
    /// Peek until the host durably saves the notice, then acknowledge it. This
    /// prevents a transient disk error from silently dropping a completion.
    pub async fn completion(&self) -> Option<Completion> {
        self.completions.lock().await.front().cloned()
    }
    pub async fn acknowledge_completion(&self, key: &str) {
        let mut queue = self.completions.lock().await;
        if queue.front().is_some_and(|notice| notice.key() == key) {
            queue.pop_front();
        }
    }
    /// Runtime events for host progress reporting. Callers must filter by their owned chat IDs.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<serde_json::Value> {
        self.bridge.subscribe()
    }
    pub async fn execute(&self, authority: Authorised) -> Result<String, String> {
        self.execute_with_notifications(authority, false).await
    }
    pub async fn execute_with_notifications(
        &self,
        authority: Authorised,
        notify_default: bool,
    ) -> Result<String, String> {
        let notify = notify_default || authority.notify_on_completion();
        let plan = authority.plan();
        plan.validate()?;
        if !self.healthy.load(Ordering::SeqCst) {
            return Err("The session-control connection lost its live state. Check the target in Codex before restarting notdot; no action was retried.".into());
        }
        if !authority.is_valid() {
            return Err("Chat action cancelled or expired. Nothing was sent.".into());
        }
        if plan.action == Action::Status {
            return Ok(if self.active.lock().await.contains_key(&plan.thread_id) {
                format!(
                    "The task notdot started in {} is still running.",
                    plan.title
                )
            } else if self.releasing.lock().await.contains_key(&plan.thread_id) {
                format!(
                    "Codex finished its reply in {}. Its runtime is still releasing, or background work is keeping it open.",
                    plan.title
                )
            } else if let Some((_, status)) = self
                .finished
                .lock()
                .await
                .iter()
                .find(|(id, _)| id == &plan.thread_id)
            {
                format!(
                    "The last Codex turn notdot started in {} is {status}. Ask me to read that chat for its answer.",
                    plan.title
                )
            } else {
                format!(
                    "Notdot is not running {}. I can read its saved messages, but cannot confirm the desktop app's live status.",
                    plan.title
                )
            });
        }
        let mut gate = self.gate.lock().await;
        if plan.action == Action::Send && self.releasing.lock().await.contains_key(&plan.thread_id)
        {
            drop(gate);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while self.releasing.lock().await.contains_key(&plan.thread_id)
                && std::time::Instant::now() < deadline
            {
                if !authority.is_valid() {
                    return Err("Chat action cancelled or expired".into());
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            gate = self.gate.lock().await;
        }
        let _gate = gate;
        if !self.healthy.load(Ordering::SeqCst) {
            return Err(
                "The session-control live state is unavailable. No action was retried.".into(),
            );
        }
        if !authority.is_valid() {
            return Err("Chat action cancelled or expired".into());
        }
        if plan.action == Action::Send {
            if self.releasing.lock().await.contains_key(&plan.thread_id) {
                return Err("That chat is still releasing its previous turn or has background work. Check its progress before sending again.".into());
            }
            let active = self.active.lock().await;
            if active.contains_key(&plan.thread_id) {
                return Err(
                    "That chat is already working. Ask to add an instruction instead.".into(),
                );
            }
            let count = active.len();
            drop(active);
            let releasing = self.releasing.lock().await;
            if releasing.contains_key(&plan.thread_id) {
                return Err("That chat is releasing its previous turn. Nothing was sent; ask again once it closes.".into());
            }
            if count + self.completions.lock().await.len() >= 24 {
                return Err("Task reports are waiting to be saved. Review follow-ups before starting more work.".into());
            }
            if count + releasing.len() >= 4 {
                return Err("Four chats are already running. Wait for one to finish.".into());
            }
            drop(releasing);
            let resumed=self.bridge.call("thread/resume",json!({"threadId":plan.thread_id,"excludeTurns":true})).await.map_err(|e|if e.contains("active writer") {"The desktop app or another runtime owns that chat. Release it there before asking notdot to send. Nothing was sent.".into()}else{e})?;
            if resumed["thread"]["id"] != plan.thread_id
                || resumed["thread"]["status"]["type"] == "active"
            {
                let _ = self
                    .bridge
                    .call("thread/unsubscribe", json!({"threadId":plan.thread_id}))
                    .await;
                return Err("That chat is not idle. Nothing was sent.".into());
            }
            if let Some(title) = resumed["thread"]["name"].as_str()
                && title != plan.title
            {
                let _ = self
                    .bridge
                    .call("thread/unsubscribe", json!({"threadId":plan.thread_id}))
                    .await;
                return Err(
                    "That chat's name changed. Find it again before sending. Nothing was sent."
                        .into(),
                );
            }
            if !authority.is_valid() {
                let _ = self
                    .bridge
                    .call("thread/unsubscribe", json!({"threadId":plan.thread_id}))
                    .await;
                return Err("Chat action cancelled before sending. Nothing was sent.".into());
            }
            self.active.lock().await.insert(
                plan.thread_id.clone(),
                Running {
                    turn: None,
                    title: plan.title.clone(),
                    notify,
                    summary: String::new(),
                    approval_blocked: false,
                },
            );
            // Preserve the target's model, working directory and permission profile.
            let started=self.bridge.call("turn/start",json!({"threadId":plan.thread_id,"input":[{"type":"text","text":plan.message}]})).await;
            match started {
                Ok(result) => {
                    let turn = result["turn"]["id"].as_str().ok_or(
                        "The target did not confirm a turn. Check that chat before sending again.",
                    )?;
                    if let Some(slot) = self.active.lock().await.get_mut(&plan.thread_id) {
                        slot.turn = Some(turn.into());
                    }
                    Ok(format!(
                        "Sent to {}. Codex accepted the task.{}",
                        plan.title,
                        if notify {
                            " I'll report when its reply ends, respecting quiet hours."
                        } else {
                            ""
                        }
                    ))
                }
                Err(error) => {
                    if error.contains("timed out") {
                        return Err("Sending was not confirmed. Check the target chat before sending again; notdot will not retry automatically.".into());
                    }
                    self.active.lock().await.remove(&plan.thread_id);
                    let _ = self
                        .bridge
                        .call("thread/unsubscribe", json!({"threadId":plan.thread_id}))
                        .await;
                    Err(error)
                }
            }
        } else {
            let turn=self.active.lock().await.get(&plan.thread_id).and_then(|task| task.turn.clone()).ok_or("Notdot does not own a running task in that chat. It cannot steer or stop a desktop-owned session.")?;
            if !authority.is_valid() {
                return Err("Chat action cancelled or expired".into());
            }
            match plan.action {
                Action::Steer => {
                    if notify && let Some(task) = self.active.lock().await.get_mut(&plan.thread_id)
                    {
                        task.notify = true;
                    }
                    self.bridge.call("turn/steer",json!({"threadId":plan.thread_id,"expectedTurnId":turn,"input":[{"type":"text","text":plan.message}]})).await?;
                    Ok(format!("Added the instruction to {}.", plan.title))
                }
                Action::Stop => {
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
                    let mut delay = 50;
                    loop {
                        if !authority.is_valid() {
                            return Err("Stop request cancelled or expired".into());
                        }
                        if self
                            .active
                            .lock()
                            .await
                            .get(&plan.thread_id)
                            .and_then(|t| t.turn.as_deref())
                            != Some(turn.as_str())
                        {
                            return Ok(format!(
                                "Codex has already ended that turn in {}. No stop was needed.",
                                plan.title
                            ));
                        }
                        match self.bridge.interrupt(&plan.thread_id, &turn).await {
                            Ok(()) => return Ok(format!("Stop requested for {}.", plan.title)),
                            Err(error) if error.contains("no active turn") && std::time::Instant::now() < deadline => {
                                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                                delay = (delay * 2).min(500);
                            }
                            Err(error) if error.contains("no active turn") => return Err("Codex has not accepted the stop yet. Check that chat or ask again; the task was not reported as stopped.".into()),
                            Err(error) => return Err(error),
                        }
                    }
                }
                _ => unreachable!("send and status handled above"),
            }
        }
    }
    pub async fn shutdown(&self) {
        self.bridge.shutdown().await;
    }
}
