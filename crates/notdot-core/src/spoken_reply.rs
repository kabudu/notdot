//! Only final-answer items are eligible for incremental speech.
use serde::Serialize;
use serde_json::Value;
#[derive(Clone, Default, Serialize)]
pub struct SpokenReply {
    pub turn: String,
    pub text: String,
    pub done: bool,
    pub failed: bool,
    #[serde(skip)]
    item: String,
    #[serde(skip)]
    item_start: usize,
}
impl SpokenReply {
    pub fn start(&mut self, turn: &str) {
        *self = Self {
            turn: turn.into(),
            ..Self::default()
        };
    }
    pub fn immediate(turn: String, text: String) -> Self {
        Self {
            turn,
            text,
            done: true,
            ..Self::default()
        }
    }
    pub fn event(&mut self, event: &Value) {
        let p = &event["params"];
        if p["turnId"].as_str().is_some_and(|id| id != self.turn) {
            return;
        }
        match event["method"].as_str().unwrap_or("") {
            "item/started"
                if p["item"]["type"] == "agentMessage" && p["item"]["phase"] == "final_answer" =>
            {
                if !self.text.is_empty() {
                    self.text.push('\n');
                }
                self.item_start = self.text.len();
                self.item = p["item"]["id"].as_str().unwrap_or("").into();
            }
            "item/agentMessage/delta" if !self.item.is_empty() && p["itemId"] == self.item => {
                if let Some(delta) = p["delta"].as_str() {
                    if self.text.len() + delta.len() <= 64_000 {
                        self.text.push_str(delta);
                    } else {
                        self.failed = true;
                    }
                }
            }
            "item/completed"
                if p["item"]["type"] == "agentMessage" && p["item"]["phase"] == "final_answer" =>
            {
                let item = &p["item"];
                if let Some(text) = item["text"].as_str() {
                    if item["id"] != self.item {
                        self.item_start = self.text.len();
                    }
                    if self.item_start + text.len() <= 64_000 {
                        self.text.truncate(self.item_start);
                        self.text.push_str(text);
                        self.item = item["id"].as_str().unwrap_or("").into();
                    } else {
                        self.failed = true;
                    }
                }
            }
            "turn/completed" if p["turn"]["id"] == self.turn => {
                self.done = true;
                self.failed |= p["turn"]["status"] != "completed";
            }
            _ => {}
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn commentary_and_other_turns_never_enter_spoken_text() {
        let mut s = SpokenReply::default();
        s.start("turn");
        for phase in ["commentary", "final_answer"] {
            s.event(&json!({"method":"item/started","params":{"turnId":"turn","item":{"id":phase,"type":"agentMessage","phase":phase}}}));
            s.event(&json!({"method":"item/agentMessage/delta","params":{"turnId":"other","itemId":phase,"delta":"WRONG"}}));
            s.event(&json!({"method":"item/agentMessage/delta","params":{"turnId":"turn","itemId":phase,"delta":"A complete sentence. "}}));
        }
        assert_eq!(s.text, "A complete sentence. ");
        assert!(!s.done);
        s.event(&json!({"method":"turn/completed","params":{"turn":{"id":"turn","status":"interrupted"}}}));
        assert!(s.failed && s.done);
        s.start("new");
        assert!(s.text.is_empty() && !s.done && !s.failed);
    }
    #[test]
    fn completed_item_replaces_its_own_deltas_without_repeating_them() {
        let mut s = SpokenReply::default();
        s.start("t");
        s.event(&json!({"method":"item/started","params":{"turnId":"t","item":{"id":"i","type":"agentMessage","phase":"final_answer"}}}));
        s.event(&json!({"method":"item/agentMessage/delta","params":{"turnId":"t","itemId":"i","delta":"Hello. "}}));
        s.event(&json!({"method":"item/completed","params":{"turnId":"t","item":{"id":"i","type":"agentMessage","phase":"final_answer","text":"Hello. More."}}}));
        assert_eq!(s.text, "Hello. More.");
        s.event(
            &json!({"method":"turn/completed","params":{"turn":{"id":"t","status":"completed"}}}),
        );
        assert!(s.done && !s.failed);
    }
}
