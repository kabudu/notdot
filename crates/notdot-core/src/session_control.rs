//! Scoped, single-use authority for changing another Codex chat.
use serde::{Deserialize, Serialize};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    ReadOnly,
    Confirm,
    WhenAsked,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Send,
    Steer,
    Stop,
    Status,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub action: Action,
    pub thread_id: String,
    pub title: String,
    #[serde(default)]
    pub unambiguous: bool,
    #[serde(default)]
    pub message: String,
}
impl Plan {
    pub fn from_tool_event(
        event: &serde_json::Value,
        source_thread: &str,
        source_turn: &str,
    ) -> Result<Option<Self>, String> {
        let item = &event["params"]["item"];
        if event["method"] != "item/completed"
            || event["params"]["threadId"] != source_thread
            || event["params"]["turnId"] != source_turn
            || item["type"] != "mcpToolCall"
            || item["server"] != "notdot_chats"
            || item["tool"] != "prepare_chat_action"
            || item["status"] != "completed"
            || !item["error"].is_null()
        {
            return Ok(None);
        }
        let contents = item["result"]["content"]
            .as_array()
            .ok_or("Chat action returned no result")?;
        for content in contents {
            if let Some(text) = content["text"].as_str().filter(|s| s.len() <= 8192) {
                let value: serde_json::Value =
                    serde_json::from_str(text).map_err(|_| "Invalid prepared chat action")?;
                if value["status"] == "prepared_only_not_executed" {
                    let plan: Self = serde_json::from_value(value["prepared_action"].clone())
                        .map_err(|_| "Invalid chat action")?;
                    plan.validate()?;
                    if item["arguments"]["thread_id"] != plan.thread_id
                        || item["arguments"]["action"]
                            != serde_json::to_value(plan.action).expect("serializable action")
                        || item["arguments"]["message"].as_str().unwrap_or("") != plan.message
                    {
                        return Err(
                            "Prepared chat action does not match the requested operation".into(),
                        );
                    }
                    return Ok(Some(plan));
                }
            }
        }
        Ok(None)
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.thread_id.is_empty()
            || self.thread_id.len() > 100
            || !self
                .thread_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
            || self.title.trim().is_empty()
            || self.title.len() > 160
            || self.title.chars().any(char::is_control)
            || self.message.len() > 1000
            || self.message.chars().any(|c| c.is_control() && c != '\n')
        {
            return Err("Invalid chat action".into());
        }
        if matches!(self.action, Action::Send | Action::Steer) == self.message.trim().is_empty() {
            return Err(
                "A send or instruction needs a message; stop and status must not include one"
                    .into(),
            );
        }
        Ok(())
    }
    pub fn question(&self) -> String {
        let destination = if self.unambiguous {
            self.title.clone()
        } else {
            let suffix: String = self
                .thread_id
                .chars()
                .rev()
                .take(6)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            format!("{}, chat ending {}", self.title, suffix)
        };
        let detail = match self.action {
            Action::Send => format!("Send this to {}: {}", destination, self.message),
            Action::Steer => format!("Add this instruction to {}: {}", destination, self.message),
            Action::Stop => format!("Stop the running task in {}", destination),
            Action::Status => format!("Check {}", destination),
        };
        format!(
            "{detail}. This uses that chat's existing permissions. Say {} to confirm, or no to cancel.",
            self.confirmation_phrase()
        )
    }
    fn confirmation_phrase(&self) -> &'static str {
        match self.action {
            Action::Send => "yes, please send that message",
            Action::Steer => "yes, please add that instruction",
            Action::Stop => "yes, please stop that task",
            Action::Status => "yes, please check that chat",
        }
    }
}
fn words(s: &str) -> String {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}
/// Only exact direct commands can bypass read-back. Conversational requests fall
/// back to confirmation; historical text and a model's paraphrase cannot authorize.
pub fn directly_requested(text: &str, plan: &Plan) -> bool {
    if !plan.unambiguous {
        return false;
    }
    let text = text.trim();
    let title = plan.title.trim();
    let message = plan.message.trim();
    match plan.action {
        Action::Send | Action::Steer => [
            format!("tell {title} to "),
            format!("tell the {title} chat to "),
            format!("send to {title} "),
            format!("send to the {title} chat "),
        ]
        .iter()
        .any(|prefix| {
            text.get(..prefix.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
                && text.get(prefix.len()..) == Some(message)
        }),
        Action::Stop => {
            text.eq_ignore_ascii_case(&format!("stop {title}"))
                || text.eq_ignore_ascii_case(&format!("stop the {title} chat"))
        }
        Action::Status => true,
    }
}
pub enum Decision {
    Ask(String),
    Execute(Authorised),
}
/// Delimited, explicit suffixes only. A mention inside a task, a negation or a
/// model's invented notification flag is not permission to interrupt later.
pub fn completion_request(text: &str) -> (&str, bool) {
    let text = text.trim();
    let candidate = text.trim_end_matches(['.', '!', '?']);
    for separator in [", and ", " and ", ", then ", " then "] {
        for notice in [
            "let me know when it's done",
            "let me know when it’s done",
            "let me know when it is done",
            "let me know when it's finished",
            "let me know when it’s finished",
            "let me know when it is finished",
            "tell me when it's done",
            "tell me when it’s done",
            "tell me when it is done",
            "tell me when it's finished",
            "tell me when it’s finished",
            "tell me when it is finished",
            "tell me when it finishes",
        ] {
            let suffix = format!("{separator}{notice}");
            if candidate.len() > suffix.len() {
                let start = candidate.len() - suffix.len();
                if candidate
                    .get(start..)
                    .is_some_and(|s| s.eq_ignore_ascii_case(&suffix))
                {
                    return (candidate[..start].trim_end(), true);
                }
            }
        }
    }
    (text, false)
}
pub struct Authorised {
    plan: Plan,
    notify: bool,
    valid: Arc<AtomicBool>,
    expires: Instant,
}
impl Authorised {
    pub fn plan(&self) -> &Plan {
        &self.plan
    }
    /// Derived from the current spoken request, never from retrieved chat text.
    pub fn notify_on_completion(&self) -> bool {
        self.notify
    }
    pub fn is_valid(&self) -> bool {
        self.valid.load(Ordering::SeqCst) && Instant::now() < self.expires
    }
}
struct Pending {
    id: String,
    plan: Plan,
    created: Instant,
    offered: bool,
    automatic: bool,
    notify: bool,
}
#[derive(Default)]
pub struct Consent {
    mode: Mode,
    live: bool,
    transcript: String,
    turn: Option<String>,
    proposed: bool,
    pending: Option<Pending>,
    authority: Arc<AtomicBool>,
    issued_id: Option<String>,
}
impl Consent {
    pub fn begin(&mut self, mode: Mode, transcript: &str) {
        self.cancel();
        self.mode = mode;
        self.live = true;
        self.authority = Arc::new(AtomicBool::new(true));
        self.transcript = transcript.chars().take(24000).collect();
        self.proposed = false;
    }
    pub fn bind(&mut self, turn: &str) {
        if self.live && self.turn.is_none() {
            self.turn = Some(turn.into());
        }
    }
    pub fn stage(&mut self, turn: &str, mut plan: Plan) -> Result<(), String> {
        plan.validate()?;
        if self.turn.as_deref() != Some(turn)
            || self.proposed
            || (self.mode == Mode::ReadOnly && plan.action != Action::Status)
        {
            return Err("Chat control is disabled or this request is no longer current".into());
        }
        let (instruction, notify) = completion_request(&self.transcript);
        // Some model calls include the routing command in their message. Strip
        // only a literal, current command addressed to the verified unique chat.
        // Never rewrite a paraphrase, negation, quotation or command punctuation.
        if matches!(plan.action, Action::Send | Action::Steer)
            && plan.unambiguous
            && plan.message.trim() == self.transcript.trim()
        {
            let text = instruction;
            for prefix in [
                format!("Tell {} to ", plan.title),
                format!("Tell the {} chat to ", plan.title),
                format!("Send to {} ", plan.title),
                format!("Send to the {} chat ", plan.title),
            ] {
                if text
                    .get(..prefix.len())
                    .is_some_and(|head| head.eq_ignore_ascii_case(&prefix))
                    && let Some(message) = text.get(prefix.len()..).map(str::trim)
                    && !message.is_empty()
                {
                    plan.message = message.into();
                    break;
                }
            }
        }
        // Only remove the literal notification suffix. Never alter the task's
        // punctuation, casing or internal whitespace to gain automatic consent.
        if notify && matches!(plan.action, Action::Send | Action::Steer) {
            let (message, included) = completion_request(&plan.message);
            if included {
                plan.message = message.to_owned();
            }
        }
        let automatic = plan.action == Action::Status
            || (self.mode == Mode::WhenAsked && directly_requested(instruction, &plan));
        self.proposed = true;
        let notify = notify && matches!(plan.action, Action::Send | Action::Steer);
        self.pending = Some(Pending {
            id: format!("session-{}", uuid::Uuid::new_v4()),
            plan,
            created: Instant::now(),
            offered: false,
            automatic,
            notify,
        });
        self.issued_id = self.pending.as_ref().map(|p| p.id.clone());
        Ok(())
    }
    fn prune(&mut self) {
        if self
            .pending
            .as_ref()
            .is_some_and(|p| p.created.elapsed() > Duration::from_secs(120))
        {
            self.pending = None;
        }
    }
    pub fn waiting(&mut self) -> bool {
        self.prune();
        self.pending.is_some()
    }
    pub fn pending(&mut self) -> Option<String> {
        self.prune();
        self.pending
            .as_ref()
            .filter(|p| !p.offered)
            .map(|p| p.id.clone())
    }
    pub fn question(&mut self, id: &str) -> Result<Decision, String> {
        self.prune();
        let p = self
            .pending
            .as_mut()
            .filter(|p| p.id == id && !p.offered)
            .ok_or("That chat action expired")?;
        if p.automatic {
            let p = self.pending.take().expect("checked proposal");
            return Ok(Decision::Execute(Authorised {
                plan: p.plan,
                notify: p.notify,
                valid: self.authority.clone(),
                expires: p.created + Duration::from_secs(120),
            }));
        }
        p.offered = true;
        p.created = Instant::now();
        let mut question = p.plan.question();
        if p.notify {
            question.push_str(" I'll report when that Codex turn ends, respecting quiet hours.");
        }
        Ok(Decision::Ask(question))
    }
    pub fn answer(&mut self, id: &str, text: &str) -> Result<Option<Authorised>, String> {
        self.prune();
        if !self
            .pending
            .as_ref()
            .is_some_and(|p| p.id == id && p.offered)
        {
            return Err("That chat confirmation expired".into());
        }
        let p = self.pending.take().expect("checked proposal");
        let expected = if p.plan.action == Action::Stop {
            "yes stop it"
        } else {
            "yes send it"
        };
        let answer = words(text);
        let longer = words(p.plan.confirmation_phrase());
        Ok(
            (answer == expected || answer == longer).then_some(Authorised {
                plan: p.plan,
                notify: p.notify,
                valid: self.authority.clone(),
                expires: p.created + Duration::from_secs(120),
            }),
        )
    }
    pub fn cancel_id(&mut self, id: &str) {
        if self.issued_id.as_deref() == Some(id) {
            self.cancel();
        }
    }
    pub fn cancel(&mut self) {
        self.authority.store(false, Ordering::SeqCst);
        self.issued_id = None;
        self.pending = None;
        self.live = false;
        self.turn = None;
        self.transcript.clear();
    }
}

impl Drop for Consent {
    fn drop(&mut self) {
        self.authority.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn plan() -> Plan {
        Plan {
            action: Action::Send,
            thread_id: "chat-one".into(),
            title: "Website".into(),
            unambiguous: true,
            message: "make the button clearer".into(),
        }
    }
    #[test]
    fn a_prepared_action_requires_the_exact_tool_and_source_turn() {
        use serde_json::json;
        let p = plan();
        let mut event = json!({"method":"item/completed","params":{"threadId":"source","turnId":"t","item":{"type":"mcpToolCall","server":"notdot_chats","tool":"prepare_chat_action","status":"completed","arguments":{"action":"send","thread_id":"chat-one","message":p.message},"result":{"content":[{"type":"text","text":json!({"status":"prepared_only_not_executed","prepared_action":p}).to_string()}]}}}});
        assert!(
            Plan::from_tool_event(&event, "source", "t")
                .unwrap()
                .is_some()
        );
        assert!(
            Plan::from_tool_event(&event, "source", "stale")
                .unwrap()
                .is_none()
        );
        event["params"]["item"]["arguments"]["message"] = "altered".into();
        assert!(Plan::from_tool_event(&event, "source", "t").is_err());
        event["params"]["item"]["server"] = "untrusted_server".into();
        assert!(
            Plan::from_tool_event(&event, "source", "t")
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn issued_authority_is_revoked_by_escape_or_a_new_request() {
        let mut c = Consent::default();
        c.begin(Mode::WhenAsked, "Tell Website to make the button clearer");
        c.bind("t");
        c.stage("t", plan()).unwrap();
        let id = c.pending().unwrap();
        let Decision::Execute(authority) = c.question(&id).unwrap() else {
            panic!("direct request should execute")
        };
        assert!(authority.is_valid());
        c.cancel_id(&id);
        assert!(!authority.is_valid());
        c.begin(Mode::WhenAsked, "Tell Website to make the button clearer");
        c.bind("t2");
        c.stage("t2", plan()).unwrap();
        let id = c.pending().unwrap();
        let Decision::Execute(authority) = c.question(&id).unwrap() else {
            panic!("direct request should execute")
        };
        c.begin(Mode::ReadOnly, "new request");
        assert!(!authority.is_valid());
    }
    #[test]
    fn read_only_is_default_and_cannot_stage_a_write() {
        let mut c = Consent::default();
        c.bind("t");
        assert!(c.stage("t", plan()).is_err());
    }
    #[test]
    fn confirmation_is_scoped_and_single_use() {
        let mut c = Consent::default();
        c.begin(Mode::Confirm, "Tell Website to make the button clearer");
        c.bind("t");
        c.stage("t", plan()).unwrap();
        let id = c.pending().unwrap();
        assert!(c.answer(&id, "yes send it").is_err());
        assert!(matches!(c.question(&id).unwrap(), Decision::Ask(_)));
        assert!(c.pending().is_none());
        assert!(
            c.waiting(),
            "offered confirmation must block speaker switches"
        );
        assert!(c.answer(&id, "yes send it").unwrap().is_some());
        assert!(c.answer(&id, "yes send it").is_err());
    }
    #[test]
    fn longer_chat_confirmations_are_exact_and_action_specific() {
        for (action, phrase, legacy) in [
            (
                Action::Send,
                "Yes, please send that message!",
                "yes send it",
            ),
            (
                Action::Steer,
                "Yes, please add that instruction.",
                "yes send it",
            ),
            (Action::Stop, "Yes, please stop that task.", "yes stop it"),
        ] {
            for answer in [
                phrase,
                legacy,
                "yes",
                "yes please do not send that message",
                "yes please send that message no",
                "yes please save that memory",
                if action == Action::Stop {
                    "yes please send that message"
                } else {
                    "yes please stop that task"
                },
            ] {
                let mut consent = Consent::default();
                consent.begin(Mode::Confirm, "Please control this chat");
                consent.bind("turn");
                let mut requested = plan();
                requested.action = action;
                if matches!(action, Action::Stop | Action::Status) {
                    requested.message.clear();
                }
                consent.stage("turn", requested).unwrap();
                let id = consent.pending().unwrap();
                assert!(matches!(consent.question(&id).unwrap(), Decision::Ask(_)));
                let approved = consent.answer(&id, answer).unwrap().is_some();
                assert_eq!(
                    approved,
                    answer == phrase || answer == legacy,
                    "{action:?}: {answer}"
                );
            }
        }
    }
    #[test]
    fn when_asked_requires_the_exact_current_request() {
        let p = plan();
        assert!(directly_requested(
            "Tell Website to make the button clearer",
            &p
        ));
        for s in [
            "Do not tell Website to make the button clearer",
            "What does tell Website to make the button clearer mean?",
            "Tell Website to delete the repository",
            "Read the Website chat",
            "\"Tell Website to make the button clearer\"",
        ] {
            assert!(!directly_requested(s, &p));
        }
        let mut c = Consent::default();
        c.begin(Mode::WhenAsked, "Read Website");
        c.bind("t");
        c.stage("t", p).unwrap();
        let id = c.pending().unwrap();
        assert!(matches!(c.question(&id).unwrap(), Decision::Ask(_)));
        assert!(c.answer(&id, "yes").unwrap().is_none());
    }
    #[test]
    fn routing_words_are_removed_only_from_an_exact_current_direct_command() {
        let mut c = Consent::default();
        let text = "Tell the Website chat to run rm -r ./cache.";
        c.begin(Mode::WhenAsked, text);
        c.bind("t");
        let mut p = plan();
        p.message = text.into();
        c.stage("t", p).unwrap();
        let id = c.pending().unwrap();
        let Decision::Execute(authority) = c.question(&id).unwrap() else {
            panic!("an exact routed instruction should not need a second approval")
        };
        assert_eq!(authority.plan().message, "run rm -r ./cache.");
        for transcript in [
            "Don't tell the Website chat to run rm -r ./cache.",
            "Tell the Website chat to run rm r ./cache.",
            "\"Tell the Website chat to run rm -r ./cache.\"",
        ] {
            c.begin(Mode::WhenAsked, transcript);
            c.bind("t");
            let mut p = plan();
            p.message = text.into();
            c.stage("t", p).unwrap();
            let id = c.pending().unwrap();
            assert!(matches!(c.question(&id).unwrap(), Decision::Ask(_)));
        }
    }

    #[test]
    fn instruction_case_is_preserved_even_when_destination_case_differs() {
        let mut p = plan();
        p.message = "rename Cache to Archive".into();
        assert!(directly_requested(
            "tell WEBSITE to rename Cache to Archive",
            &p
        ));
        assert!(!directly_requested(
            "Tell Website to rename cache to archive",
            &p
        ));
        p.message = "read \"two  spaces.txt\"".into();
        assert!(!directly_requested(
            "Tell Website to read \"two spaces.txt\"",
            &p
        ));
        assert!(directly_requested(
            "Tell Website to read \"two  spaces.txt\"",
            &p
        ));
    }
    #[test]
    fn changing_command_punctuation_never_bypasses_confirmation() {
        let mut p = plan();
        p.message = "run rm r ./cache".into();
        assert!(!directly_requested("Tell Website to run rm -r ./cache", &p));
        p.message = "run rm -r ./cache".into();
        assert!(directly_requested("Tell Website to run rm -r ./cache", &p));
    }
    #[test]
    fn a_non_unique_destination_always_requires_read_back() {
        let mut c = Consent::default();
        c.begin(Mode::WhenAsked, "Tell Website to make the button clearer");
        c.bind("t");
        let mut p = plan();
        p.unambiguous = false;
        c.stage("t", p).unwrap();
        let id = c.pending().unwrap();
        let Decision::Ask(question) = c.question(&id).unwrap() else {
            panic!("ambiguous destination cannot execute automatically")
        };
        assert!(question.contains("chat ending"));
    }
    #[test]
    fn stale_and_repeated_proposals_cannot_execute() {
        let mut c = Consent::default();
        c.begin(Mode::WhenAsked, "Tell Website to make the button clearer");
        c.bind("new");
        assert!(c.stage("old", plan()).is_err());
        c.stage("new", plan()).unwrap();
        assert!(c.stage("new", plan()).is_err());
        let id = c.pending().unwrap();
        c.pending.as_mut().unwrap().created = Instant::now() - Duration::from_secs(121);
        assert!(c.question(&id).is_err());
        c.begin(Mode::Confirm, "new request");
        assert!(c.answer(&id, "yes send it").is_err());
    }
    #[test]
    fn direct_request_executes_once_and_changes_need_fresh_authority() {
        let mut c = Consent::default();
        c.begin(Mode::WhenAsked, "Tell Website to make the button clearer");
        c.bind("t");
        c.stage("t", plan()).unwrap();
        let id = c.pending().unwrap();
        assert!(matches!(c.question(&id).unwrap(), Decision::Execute(_)));
        assert!(c.question(&id).is_err());
        c.cancel();
        assert!(c.stage("t", plan()).is_err());
    }
}

#[cfg(test)]
mod completion_tests {
    use super::*;
    fn stage(text: &str, mode: Mode, message: &str) -> (Consent, String) {
        let mut c = Consent::default();
        c.begin(mode, text);
        c.bind("turn");
        c.stage(
            "turn",
            Plan {
                action: Action::Send,
                thread_id: "target".into(),
                title: "Website".into(),
                unambiguous: true,
                message: message.into(),
            },
        )
        .unwrap();
        let id = c.pending().unwrap();
        (c, id)
    }
    #[test]
    fn explicit_suffix_is_host_verified_and_never_forwarded_as_task_work() {
        for message in [
            "make the button clearer",
            "make the button clearer, and let me know when it's done",
            "Tell Website to make the button clearer, and let me know when it's done",
        ] {
            let (mut c, id) = stage(
                "Tell Website to make the button clearer, and let me know when it's done",
                Mode::WhenAsked,
                message,
            );
            let Decision::Execute(a) = c.question(&id).unwrap() else {
                panic!("exact command needs no extra confirmation")
            };
            assert!(a.notify_on_completion());
            assert_eq!(a.plan().message, "make the button clearer");
        }
    }
    #[test]
    fn a_completion_notice_never_bypasses_action_confirmation_or_read_only() {
        let (mut c, id) = stage(
            "Tell Website to make the button clearer, and let me know when it's done",
            Mode::Confirm,
            "make the button clearer",
        );
        let Decision::Ask(q) = c.question(&id).unwrap() else {
            panic!()
        };
        assert!(q.contains("report"));
        assert!(c.answer(&id, "yes").unwrap().is_none());
        let (mut c, id) = stage(
            "Tell Website to make the button clearer, and let me know when it's done",
            Mode::Confirm,
            "make the button clearer",
        );
        c.question(&id).unwrap();
        assert!(
            c.answer(&id, "yes send it")
                .unwrap()
                .unwrap()
                .notify_on_completion()
        );
        let mut c = Consent::default();
        c.begin(
            Mode::ReadOnly,
            "Tell Website to do it and let me know when it's done",
        );
        c.bind("turn");
        assert!(
            c.stage(
                "turn",
                Plan {
                    action: Action::Send,
                    thread_id: "target".into(),
                    title: "Website".into(),
                    unambiguous: true,
                    message: "do it".into()
                }
            )
            .is_err()
        );
    }
    #[test]
    fn no_notice_from_a_mention_negation_or_invented_model_suffix() {
        for suffix in [
            ", and let me know when it is done.",
            " and tell me when it is finished!",
            " and let me know when it’s done",
        ] {
            let text = format!("Tell Website to make it clearer{suffix}");
            assert_eq!(
                completion_request(&text),
                ("Tell Website to make it clearer", true)
            );
        }

        for text in [
            "Tell Website to write about 'let me know when it is done'",
            "Tell Website to make the button clearer, and don't let me know when it's done",
            "Tell Website to make the button clearer",
        ] {
            assert!(!completion_request(text).1);
        }
        let (mut c, id) = stage(
            "Tell Website to make the button clearer",
            Mode::Confirm,
            "make the button clearer, and let me know when it's done",
        );
        c.question(&id).unwrap();
        assert!(
            !c.answer(&id, "yes send it")
                .unwrap()
                .unwrap()
                .notify_on_completion()
        );
    }
}
