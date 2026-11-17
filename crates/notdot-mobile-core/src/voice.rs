//! A phone voice turn: capture one spoken request after the wake phrase,
//! send it to the paired computer, and hand the reply back clip by clip.
//!
//! [`RequestCapture`] endpoints the request on the phone: it starts at the
//! first speech, keeps a short lead-in, ends after a pause, and never keeps
//! more than [`MAX_REQUEST_SECONDS`]. Audio stays in memory and is dropped
//! once taken. Sending and receiving live in
//! [`crate::link::PhoneLink::voice_turn`].
use crate::MobileError;
use notdot_wake_kws::{VoiceGate, energy};
use std::{collections::VecDeque, sync::Mutex};

pub const REQUEST_RATE: u32 = notdot_link::voice::VOICE_RATE;
/// Longest request kept (the link accepts up to 30 s).
pub const MAX_REQUEST_SECONDS: u32 = 15;
/// Pause that ends a request.
pub const END_PAUSE_MS: u32 = 800;
/// How long to wait for the request to start after the wake phrase.
pub const START_TIMEOUT_MS: u32 = 6_000;
/// Speech kept from just before the gate opened.
pub const LEAD_IN_MS: u32 = 300;
/// Less speech than this is not a request.
pub const MIN_SPEECH_MS: u32 = 200;
const PACKET: usize = 320;

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum CaptureState {
    /// Waiting for the request to start.
    Waiting,
    /// Collecting the request.
    Speaking,
    /// The request ended at a pause; take it with `take_pcm16`.
    Done,
    /// Nothing was said in time; nothing to send.
    NoSpeech,
    /// The request reached the length limit; take it with `take_pcm16`.
    TooLong,
}

struct Capture {
    gate: VoiceGate,
    /// Linear resampler state for `push_samples`: input rate and the
    /// fractional read position carried between pushes.
    rate: u32,
    phase: f64,
    last: f32,
    pending: Vec<i16>,
    lead: VecDeque<i16>,
    pcm: Vec<i16>,
    speech: usize,
    silence: usize,
    waited: usize,
    state: CaptureState,
}
impl Capture {
    fn new() -> Self {
        Self {
            gate: VoiceGate::default(),
            rate: 0,
            phase: 0.0,
            last: 0.0,
            pending: Vec::new(),
            lead: VecDeque::new(),
            pcm: Vec::new(),
            speech: 0,
            silence: 0,
            waited: 0,
            state: CaptureState::Waiting,
        }
    }
    /// Resample float audio at `rate` to 16 kHz PCM16 (linear; speech only
    /// needs the band the recogniser uses). State carries across calls.
    fn resample(&mut self, samples: &[f32], rate: u32) -> Vec<i16> {
        if rate != self.rate {
            self.rate = rate;
            self.phase = 0.0;
            self.last = 0.0;
        }
        let step = f64::from(rate) / f64::from(REQUEST_RATE);
        let mut out = Vec::with_capacity((samples.len() as f64 / step) as usize + 1);
        // phase is the position of the next output, in input samples, from
        // the sample before this buffer (index -1 holds `last`).
        while self.phase < samples.len() as f64 - 1.0 + 1.0 {
            let i = self.phase.floor();
            let frac = (self.phase - i) as f32;
            let at = |k: f64| -> f32 {
                if k < 0.0 {
                    self.last
                } else {
                    samples[(k as usize).min(samples.len() - 1)]
                }
            };
            let v = at(i - 1.0) * (1.0 - frac) + at(i) * frac;
            out.push((v.clamp(-1.0, 1.0) * 32767.0) as i16);
            self.phase += step;
        }
        self.phase -= samples.len() as f64;
        if let Some(l) = samples.last() {
            self.last = *l;
        }
        out
    }
    fn ms(samples: usize) -> u32 {
        (samples as u64 * 1000 / u64::from(REQUEST_RATE)) as u32
    }
    fn packet(&mut self, packet: &[i16]) {
        let floats: Vec<f32> = packet.iter().map(|s| f32::from(*s) / 32768.0).collect();
        let voiced = self.gate.observe(energy(&floats));
        match self.state {
            CaptureState::Waiting => {
                if voiced {
                    self.pcm.extend(self.lead.drain(..));
                    self.pcm.extend_from_slice(packet);
                    self.speech = packet.len();
                    self.state = CaptureState::Speaking;
                } else {
                    self.lead.extend(packet.iter().copied());
                    let keep = (REQUEST_RATE * LEAD_IN_MS / 1000) as usize;
                    while self.lead.len() > keep {
                        self.lead.pop_front();
                    }
                    self.waited += packet.len();
                    if Self::ms(self.waited) >= START_TIMEOUT_MS {
                        self.lead.clear();
                        self.state = CaptureState::NoSpeech;
                    }
                }
            }
            CaptureState::Speaking => {
                let room = (REQUEST_RATE * MAX_REQUEST_SECONDS) as usize - self.pcm.len();
                self.pcm
                    .extend_from_slice(&packet[..packet.len().min(room)]);
                if voiced {
                    self.speech += packet.len();
                    self.silence = 0;
                } else {
                    self.silence += packet.len();
                }
                if Self::ms(self.silence) >= END_PAUSE_MS {
                    self.state = if Self::ms(self.speech) >= MIN_SPEECH_MS {
                        // Keep a little of the pause, not all of it.
                        let trim = self.silence.saturating_sub((REQUEST_RATE / 5) as usize);
                        self.pcm.truncate(self.pcm.len() - trim.min(self.pcm.len()));
                        CaptureState::Done
                    } else {
                        // A click or breath: wait for the real request.
                        self.pcm.clear();
                        self.speech = 0;
                        self.silence = 0;
                        CaptureState::Waiting
                    };
                } else if self.pcm.len() >= (REQUEST_RATE * MAX_REQUEST_SECONDS) as usize {
                    self.state = CaptureState::TooLong;
                }
            }
            _ => {}
        }
    }
}

/// Endpoints one spoken request (16 kHz mono PCM16, fed in any packet size).
#[derive(uniffi::Object)]
pub struct RequestCapture(Mutex<Capture>);

#[uniffi::export]
impl RequestCapture {
    #[uniffi::constructor]
    pub fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self(Mutex::new(Capture::new())))
    }

    /// Feed little-endian PCM16 at 16 kHz. Audio after the request ended is
    /// ignored.
    pub fn push_pcm16(&self, pcm16: Vec<u8>) -> Result<CaptureState, MobileError> {
        if !pcm16.len().is_multiple_of(2) || pcm16.len() > 2 * REQUEST_RATE as usize * 2 {
            return Err(MobileError::Invalid {
                detail: "push whole PCM16 samples, at most two seconds at a time".into(),
            });
        }
        let mut c = self.0.lock().unwrap_or_else(|e| e.into_inner());
        c.pending.extend(
            pcm16
                .chunks_exact(2)
                .map(|b| i16::from_le_bytes([b[0], b[1]])),
        );
        while c.pending.len() >= PACKET
            && matches!(c.state, CaptureState::Waiting | CaptureState::Speaking)
        {
            let packet: Vec<i16> = c.pending.drain(..PACKET).collect();
            c.packet(&packet);
        }
        if !matches!(c.state, CaptureState::Waiting | CaptureState::Speaking) {
            c.pending.clear();
        }
        Ok(c.state)
    }

    /// Feed float audio at any rate from 8 to 192 kHz (iOS gives the
    /// hardware rate); it is resampled to 16 kHz first.
    pub fn push_samples(&self, samples: Vec<f32>, rate: u32) -> Result<CaptureState, MobileError> {
        if !(8_000..=192_000).contains(&rate) || samples.len() > rate as usize * 2 {
            return Err(MobileError::Invalid {
                detail: "push 8-192 kHz audio, at most two seconds at a time".into(),
            });
        }
        if samples.is_empty() {
            return Ok(self.state());
        }
        let pcm: Vec<u8> = {
            let mut c = self.0.lock().unwrap_or_else(|e| e.into_inner());
            let clean: Vec<f32> = samples
                .iter()
                .map(|s| if s.is_finite() { *s } else { 0.0 })
                .collect();
            c.resample(&clean, rate)
                .iter()
                .flat_map(|s| s.to_le_bytes())
                .collect()
        };
        self.push_pcm16(pcm)
    }

    pub fn state(&self) -> CaptureState {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).state
    }

    /// The finished request as PCM16 bytes (empty unless Done or TooLong),
    /// and start afresh.
    pub fn take_pcm16(&self) -> Vec<u8> {
        let mut c = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let out = if matches!(c.state, CaptureState::Done | CaptureState::TooLong) {
            c.pcm.iter().flat_map(|s| s.to_le_bytes()).collect()
        } else {
            Vec::new()
        };
        *c = Capture::new();
        out
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Synthetic tone or silence as PCM16 bytes (never played).
    pub(crate) fn tone(ms: u32, amplitude: f32) -> Vec<u8> {
        let n = (REQUEST_RATE * ms / 1000) as usize;
        (0..n)
            .flat_map(|k| {
                let v = (k as f32 / REQUEST_RATE as f32 * 250.0 * std::f32::consts::TAU).sin()
                    * amplitude;
                (v as i16).to_le_bytes()
            })
            .collect()
    }

    fn feed(c: &RequestCapture, audio: &[u8]) -> CaptureState {
        let mut state = c.state();
        for chunk in audio.chunks(640) {
            state = c.push_pcm16(chunk.to_vec()).unwrap();
        }
        state
    }

    #[test]
    fn a_request_ends_at_the_pause_with_its_lead_in() {
        let c = RequestCapture::new();
        let quiet = tone(600, 20.0);
        assert_eq!(feed(&c, &quiet), CaptureState::Waiting);
        assert_eq!(feed(&c, &tone(1200, 6000.0)), CaptureState::Speaking);
        assert_eq!(feed(&c, &tone(1000, 20.0)), CaptureState::Done);
        let pcm = c.take_pcm16();
        let ms = pcm.len() as u32 / 2 * 1000 / REQUEST_RATE;
        // Lead-in + speech + a little of the pause, not the whole pause.
        assert!((1400..=1800).contains(&ms), "{ms} ms");
        assert_eq!(c.state(), CaptureState::Waiting);
        assert!(c.take_pcm16().is_empty());
    }

    #[test]
    fn silence_times_out_and_clicks_are_not_requests() {
        let c = RequestCapture::new();
        assert_eq!(feed(&c, &tone(300, 20.0)), CaptureState::Waiting);
        // A 60 ms click followed by a pause is not a request.
        feed(&c, &tone(60, 6000.0));
        assert_eq!(feed(&c, &tone(900, 20.0)), CaptureState::Waiting);
        assert_eq!(
            feed(&c, &tone(START_TIMEOUT_MS, 20.0)),
            CaptureState::NoSpeech
        );
        assert!(c.take_pcm16().is_empty());
    }

    #[test]
    fn float_audio_at_48_khz_is_resampled_to_the_same_request() {
        let c = RequestCapture::new();
        let mk = |ms: u32, amp: f32| -> Vec<f32> {
            (0..48 * ms)
                .map(|k| (k as f32 / 48_000.0 * 250.0 * std::f32::consts::TAU).sin() * amp)
                .collect()
        };
        let mut audio = mk(300, 0.0006);
        audio.extend(mk(1200, 0.2));
        audio.extend(mk(1000, 0.0006));
        let mut state = CaptureState::Waiting;
        for chunk in audio.chunks(1024) {
            state = c.push_samples(chunk.to_vec(), 48_000).unwrap();
        }
        assert_eq!(state, CaptureState::Done);
        let pcm = c.take_pcm16();
        let ms = pcm.len() as u32 / 2 * 1000 / REQUEST_RATE;
        assert!((1400..=1800).contains(&ms), "{ms} ms");
        // About the same peak as the input (0.2 of full scale).
        let peak = pcm
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]).unsigned_abs())
            .max()
            .unwrap();
        assert!((6000..=6700).contains(&peak), "{peak}");
        assert!(c.push_samples(vec![0.0; 10], 4_000).is_err());
    }

    #[test]
    fn requests_are_capped_and_bad_input_is_refused() {
        let c = RequestCapture::new();
        feed(&c, &tone(200, 20.0));
        let state = feed(&c, &tone(MAX_REQUEST_SECONDS * 1000 + 2000, 6000.0));
        assert_eq!(state, CaptureState::TooLong);
        let pcm = c.take_pcm16();
        assert_eq!(pcm.len(), 2 * (REQUEST_RATE * MAX_REQUEST_SECONDS) as usize);
        assert!(c.push_pcm16(vec![0; 3]).is_err());
        assert!(
            c.push_pcm16(vec![0; 4 * REQUEST_RATE as usize + 2])
                .is_err()
        );
    }
}
