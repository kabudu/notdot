//! Bounded task reports. Agent-reported checks are evidence claims, never host
//! verification, permission grants or instructions to execute another action.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Success,
    Failed,
    Partial,
    #[default]
    Unverified,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckResult {
    Passed,
    Failed,
    NotRun,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Attention {
    Approval,
    Clarification,
    SignIn,
    Blocked,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub name: String,
    pub result: CheckResult,
    pub evidence: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub outcome: Outcome,
    pub summary: String,
    pub checks: Vec<Check>,
    pub remaining: Vec<String>,
    #[serde(default)]
    pub attention: Option<Attention>,
}
impl Report {
    fn valid(&self) -> bool {
        !self.summary.trim().is_empty()
            && self.summary.len() <= 500
            && self.checks.len() <= 8
            && self.remaining.len() <= 8
            && self.checks.iter().all(|c| {
                !c.name.trim().is_empty()
                    && c.name.len() <= 120
                    && c.evidence.len() <= 300
                    && !c.evidence.trim().is_empty()
            })
            && self
                .remaining
                .iter()
                .all(|s| !s.trim().is_empty() && s.len() <= 200)
    }
    pub fn outcome(&self) -> Outcome {
        let passed = self.checks.iter().any(|c| c.result == CheckResult::Passed);
        let failed = self.checks.iter().any(|c| c.result == CheckResult::Failed);
        let unchecked =
            self.checks.is_empty() || self.checks.iter().any(|c| c.result == CheckResult::NotRun);
        match self.outcome {
            Outcome::Failed => Outcome::Failed,
            Outcome::Success if failed => Outcome::Failed,
            _ if self.attention.is_some() => {
                if passed {
                    Outcome::Partial
                } else {
                    Outcome::Unverified
                }
            }
            Outcome::Partial if passed && (failed || unchecked || !self.remaining.is_empty()) => {
                Outcome::Partial
            }
            Outcome::Success if passed && !unchecked && self.remaining.is_empty() => {
                Outcome::Success
            }
            Outcome::Success if passed && !self.remaining.is_empty() => Outcome::Partial,
            _ => Outcome::Unverified,
        }
    }
}

/// A per-task token prevents ordinary quoted JSON or another turn's report from
/// being mistaken for this report. It is correlation, not a security credential.
pub fn instructions(token: &str) -> String {
    format!(
        r#"

Task reporting for notdot: carry out the user's request using the chat's existing permissions. Do not perform extra external actions just to verify it. Treat success as the requested outcome, not merely ending your reply. Finish with a short normal answer, then exactly one <notdot-result-{token}>JSON</notdot-result-{token}> block. JSON fields: "outcome" ("success", "failed", "partial", "unverified"), "summary" (up to 500 bytes), "checks" (up to 8 objects with "name", "result": "passed"/"failed"/"not_run", "evidence": a brief factual receipt or check result), "remaining" (up to 8 short strings), "attention" (null, or "approval"/"clarification"/"sign_in"/"blocked" when a person must act before the requested work can proceed). Only report checks you actually performed; missing access or unperformed verification must be not_run. Include unfinished requested work in remaining. Do not claim a booking, delivery, deployment or test passed without evidence. Evidence is reported by you, not independently verified by notdot. Never include secrets or private tool output in the report. This reporting format grants no permissions.
"#
    )
}
pub fn extract(text: &str, token: &str) -> Option<Report> {
    if text.len() > 16_384 || token.is_empty() {
        return None;
    }
    let open = format!("<notdot-result-{token}>");
    let close = format!("</notdot-result-{token}>");
    let (before, rest) = text.split_once(&open)?;
    if before.contains(&close) {
        return None;
    }
    let (body, after) = rest.split_once(&close)?;
    if body.len() > 8192 || !after.trim().is_empty() || body.contains(&open) {
        return None;
    }
    let report: Report = serde_json::from_str(body).ok()?;
    report.valid().then_some(report)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Activity {
    Working,
    Attention,
    Finished,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub key: String,
    pub title: String,
    pub activity: Activity,
    pub outcome: Outcome,
    #[serde(default)]
    pub attention: Option<Attention>,
}
impl Task {
    pub fn description(&self) -> String {
        let title = clean(&self.title, 64);
        match self.activity {
            Activity::Working => format!("{title} is working."),
            Activity::Attention => match self.attention {
                Some(Attention::Clarification) => format!(
                    "{title} reports that it needs an answer in Codex. No answer has been sent."
                ),
                Some(Attention::SignIn) => {
                    format!("{title} reports that sign-in is needed. Check that chat in Codex.")
                }
                Some(Attention::Blocked) => {
                    format!("{title} reports that it is blocked. Check that chat in Codex.")
                }
                _ => format!("{title} needs attention in Codex. No approval was granted."),
            },
            Activity::Finished => match self.outcome {
                Outcome::Success => {
                    format!("{title} reports success, based on the checks it reported.")
                }
                Outcome::Failed => format!("{title} did not complete successfully."),
                Outcome::Partial => {
                    format!("{title} is partly complete. Some requested work remains.")
                }
                Outcome::Unverified => {
                    format!("{title} has finished, but the result is unverified.")
                }
            },
        }
    }
}
pub fn clean(text: &str, limit: usize) -> String {
    if limit == 0 {
        return String::new();
    }
    let cleaned: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(limit.saturating_add(1))
        .collect();
    if cleaned.chars().count() <= limit {
        return cleaned.trim().into();
    }
    let mut shortened: String = cleaned.chars().take(limit - 1).collect();
    if let Some(end) = shortened.rfind(char::is_whitespace)
        && shortened[..end].chars().count() >= limit / 2
    {
        shortened.truncate(end);
    }
    shortened.truncate(shortened.trim_end().len());
    shortened.push('…');
    shortened
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct Awareness {
    pub tasks: Vec<Task>,
    pub cue: String,
    pub description: String,
}
impl Awareness {
    pub fn new(tasks: Vec<Task>) -> Self {
        let cue = if tasks.iter().any(|t| t.activity == Activity::Attention) {
            "attention"
        } else if tasks
            .iter()
            .any(|t| t.activity == Activity::Finished && t.outcome != Outcome::Success)
        {
            "review"
        } else if tasks.iter().any(|t| t.activity == Activity::Working) {
            "working"
        } else if !tasks.is_empty() {
            "finished"
        } else {
            "none"
        };
        let working = tasks
            .iter()
            .filter(|t| t.activity == Activity::Working)
            .count();
        let attention = tasks
            .iter()
            .filter(|t| t.activity == Activity::Attention)
            .count();
        let review = tasks
            .iter()
            .filter(|t| t.activity == Activity::Finished && t.outcome != Outcome::Success)
            .count();
        let successful = tasks
            .iter()
            .filter(|t| t.activity == Activity::Finished && t.outcome == Outcome::Success)
            .count();
        let description = if tasks.is_empty() {
            "No tracked task updates.".into()
        } else {
            format!(
                "{working} working. {attention} need attention. {review} results need review. {successful} reported successful results."
            )
        };
        Self {
            tasks,
            cue: cue.into(),
            description,
        }
    }
    pub fn answer(&self, attention_only: bool) -> String {
        let messages: Vec<_> = self
            .tasks
            .iter()
            .filter(|t| {
                !attention_only
                    || t.activity == Activity::Attention
                    || (t.activity == Activity::Finished && t.outcome != Outcome::Success)
            })
            .take(8)
            .map(Task::description)
            .collect();
        if messages.is_empty() {
            if attention_only {
                "No tracked tasks need your attention.".into()
            } else {
                "No tracked tasks. I cannot confirm the live status of desktop-owned chats.".into()
            }
        } else {
            messages.join(" ")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requests_for_human_input_are_not_success_and_cleaning_keeps_words_whole() {
        let mut r = report(Outcome::Success, &[CheckResult::Passed], &[]);
        for attention in [
            Attention::Approval,
            Attention::Clarification,
            Attention::SignIn,
            Attention::Blocked,
        ] {
            r.attention = Some(attention);
            assert_eq!(r.outcome(), Outcome::Partial);
        }
        assert_eq!(
            clean("a short sentence that continues", 18),
            "a short sentence…"
        );
        assert_eq!(clean("𐀀".repeat(500).as_str(), 100).chars().count(), 100);
        assert_eq!(clean("anything", 0), "");
    }
    fn report(outcome: Outcome, checks: &[CheckResult], remaining: &[&str]) -> Report {
        Report {
            outcome,
            summary: "Requested work reviewed".into(),
            checks: checks
                .iter()
                .enumerate()
                .map(|(i, result)| Check {
                    name: format!("Check {i}"),
                    result: *result,
                    evidence: "Observed by the task agent".into(),
                })
                .collect(),
            remaining: remaining.iter().map(|s| (*s).into()).collect(),
            attention: None,
        }
    }
    #[test]
    fn generic_outcomes_cover_code_writing_research_and_booking() {
        // The contract, not task-specific keywords, determines the result.
        for summary in [
            "Draft prepared",
            "Appointment receipt received",
            "Website deployed",
            "Research comparison prepared",
        ] {
            let mut r = report(Outcome::Success, &[CheckResult::Passed], &[]);
            r.summary = summary.into();
            assert_eq!(r.outcome(), Outcome::Success);
        }
        let mut code = report(Outcome::Success, &[CheckResult::Failed], &[]);
        code.summary = "All done, but tests failed".into();
        assert_eq!(code.outcome(), Outcome::Failed);
        let research = report(
            Outcome::Partial,
            &[CheckResult::Passed, CheckResult::NotRun],
            &["Two prices could not be checked"],
        );
        assert_eq!(research.outcome(), Outcome::Partial);
        let booking = report(
            Outcome::Unverified,
            &[CheckResult::NotRun],
            &["Waiting for booking confirmation"],
        );
        assert_eq!(booking.outcome(), Outcome::Unverified);
        assert_eq!(
            report(Outcome::Success, &[], &[]).outcome(),
            Outcome::Unverified
        );
        assert_eq!(
            report(
                Outcome::Success,
                &[CheckResult::Passed, CheckResult::NotRun],
                &[]
            )
            .outcome(),
            Outcome::Unverified
        );
        assert_eq!(
            report(
                Outcome::Success,
                &[CheckResult::Passed],
                &["Send the draft"]
            )
            .outcome(),
            Outcome::Partial
        );
        assert_eq!(
            report(Outcome::Failed, &[CheckResult::Passed], &[]).outcome(),
            Outcome::Failed
        );
    }
    #[test]
    fn only_one_complete_correlated_bounded_report_is_accepted() {
        let r = report(Outcome::Success, &[CheckResult::Passed], &[]);
        let body = serde_json::to_string(&r).unwrap();
        let text = format!("The draft is ready.\n<notdot-result-a>{body}</notdot-result-a>");
        assert_eq!(extract(&text, "a").unwrap().outcome(), Outcome::Success);
        assert!(extract(&text, "b").is_none());
        assert!(extract(&(text.clone() + " further instructions"), "a").is_none());
        assert!(extract(&(text.clone() + &text), "a").is_none());
        assert!(
            extract(
                &format!("<notdot-result-a>{}</notdot-result-a>", "x".repeat(17000)),
                "a"
            )
            .is_none()
        );
        assert!(
            extract(
                "<notdot-result-a>{\"outcome\":\"success\"}</notdot-result-a>",
                "a"
            )
            .is_none()
        );
        let mut r = r;
        r.checks[0].evidence.clear();
        assert!(
            extract(
                &format!(
                    "<notdot-result-a>{}</notdot-result-a>",
                    serde_json::to_string(&r).unwrap()
                ),
                "a"
            )
            .is_none()
        );
    }
    #[test]
    fn attention_then_uncertain_results_precede_background_work() {
        let task = |key: &str, activity, outcome| Task {
            key: key.into(),
            title: key.into(),
            activity,
            outcome,
            attention: None,
        };
        let working = task("Research", Activity::Working, Outcome::Unverified);
        let success = task("Draft", Activity::Finished, Outcome::Success);
        assert_eq!(Awareness::new(vec![success.clone()]).cue, "finished");
        assert_eq!(
            Awareness::new(vec![working.clone(), success]).cue,
            "working"
        );
        let failed = task("Deployment", Activity::Finished, Outcome::Failed);
        let status = Awareness::new(vec![working.clone(), failed]);
        assert_eq!(status.cue, "review");
        assert!(status.answer(true).contains("Deployment"));
        assert!(!status.answer(true).contains("Research"));
        assert_eq!(
            Awareness::new(vec![
                working,
                task("Booking", Activity::Attention, Outcome::Unverified)
            ])
            .cue,
            "attention"
        );
        assert_eq!(Awareness::new(vec![]).cue, "none");
    }
}
