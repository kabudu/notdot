//! Bounded cancellation tombstones cover requests cancelled before submission starts.
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};
#[derive(Default)]
pub struct Requests {
    cancelled: VecDeque<(String, Instant)>,
    blocked_until: Option<Instant>,
}
impl Requests {
    fn prune(&mut self, now: Instant) {
        while self
            .cancelled
            .front()
            .is_some_and(|(_, at)| now.duration_since(*at) >= Duration::from_secs(180))
        {
            self.cancelled.pop_front();
        }
        if self.blocked_until.is_some_and(|until| now >= until) {
            self.blocked_until = None;
        }
    }
    pub fn validate(id: &str) -> Result<(), String> {
        if id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_.:".contains(&c))
        {
            Err("Invalid voice request identity".into())
        } else {
            Ok(())
        }
    }
    pub fn cancel(&mut self, id: &str) -> Result<(), String> {
        Self::validate(id)?;
        let now = Instant::now();
        self.prune(now);
        if self.cancelled.iter().any(|(known, _)| known == id) {
            return Ok(());
        }
        if self.cancelled.len() >= 256 {
            self.blocked_until = Some(now + Duration::from_secs(180));
        } else {
            self.cancelled.push_back((id.into(), now));
        }
        Ok(())
    }
    pub fn is_cancelled(&mut self, id: &str) -> bool {
        self.prune(Instant::now());
        self.blocked_until.is_some() || self.cancelled.iter().any(|(known, _)| known == id)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancel_before_submission_is_remembered_without_cancelling_a_new_identity() {
        let mut requests = Requests::default();
        requests.cancel("old").unwrap();
        assert!(requests.is_cancelled("old"));
        assert!(!requests.is_cancelled("new"));
        requests.cancel("old").unwrap();
        assert_eq!(requests.cancelled.len(), 1);
    }
    #[test]
    fn saturation_fails_closed_and_expiry_recovers_without_unbounded_storage() {
        let mut requests = Requests::default();
        for i in 0..257 {
            requests.cancel(&format!("request-{i}")).unwrap();
        }
        assert_eq!(requests.cancelled.len(), 256);
        assert!(requests.is_cancelled("otherwise-new"));
        let future = Instant::now() + Duration::from_secs(181);
        requests.prune(future);
        assert!(requests.cancelled.is_empty());
        assert!(requests.blocked_until.is_none());
        assert!(requests.cancel("bad\nidentity").is_err());
    }
}
