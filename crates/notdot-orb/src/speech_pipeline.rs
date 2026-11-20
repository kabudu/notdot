//! Ordered, bounded look-ahead for spoken sentences.
//!
//! Sentences are synthesized up to `AHEAD` at a time so a slow request does
//! not leave silence after a short sentence, but they always play in the
//! order they were requested, whatever order the requests complete in. One
//! clip plays at a time; the player owns it, not this queue.
use std::collections::BTreeMap;

/// Clips requested but not yet played: in flight plus ready. Two hides one
/// slow synthesis behind the clip that is playing without bursting requests.
pub(crate) const AHEAD: usize = 2;

pub(crate) struct Pipeline<T> {
    /// Sequence number of the next request.
    requested: u64,
    /// Sequence number of the next clip to play.
    playing: u64,
    in_flight: usize,
    ready: BTreeMap<u64, T>,
}
impl<T> Default for Pipeline<T> {
    fn default() -> Self {
        Self {
            requested: 0,
            playing: 0,
            in_flight: 0,
            ready: BTreeMap::new(),
        }
    }
}
impl<T> Pipeline<T> {
    /// Whether another sentence may be sent for synthesis now.
    pub(crate) fn can_request(&self) -> bool {
        self.in_flight + self.ready.len() < AHEAD
    }
    /// Reserve the next position in playback order.
    pub(crate) fn begin(&mut self) -> u64 {
        let seq = self.requested;
        self.requested += 1;
        self.in_flight += 1;
        seq
    }
    /// A synthesis finished. Returns the clip back if it does not belong to
    /// this pipeline (already completed or never requested), so the caller can
    /// release it.
    pub(crate) fn complete(&mut self, seq: u64, clip: T) -> Result<(), T> {
        if seq < self.playing || seq >= self.requested || self.ready.contains_key(&seq) {
            return Err(clip);
        }
        self.in_flight = self.in_flight.saturating_sub(1);
        self.ready.insert(seq, clip);
        Ok(())
    }
    /// The next clip in order, once it is ready. A later clip never jumps
    /// ahead of an earlier one that is still being synthesized.
    pub(crate) fn take_next(&mut self) -> Option<T> {
        let clip = self.ready.remove(&self.playing)?;
        self.playing += 1;
        Some(clip)
    }
    /// Nothing requested is still waiting to play.
    pub(crate) fn idle(&self) -> bool {
        self.in_flight == 0 && self.ready.is_empty()
    }
    /// Clips that will never play, for release on cancellation.
    /// Results of requests still in flight are refused afterwards.
    pub(crate) fn drain(&mut self) -> Vec<T> {
        self.in_flight = 0;
        self.playing = self.requested;
        std::mem::take(&mut self.ready).into_values().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn out_of_order_synthesis_still_plays_in_request_order() {
        let mut p = Pipeline::default();
        let a = p.begin();
        let b = p.begin();
        assert!(!p.can_request(), "look-ahead is bounded");
        p.complete(b, "second").unwrap();
        assert_eq!(p.take_next(), None, "a later clip never jumps ahead");
        assert!(
            !p.can_request(),
            "a ready clip still counts against the bound"
        );
        p.complete(a, "first").unwrap();
        assert_eq!(p.take_next(), Some("first"));
        assert!(p.can_request());
        let c = p.begin();
        assert_eq!(p.take_next(), Some("second"));
        assert_eq!(p.take_next(), None);
        p.complete(c, "third").unwrap();
        assert_eq!(p.take_next(), Some("third"));
        assert!(p.idle());
    }
    #[test]
    fn duplicate_stale_or_unknown_completions_are_returned_for_release() {
        let mut p = Pipeline::default();
        let a = p.begin();
        p.complete(a, 1).unwrap();
        assert_eq!(p.complete(a, 2), Err(2), "duplicate");
        assert_eq!(p.take_next(), Some(1));
        assert_eq!(p.complete(a, 3), Err(3), "already played");
        assert_eq!(p.complete(9, 4), Err(4), "never requested");
        assert!(p.idle());
    }
    #[test]
    fn cancellation_releases_every_unplayed_clip_and_late_results() {
        let mut p = Pipeline::default();
        let a = p.begin();
        let b = p.begin();
        p.complete(b, "ready").unwrap();
        assert_eq!(p.drain(), vec!["ready"]);
        assert!(p.idle());
        assert_eq!(
            p.complete(a, "late"),
            Err("late"),
            "in flight at cancellation"
        );
        assert!(p.idle());
        // A replacement pipeline rejects the old request's late result.
        let mut fresh = Pipeline::<&str>::default();
        assert_eq!(fresh.complete(a, "late"), Err("late"));
    }
    #[test]
    fn a_long_reply_never_holds_more_than_the_look_ahead() {
        let mut p = Pipeline::default();
        let mut played = Vec::new();
        let mut pending = Vec::new();
        for _ in 0..40 {
            while p.can_request() {
                pending.push(p.begin());
            }
            assert!(pending.len() <= AHEAD);
            // Complete newest first: the worst order for playback.
            while let Some(seq) = pending.pop() {
                p.complete(seq, seq).unwrap();
            }
            while let Some(seq) = p.take_next() {
                played.push(seq);
            }
        }
        assert_eq!(played, (0..played.len() as u64).collect::<Vec<_>>());
    }
}
