//! A bounded, persisted decision journal. Entries can only be surfaced locally;
//! they never grant authority to call tools or perform external actions.
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Behaviour {
    #[default]
    Reactive,
    Suggestions,
    Proactive,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Policy {
    pub behaviour: Behaviour,
    pub quiet_start: u16,
    pub quiet_end: u16,
    pub daily_limit: u8,
    pub gap_minutes: u16,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            behaviour: Behaviour::Reactive,
            quiet_start: 22 * 60,
            quiet_end: 8 * 60,
            daily_limit: 3,
            gap_minutes: 60,
        }
    }
}
impl Policy {
    pub fn valid(&self) -> bool {
        self.quiet_start < 1440
            && self.quiet_end < 1440
            && (1..=10).contains(&self.daily_limit)
            && (15..=1440).contains(&self.gap_minutes)
    }
    pub fn quiet(&self, minute: u16) -> bool {
        if self.quiet_start == self.quiet_end {
            return false;
        }
        if self.quiet_start < self.quiet_end {
            (self.quiet_start..self.quiet_end).contains(&minute)
        } else {
            minute >= self.quiet_start || minute < self.quiet_end
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub key: String,
    pub message: String,
    pub source: String,
    pub evidence: String,
    pub created: u64,
    pub due: u64,
    pub expires: u64,
    pub permitted: bool,
    pub delivered: bool,
    pub dismissed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_origin: Option<crate::chat_tools::ChatPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<crate::task_awareness::Task>,
}
impl Entry {
    /// Check immutable origin again after restart or grant changes. Native
    /// delivery additionally binds this origin to the active profile context.
    pub fn origin_allowed(&self) -> bool {
        self.profile_origin.as_ref().is_none_or(|policy| {
            if let Some(task) = self.source.strip_prefix("task:") {
                task.split_once(':').is_some_and(|(thread, _)| {
                    policy.check(thread, crate::profiles::ChatOp::Read).is_ok()
                })
            } else {
                policy.roster().is_ok()
            }
        })
    }

    /// Task reports are requested by the user separately from inferred check-ins.
    pub fn task_report(&self) -> bool {
        self.source.starts_with("task:") && self.permitted
    }
    /// Successful results settle quietly; attention and review remain until
    /// acknowledgement or journal expiry. Delivery alone is not acknowledgement.
    pub fn task_cue(&self, now: u64) -> Option<&crate::task_awareness::Task> {
        self.task.as_ref().filter(|task| {
            !self.dismissed
                && self.expires > now
                && (task.activity != crate::task_awareness::Activity::Finished
                    || task.outcome != crate::task_awareness::Outcome::Success
                    || now < self.created.saturating_add(8))
        })
    }
}
#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Journal {
    pub entries: Vec<Entry>,
    pub day: String,
    pub delivered_today: u8,
    pub last_delivery: u64,
    pub last_entry: Option<String>,
}
impl Journal {
    pub fn insert(&mut self, entry: Entry, now: u64) -> Result<(), String> {
        if entry.message.trim().is_empty()
            || entry.message.len() > 1000
            || entry.evidence.len() > 4000
            || entry.source.len() > 512
            || entry.key.len() > 256
            || entry.due < now
            || entry.expires <= entry.due
        {
            return Err("Invalid follow-up".into());
        }
        // Retain completed tombstones until expiry so background discovery cannot
        // resurrect a dismissed proposal after a restart.
        self.entries.retain(|e| e.expires > now);
        if self.entries.iter().any(|e| e.key == entry.key) {
            return Ok(());
        }
        if self.entries.len() >= 256 {
            return Err("Review follow-ups before adding more (limit 256)".into());
        }
        self.entries.push(entry);
        Ok(())
    }
    pub fn available(&self, policy: &Policy, now: u64) -> Vec<&Entry> {
        self.entries
            .iter()
            .filter(|e| {
                !e.dismissed
                    && e.origin_allowed()
                    && !e.delivered
                    && e.due <= now
                    && e.expires > now
                    && (policy.behaviour != Behaviour::Reactive || e.task_report())
            })
            .collect()
    }
    pub fn next(&self, policy: &Policy, now: u64, day: &str, minute: u16) -> Option<&Entry> {
        if policy.quiet(minute) {
            return None;
        }
        // Requested task results are acknowledgements, not unsolicited check-ins.
        // Space simultaneous results apart, without the hour-long proactive gap.
        if now >= self.last_delivery.saturating_add(5)
            && let Some(entry) = self
                .available(policy, now)
                .into_iter()
                .filter(|e| e.task_report())
                .min_by_key(|e| e.due)
        {
            return Some(entry);
        }
        if policy.behaviour != Behaviour::Proactive
            || now
                < self
                    .last_delivery
                    .saturating_add(u64::from(policy.gap_minutes) * 60)
            || (self.day == day && self.delivered_today >= policy.daily_limit)
        {
            return None;
        }
        self.available(policy, now)
            .into_iter()
            .filter(|e| e.permitted && !e.task_report())
            .min_by_key(|e| e.due)
    }
    pub fn claim(
        &mut self,
        id: &str,
        policy: &Policy,
        now: u64,
        day: &str,
        minute: u16,
    ) -> Result<String, String> {
        if self
            .next(policy, now, day, minute)
            .is_none_or(|e| e.id != id)
        {
            return Err("Follow-up is no longer eligible".into());
        }
        let e = self
            .entries
            .iter_mut()
            .find(|e| e.id == id)
            .ok_or("Follow-up missing")?;
        e.delivered = true;
        if self.day != day {
            self.day = day.into();
            self.delivered_today = 0;
        }
        if !e.task_report() {
            self.delivered_today += 1;
        }
        self.last_entry = Some(e.id.clone());
        self.last_delivery = now;
        Ok(e.message.clone())
    }
    pub fn respond(&mut self, text: &str, now: u64) -> Option<String> {
        let text = text
            .trim()
            .trim_end_matches(['.', '?', '!'])
            .to_lowercase()
            .replace('’', "'");
        let action = match text.as_str() {
            "why did you suggest that" | "why that suggestion" => "why",
            "later" | "snooze that" | "remind me later" => "snooze",
            "stop asking about this" | "dismiss that suggestion" => "dismiss",
            _ => return None,
        };
        let entry = self.last_entry.as_ref().and_then(|id| {
            self.entries
                .iter()
                .find(|e| &e.id == id && !e.dismissed && e.expires > now && e.origin_allowed())
        });
        let Some(entry) =
            entry.filter(|_| now >= self.last_delivery && now - self.last_delivery <= 120)
        else {
            return Some(
                "Please choose the item in Follow-ups so I know which one you mean.".into(),
            );
        };
        if action == "why" {
            return Some(format!(
                "This update comes from {}. {}",
                if entry.source.starts_with("memory:") {
                    "a detail you allowed notdot to remember"
                } else if entry.task.is_some() {
                    "the task you asked notdot to follow"
                } else {
                    "your reminder request"
                },
                entry.evidence
            ));
        }
        let id = entry.id.clone();
        Some(match self.action(&id, action, now) {
            Ok(()) if action == "snooze" => {
                "Snoozed for one hour. Your quiet hours and announcement settings still apply."
                    .into()
            }
            Ok(()) => "Dismissed. I won’t ask about this saved item again.".into(),
            Err(e) => e,
        })
    }
    pub fn action(&mut self, id: &str, action: &str, now: u64) -> Result<(), String> {
        let e = self
            .entries
            .iter_mut()
            .find(|e| e.id == id && e.expires > now)
            .ok_or("Follow-up expired or missing")?;
        match action {
            "dismiss" => e.dismissed = true,
            "allow" if !e.dismissed => e.permitted = true,
            "snooze" if !e.dismissed => {
                e.due = now + 3600;
                e.expires = e.expires.max(e.due + 86400);
                e.delivered = false;
            }
            _ => return Err("Unsupported follow-up action".into()),
        }
        Ok(())
    }
}
/// Deliberately narrow: relative durations are unambiguous and never model-guessed.
/// Unsupported reminder requests return an error, rather than claiming to schedule.
pub fn reminder(text: &str, now: u64) -> Option<Result<(String, u64), String>> {
    let lower = text.trim().to_lowercase();
    let request = lower.strip_prefix("please ").unwrap_or(&lower);
    if !request.starts_with("remind me ") {
        return None;
    }
    let Some(rest) = request.strip_prefix("remind me in ") else {
        return Some(Err("For reminders, say: remind me in ten minutes to take a break. Use minutes, hours or days.".into()));
    };
    let Some((duration, _)) = rest.split_once(" to ") else {
        return Some(Err(
            "Say what to remind you to do after the duration.".into()
        ));
    };
    let words: Vec<_> = duration.split_whitespace().collect();
    let parse = || -> Result<(String, u64), String> {
        if words.len() != 2 {
            return Err("Use a duration such as ten minutes or two hours.".into());
        }
        let number = words[0]
            .parse::<u64>()
            .ok()
            .or_else(|| match words[0] {
                "a" | "an" | "one" => Some(1),
                "two" => Some(2),
                "three" => Some(3),
                "four" => Some(4),
                "five" => Some(5),
                "six" => Some(6),
                "seven" => Some(7),
                "eight" => Some(8),
                "nine" => Some(9),
                "ten" => Some(10),
                "fifteen" => Some(15),
                "twenty" => Some(20),
                "thirty" => Some(30),
                "sixty" => Some(60),
                _ => None,
            })
            .ok_or("Use a whole number of minutes, hours or days.")?;
        let unit = match words[1].trim_end_matches('s') {
            "minute" => 60,
            "hour" => 3600,
            "day" => 86400,
            _ => return Err("Use minutes, hours or days.".into()),
        };
        let seconds = number
            .checked_mul(unit)
            .filter(|s| (60..=30 * 86400).contains(s))
            .ok_or("Choose a reminder between one minute and thirty days away.")?;
        // Preserve user casing using the byte offset of the final delimiter.
        let offset = text
            .to_lowercase()
            .find(" to ")
            .ok_or("Reminder text missing")?;
        let message = text
            .get(offset + 4..)
            .ok_or("Unsupported reminder text")?
            .trim();
        if message.is_empty() || message.len() > 900 {
            return Err("Keep the reminder under 900 bytes.".into());
        }
        Ok((format!("Reminder: {message}"), now.saturating_add(seconds)))
    };
    Some(parse())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn item(now: u64) -> Entry {
        Entry {
            id: "a".into(),
            key: "a".into(),
            message: "Take a break".into(),
            source: "explicit".into(),
            evidence: "remind me".into(),
            created: now,
            due: now,
            expires: now + 86400,
            permitted: true,
            delivered: false,
            dismissed: false,
            profile_origin: None,
            task: None,
        }
    }
    #[test]
    fn conversational_actions_bind_to_one_recent_delivery_and_survive_restart() {
        let now = 100_000;
        let mut j = Journal::default();
        j.insert(item(now), now).unwrap();
        let p = Policy {
            behaviour: Behaviour::Proactive,
            ..Policy::default()
        };
        assert!(j.respond("later", now).unwrap().contains("choose"));
        j.claim("a", &p, now, "day", 600).unwrap();
        let mut j: Journal = serde_json::from_str(&serde_json::to_string(&j).unwrap()).unwrap();
        assert!(
            j.respond("Why did you suggest that?", now + 1)
                .unwrap()
                .contains("remind me")
        );
        assert!(j.respond("Later", now + 2).unwrap().contains("one hour"));
        assert_eq!(j.entries[0].due, now + 3602);
        assert!(
            j.next(&p, now + 3602, "day", 1380).is_none(),
            "Snooze never bypasses quiet hours"
        );
        assert!(
            j.respond("stop asking about this", now + 3)
                .unwrap()
                .contains("Dismissed")
        );
        j.insert(item(now + 4), now + 4).unwrap();
        assert_eq!(j.entries.len(), 1);
        assert!(j.entries[0].dismissed);
        assert!(j.respond("later", now + 121).unwrap().contains("choose"));
    }

    #[test]
    fn policies_quiet_limits_restart_and_tombstones() {
        let mut j = Journal::default();
        let now = 100_000;
        j.insert(item(now), now).unwrap();
        let mut p = Policy::default();
        assert!(j.available(&p, now).is_empty());
        p.behaviour = Behaviour::Suggestions;
        assert_eq!(j.available(&p, now).len(), 1);
        assert!(j.next(&p, now, "day", 600).is_none());
        p.behaviour = Behaviour::Proactive;
        assert!(j.next(&p, now, "day", 23 * 60).is_none());
        assert!(j.next(&p, now, "day", 7 * 60).is_none());
        assert!(j.claim("a", &p, now, "day", 600).is_ok());
        let mut restored: Journal =
            serde_json::from_str(&serde_json::to_string(&j).unwrap()).unwrap();
        assert!(restored.next(&p, now, "day", 600).is_none());
        restored.action("a", "snooze", now).unwrap();
        assert!(restored.next(&p, now + 3599, "day", 600).is_none());
        restored.action("a", "dismiss", now).unwrap();
        restored.insert(item(now), now).unwrap();
        assert!(restored.available(&p, now + 3601).is_empty());
        assert_eq!(restored.entries.len(), 1);
        assert!(j.next(&p, now - 1, "otherday", 600).is_none());
    }
    #[test]
    fn consent_and_daily_budget_are_independent_of_memory() {
        let now = 100_000;
        let mut j = Journal::default();
        let mut a = item(now);
        a.permitted = false;
        j.insert(a, now).unwrap();
        let p = Policy {
            behaviour: Behaviour::Proactive,
            daily_limit: 1,
            ..Policy::default()
        };
        assert!(j.next(&p, now, "day", 600).is_none());
        j.action("a", "allow", now).unwrap();
        j.claim("a", &p, now, "day", 600).unwrap();
        let mut b = item(now);
        b.id = "b".into();
        b.key = "b".into();
        j.insert(b, now).unwrap();
        assert!(j.next(&p, now + 3600, "day", 600).is_none());
        assert!(j.next(&p, now + 3600, "nextday", 600).is_some());
    }
    #[test]
    fn relative_reminders_reject_ambiguous_or_unbounded_requests() {
        assert_eq!(
            reminder("Remind me in ten minutes to Drink Water", 100)
                .unwrap()
                .unwrap(),
            ("Reminder: Drink Water".into(), 700)
        );
        assert!(reminder("remind me tomorrow to go", 100).unwrap().is_err());
        assert!(
            reminder("remind me in 999999999999999999 hours to go", 100)
                .unwrap()
                .is_err()
        );
        assert!(reminder("Just discussing reminders", 100).is_none());
    }
}

#[cfg(test)]
mod task_report_tests {
    use super::*;
    #[test]
    fn success_cues_expire_while_review_survives_delivery_until_acknowledged() {
        use crate::task_awareness::{Activity, Outcome, Task};
        let mut e = entry("result");
        e.task = Some(Task {
            key: e.key.clone(),
            title: "Draft".into(),
            activity: Activity::Finished,
            outcome: Outcome::Success,
            attention: None,
        });
        assert!(e.task_cue(107).is_some());
        assert!(e.task_cue(108).is_none());
        for outcome in [Outcome::Failed, Outcome::Partial, Outcome::Unverified] {
            e.task.as_mut().unwrap().outcome = outcome;
            e.delivered = true;
            assert!(e.task_cue(200).is_some());
            e.dismissed = true;
            assert!(e.task_cue(200).is_none());
            e.dismissed = false;
        }
        assert!(e.task_cue(e.expires).is_none());
    }
    fn entry(id: &str) -> Entry {
        Entry {
            id: id.into(),
            key: id.into(),
            source: format!("task:{id}"),
            message: "Codex finished its reply.".into(),
            evidence: "exact owned turn".into(),
            created: 100,
            due: 100,
            expires: 10000,
            permitted: true,
            delivered: false,
            dismissed: false,
            profile_origin: None,
            task: None,
        }
    }
    #[test]
    fn requested_reports_respect_quiet_hours_and_deliver_once_in_reactive_mode() {
        let mut journal = Journal::default();
        let policy = Policy::default();
        journal.insert(entry("first"), 100).unwrap();
        assert!(journal.next(&policy, 100, "today", 23 * 60).is_none());
        assert!(journal.next(&policy, 100, "today", 12 * 60).is_some());
        journal
            .claim("first", &policy, 100, "today", 12 * 60)
            .unwrap();
        let saved = serde_json::to_string(&journal).unwrap();
        let restored: Journal = serde_json::from_str(&saved).unwrap();
        assert!(restored.next(&policy, 200, "today", 12 * 60).is_none());
        assert_eq!(restored.delivered_today, 0);
    }
    #[test]
    fn task_reports_do_not_wait_for_the_unsolicited_hourly_gap_or_grant_external_permission() {
        let mut journal = Journal::default();
        let policy = Policy::default();
        journal.insert(entry("first"), 100).unwrap();
        journal.insert(entry("second"), 100).unwrap();
        journal.claim("first", &policy, 100, "today", 720).unwrap();
        assert!(journal.next(&policy, 104, "today", 720).is_none());
        assert_eq!(
            journal.next(&policy, 105, "today", 720).unwrap().id,
            "second"
        );
        journal.action("second", "dismiss", 105).unwrap();
        assert!(journal.next(&policy, 200, "today", 720).is_none());
        let mut e = entry("unpermitted");
        e.permitted = false;
        journal.insert(e, 100).unwrap();
        assert!(journal.available(&policy, 200).is_empty());
    }
}
