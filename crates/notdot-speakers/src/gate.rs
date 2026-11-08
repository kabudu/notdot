//! Pre-upload authorisation gate for one capture.
//!
//! In a locked identity mode, cloud speech recognition must not receive audio
//! until the local speaker decision admits the capture. The gate holds the
//! capture's PCM packets locally (bounded), asks for a decision once enough
//! audio is present, then either releases everything in order or discards it.
//! In open/legacy mode it is a zero-cost passthrough.
use crate::audio;
use zeroize::Zeroize;

/// Upper bound on held audio. Reaching it without a decision fails closed.
pub const HOLD_SECONDS: f32 = 6.0;
/// First identification attempt once this much audio is held.
pub const FIRST_ATTEMPT_SECONDS: f32 = 2.0;
/// Further attempts (after a too-short result) every this many seconds.
pub const RETRY_STEP_SECONDS: f32 = 1.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateMode {
    /// Identification off or open mode where the decision does not affect
    /// upload: forward immediately, exactly as before.
    Passthrough,
    /// Locked mode: hold until admitted.
    Hold,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateState {
    Holding,
    Forwarding,
    Discarded,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Push {
    /// Send these packets onwards now, in order.
    Forward(Vec<Vec<u8>>),
    /// Held locally; nothing leaves the device yet.
    Held,
    /// The capture was refused or exceeded the hold bound; drop it.
    Discarded,
}

pub struct PcmGate {
    rate: u32,
    state: GateState,
    held: Vec<Vec<u8>>,
    held_bytes: usize,
    max_bytes: usize,
    next_attempt_bytes: usize,
    ended: bool,
}
impl Drop for PcmGate {
    fn drop(&mut self) {
        self.wipe();
    }
}

fn bytes_for(rate: u32, seconds: f32) -> usize {
    (rate as f32 * seconds) as usize * 2
}

impl PcmGate {
    pub fn new(rate: u32, mode: GateMode) -> Self {
        let state = match mode {
            GateMode::Passthrough => GateState::Forwarding,
            GateMode::Hold if audio::supported_rate(rate) => GateState::Holding,
            // Unknown formats cannot be identified, so locked mode refuses them.
            GateMode::Hold => GateState::Discarded,
        };
        Self {
            rate,
            state,
            held: Vec::new(),
            held_bytes: 0,
            max_bytes: bytes_for(rate, HOLD_SECONDS),
            next_attempt_bytes: bytes_for(rate, FIRST_ATTEMPT_SECONDS),
            ended: false,
        }
    }
    pub fn state(&self) -> GateState {
        self.state
    }
    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn push(&mut self, pcm: Vec<u8>) -> Push {
        match self.state {
            GateState::Forwarding => Push::Forward(vec![pcm]),
            GateState::Discarded => Push::Discarded,
            GateState::Holding => {
                if self.held_bytes + pcm.len() > self.max_bytes {
                    let mut pcm = pcm;
                    pcm.zeroize();
                    self.discard();
                    return Push::Discarded;
                }
                self.held_bytes += pcm.len();
                self.held.push(pcm);
                Push::Held
            }
        }
    }

    /// The capture ended (silence, manual finish or limit).
    pub fn end(&mut self) {
        self.ended = true;
    }

    /// True when an identification attempt should run now.
    pub fn wants_decision(&self) -> bool {
        self.state == GateState::Holding
            && (self.ended || self.held_bytes >= self.next_attempt_bytes)
    }

    /// Audio for identification (a decoded copy; the caller zeroises it).
    pub fn identification_audio(&self) -> Vec<f32> {
        let mut out = Vec::with_capacity(self.held_bytes / 2);
        for p in &self.held {
            out.extend(audio::decode_s16le(p));
        }
        out
    }

    /// The attempt had too little speech. Wait for more unless the capture has
    /// ended or the hold bound leaves no room, in which case it is discarded.
    /// Returns whether another attempt will be requested.
    pub fn need_more(&mut self) -> bool {
        let next = self.held_bytes + bytes_for(self.rate, RETRY_STEP_SECONDS);
        if self.ended || next > self.max_bytes {
            self.discard();
            return false;
        }
        self.next_attempt_bytes = next;
        true
    }

    /// Admitted: release every held packet in order; later packets pass.
    pub fn admit(&mut self) -> Push {
        if self.state != GateState::Holding {
            return if self.state == GateState::Forwarding {
                Push::Forward(Vec::new())
            } else {
                Push::Discarded
            };
        }
        self.state = GateState::Forwarding;
        self.held_bytes = 0;
        Push::Forward(std::mem::take(&mut self.held))
    }

    /// Refused, cancelled or timed out: wipe held audio; nothing is sent.
    pub fn discard(&mut self) {
        self.state = GateState::Discarded;
        self.wipe();
    }

    fn wipe(&mut self) {
        for p in &mut self.held {
            p.zeroize();
        }
        self.held.clear();
        self.held_bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn packet(rate: u32, value: i16) -> Vec<u8> {
        (0..rate / 10).flat_map(|_| value.to_le_bytes()).collect()
    }
    #[test]
    fn passthrough_forwards_immediately() {
        let mut g = PcmGate::new(16_000, GateMode::Passthrough);
        assert_eq!(g.push(vec![1, 2]), Push::Forward(vec![vec![1, 2]]));
        assert!(!g.wants_decision());
    }
    #[test]
    fn holds_until_admitted_then_releases_in_order() {
        let mut g = PcmGate::new(16_000, GateMode::Hold);
        for i in 0..19 {
            assert_eq!(g.push(packet(16_000, i)), Push::Held);
            assert!(!g.wants_decision());
        }
        g.push(packet(16_000, 19));
        assert!(g.wants_decision());
        assert_eq!(g.identification_audio().len(), 16_000 * 2);
        let Push::Forward(released) = g.admit() else {
            panic!()
        };
        assert_eq!(released.len(), 20);
        assert_eq!(released[3], packet(16_000, 3));
        assert_eq!(g.push(vec![9]), Push::Forward(vec![vec![9]]));
    }
    #[test]
    fn refusal_and_overflow_never_forward() {
        let mut g = PcmGate::new(16_000, GateMode::Hold);
        g.push(packet(16_000, 1));
        g.discard();
        assert_eq!(g.push(packet(16_000, 2)), Push::Discarded);
        assert_eq!(g.admit(), Push::Discarded);
        let mut g = PcmGate::new(48_000, GateMode::Hold);
        for _ in 0..60 {
            assert_eq!(g.push(packet(48_000, 1)), Push::Held);
        }
        assert_eq!(g.push(packet(48_000, 1)), Push::Discarded);
        assert_eq!(g.state(), GateState::Discarded);
        assert!(g.identification_audio().is_empty());
        assert_eq!(
            PcmGate::new(4_000, GateMode::Hold).state(),
            GateState::Discarded
        );
    }
    #[test]
    fn short_speech_waits_then_fails_closed_at_end() {
        let mut g = PcmGate::new(16_000, GateMode::Hold);
        for _ in 0..20 {
            g.push(packet(16_000, 1));
        }
        assert!(g.need_more());
        assert!(!g.wants_decision());
        for _ in 0..15 {
            g.push(packet(16_000, 1));
        }
        assert!(g.wants_decision());
        g.end();
        assert!(!g.need_more());
        assert_eq!(g.state(), GateState::Discarded);
        // A capture that ends early is decided immediately.
        let mut g = PcmGate::new(16_000, GateMode::Hold);
        g.push(packet(16_000, 1));
        g.end();
        assert!(g.wants_decision());
    }
}
