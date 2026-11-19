//! Voice turns over the link: one spoken request up, a streamed reply down.
//!
//! The phone uploads one utterance (16 kHz mono PCM16, at most
//! [`MAX_VOICE_SECONDS`]) on the call's stream. The desktop transcribes it,
//! runs the turn for the phone's own profile and streams the reply back as
//! numbered [`VoiceEvent`]s: what it heard, reply text, and reply audio in
//! chunks. Every event is kept (bounded by [`MAX_REPLY_BYTES`]) until the
//! turn is evicted, so a phone that loses the connection reconnects and
//! resumes from the first event it has not seen, with nothing repeated or
//! lost. A dropped connection never cancels a turn; only a Cancel frame does.
//!
//! Bounds: one running voice turn per phone, [`KEPT_VOICE_RUNS`] finished
//! turns kept for resumption, reply events capped in bytes, audio chunks
//! under the frame limit.
use crate::protocol::{OpError, RequestId, ServerFrame};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
};
use tokio::sync::watch;

/// The only request audio format: mono PCM16 little-endian at this rate.
pub const VOICE_RATE: u32 = 16_000;
/// Longest request, in seconds.
pub const MAX_VOICE_SECONDS: u32 = 30;
pub const MAX_VOICE_SAMPLES: u32 = VOICE_RATE * MAX_VOICE_SECONDS;
/// Raw bytes per audio chunk, either way (base64 keeps it under a frame).
pub const AUDIO_CHUNK_BYTES: usize = 24 * 1024;
/// Most reply bytes (text and audio) kept for one turn.
pub const MAX_REPLY_BYTES: usize = 8 << 20;
/// Finished voice turns kept per phone so a reconnecting phone can resume.
pub const KEPT_VOICE_RUNS: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum VoiceEvent {
    /// The transcript of the request.
    Heard { text: String },
    /// The next piece of reply text, in order.
    Reply { text: String },
    /// The next piece of reply audio, in order. Pieces of one clip share
    /// `clip`; `last` marks its final piece, so the phone can play each
    /// clip as soon as it is complete.
    Audio {
        clip: u32,
        mime: String,
        #[serde(with = "crate::b64_chunk")]
        data: Vec<u8>,
        last: bool,
    },
}
impl VoiceEvent {
    fn size(&self) -> usize {
        match self {
            Self::Heard { text } | Self::Reply { text } => text.len(),
            Self::Audio { data, mime, .. } => data.len() + mime.len(),
        }
    }
}

/// Split one encoded clip into in-order [`VoiceEvent::Audio`] pieces.
pub fn audio_events(clip: u32, mime: &str, bytes: &[u8]) -> Vec<VoiceEvent> {
    let chunks: Vec<&[u8]> = if bytes.is_empty() {
        vec![&[][..]]
    } else {
        bytes.chunks(AUDIO_CHUNK_BYTES).collect()
    };
    let n = chunks.len();
    chunks
        .into_iter()
        .enumerate()
        .map(|(i, data)| VoiceEvent::Audio {
            clip,
            mime: mime.to_owned(),
            data: data.to_vec(),
            last: i + 1 == n,
        })
        .collect()
}

/// A request's audio, checked against its call.
#[derive(Clone, Debug)]
pub struct VoiceAudio {
    pub rate: u32,
    pub samples: Vec<i16>,
}

pub(crate) struct Run {
    pub(crate) digest: [u8; 32],
    events: Vec<VoiceEvent>,
    bytes: usize,
    outcome: Option<ServerFrame>,
    pub(crate) cancel: watch::Sender<bool>,
    changed: watch::Sender<u64>,
}
impl Run {
    fn bump(&self) {
        self.changed.send_modify(|v| *v += 1);
    }
}
pub(crate) type SharedRun = Arc<Mutex<Run>>;

fn lock(run: &SharedRun) -> std::sync::MutexGuard<'_, Run> {
    run.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn new_run(digest: [u8; 32]) -> (SharedRun, watch::Receiver<bool>) {
    let (cancel, cancelled) = watch::channel(false);
    let (changed, _) = watch::channel(0);
    (
        Arc::new(Mutex::new(Run {
            digest,
            events: Vec::new(),
            bytes: 0,
            outcome: None,
            cancel,
            changed,
        })),
        cancelled,
    )
}

/// A run that has only its final frame (recorded before a restart, or
/// evicted from memory): resuming it yields that frame.
pub(crate) fn finished_run(digest: [u8; 32], frame: ServerFrame) -> SharedRun {
    let (run, _) = new_run(digest);
    lock(&run).outcome = Some(frame);
    run
}

pub(crate) fn finish(run: &SharedRun, frame: ServerFrame) {
    let mut r = lock(run);
    if r.outcome.is_none() {
        r.outcome = Some(frame);
        r.bump();
    }
}

pub(crate) fn is_running(run: &SharedRun) -> bool {
    lock(run).outcome.is_none()
}

/// Where a backend writes a turn's reply. Sending is synchronous and never
/// waits for the phone: events are kept for it, up to [`MAX_REPLY_BYTES`].
#[derive(Clone)]
pub struct VoiceSink(pub(crate) SharedRun);
impl VoiceSink {
    pub fn send(&self, event: VoiceEvent) -> Result<(), OpError> {
        let mut r = lock(&self.0);
        if r.outcome.is_some() {
            return Err(OpError::Interrupted);
        }
        let size = event.size();
        if r.bytes + size > MAX_REPLY_BYTES {
            return Err(OpError::NotAvailable {
                reason: "the reply is too long to send to the phone".into(),
            });
        }
        r.bytes += size;
        r.events.push(event);
        r.bump();
        Ok(())
    }

    /// A sink that keeps events in memory only (tests and simulators).
    pub fn detached() -> (Self, VoiceStream) {
        let (run, _) = new_run([0; 32]);
        (Self(run.clone()), VoiceStream::new(run, RequestId(0), 0))
    }

    /// Mark the turn finished (tests and simulators using [`Self::detached`]).
    pub fn close(&self, frame: ServerFrame) {
        finish(&self.0, frame);
    }
}

/// The reply of one voice turn from event `next`, then its final frame.
pub struct VoiceStream {
    run: SharedRun,
    id: RequestId,
    next: u32,
    changed: watch::Receiver<u64>,
    done: bool,
}
impl VoiceStream {
    pub(crate) fn new(run: SharedRun, id: RequestId, next: u32) -> Self {
        let changed = lock(&run).changed.subscribe();
        Self {
            run,
            id,
            next,
            changed,
            done: false,
        }
    }

    /// The next frame: an event, then the final frame, then `None`.
    pub async fn next(&mut self) -> Option<ServerFrame> {
        if self.done {
            return None;
        }
        loop {
            self.changed.borrow_and_update();
            {
                let r = lock(&self.run);
                if let Some(event) = r.events.get(self.next as usize) {
                    let frame = ServerFrame::Event {
                        id: self.id,
                        seq: self.next,
                        event: event.clone(),
                    };
                    self.next += 1;
                    return Some(frame);
                }
                if let Some(frame) = &r.outcome {
                    self.done = true;
                    return Some(frame.clone());
                }
            }
            if self.changed.changed().await.is_err() {
                // The run was dropped without an outcome.
                self.done = true;
                return Some(ServerFrame::Failed {
                    id: self.id,
                    error: OpError::Interrupted,
                });
            }
        }
    }
}

/// Voice turns of one phone: at most one running, a few finished.
#[derive(Default)]
pub(crate) struct Runs {
    runs: HashMap<RequestId, SharedRun>,
    order: VecDeque<RequestId>,
}
impl Runs {
    pub(crate) fn get(&self, id: &RequestId) -> Option<SharedRun> {
        self.runs.get(id).cloned()
    }
    pub(crate) fn running(&self) -> Option<RequestId> {
        self.runs
            .iter()
            .find(|(_, run)| is_running(run))
            .map(|(id, _)| *id)
    }
    pub(crate) fn insert(&mut self, id: RequestId, run: SharedRun) {
        if self.runs.insert(id, run).is_none() {
            self.order.push_back(id);
        }
        while self.order.len() > KEPT_VOICE_RUNS + 1 {
            // Never evict the running turn (there is at most one).
            let Some(pos) = self
                .order
                .iter()
                .position(|old| self.runs.get(old).is_none_or(|r| !is_running(r)))
            else {
                break;
            };
            if let Some(old) = self.order.remove(pos) {
                self.runs.remove(&old);
            }
        }
    }
    pub(crate) fn cancel_all(&mut self) {
        for run in self.runs.values() {
            let _ = lock(run).cancel.send(true);
        }
        self.runs.clear();
        self.order.clear();
    }
}

/// SHA-256 of request audio as sent: the PCM16 little-endian bytes.
pub fn audio_digest(pcm: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(pcm).into()
}

/// PCM16 little-endian bytes to samples; `None` if the length is odd.
pub fn samples(pcm: &[u8]) -> Option<Vec<i16>> {
    pcm.len().is_multiple_of(2).then(|| {
        pcm.chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect()
    })
}

/// A synthetic voice pipeline (fake transcription, turn and speech) for
/// tests and the desktop simulator. It "hears" the request's length, replies
/// with `sentences` sentences and a WAV clip per sentence, and waits `step`
/// between events so cancellation and reconnection can land mid-reply.
#[derive(Clone, Debug)]
pub struct SyntheticVoice {
    pub sentences: u32,
    pub step: std::time::Duration,
    /// Samples of reply audio per sentence (16 kHz tone).
    pub clip_samples: u32,
}
impl Default for SyntheticVoice {
    fn default() -> Self {
        Self {
            sentences: 3,
            step: std::time::Duration::from_millis(5),
            clip_samples: 8_000,
        }
    }
}
impl SyntheticVoice {
    /// What the fake recogniser hears for this audio, or `None` for silence.
    pub fn transcript(audio: &VoiceAudio) -> Option<String> {
        let peak = audio
            .samples
            .iter()
            .map(|s| s.unsigned_abs())
            .max()
            .unwrap_or(0);
        (peak >= 300).then(|| {
            format!(
                "synthetic request of {} ms",
                audio.samples.len() as u64 * 1000 / u64::from(audio.rate)
            )
        })
    }

    /// The fake reply audio for sentence `i`: a short WAV tone.
    pub fn clip(&self, i: u32) -> Vec<u8> {
        let n = self.clip_samples;
        let mut wav = Vec::with_capacity(44 + 2 * n as usize);
        let data = 2 * n;
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&16_000u32.to_le_bytes());
        wav.extend_from_slice(&32_000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data.to_le_bytes());
        let freq = 220.0 + 110.0 * f64::from(i);
        for k in 0..n {
            let v = (f64::from(k) / 16_000.0 * freq * std::f64::consts::TAU).sin() * 3000.0;
            wav.extend_from_slice(&(v as i16).to_le_bytes());
        }
        wav
    }

    /// Run the fake turn. `Ok(None)` if cancelled first.
    pub async fn run(
        &self,
        audio: VoiceAudio,
        sink: VoiceSink,
        mut cancelled: watch::Receiver<bool>,
    ) -> Result<Option<serde_json::Value>, OpError> {
        let Some(heard) = Self::transcript(&audio) else {
            return Err(OpError::BadRequest {
                reason: "no speech heard".into(),
            });
        };
        let step = self.step;
        let wait = async move |cancelled: &mut watch::Receiver<bool>| {
            tokio::select! {
                _ = tokio::time::sleep(step) => {}
                _ = cancelled.wait_for(|c| *c) => return false,
            }
            !*cancelled.borrow()
        };
        if !wait(&mut cancelled).await {
            return Ok(None);
        }
        sink.send(VoiceEvent::Heard {
            text: heard.clone(),
        })?;
        let mut reply = String::new();
        for i in 0..self.sentences {
            if !wait(&mut cancelled).await {
                return Ok(None);
            }
            let sentence = format!("Synthetic sentence {}.", i + 1);
            sink.send(VoiceEvent::Reply {
                text: sentence.clone(),
            })?;
            if !reply.is_empty() {
                reply.push(' ');
            }
            reply.push_str(&sentence);
            for event in audio_events(i, "audio/wav", &self.clip(i)) {
                if !wait(&mut cancelled).await {
                    return Ok(None);
                }
                sink.send(event)?;
            }
        }
        Ok(Some(serde_json::json!({"heard": heard, "reply": reply})))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_is_split_in_order_under_the_frame_limit() {
        let clip = vec![7u8; AUDIO_CHUNK_BYTES * 2 + 5];
        let events = audio_events(3, "audio/mpeg", &clip);
        assert_eq!(events.len(), 3);
        let mut joined = Vec::new();
        for (i, e) in events.iter().enumerate() {
            let VoiceEvent::Audio {
                clip, data, last, ..
            } = e
            else {
                panic!()
            };
            assert_eq!(*clip, 3);
            assert_eq!(*last, i == 2);
            joined.extend_from_slice(data);
            let frame = ServerFrame::Event {
                id: RequestId(u128::MAX),
                seq: u32::MAX,
                event: e.clone(),
            };
            assert!(serde_json::to_vec(&frame).unwrap().len() < crate::protocol::MAX_FRAME);
        }
        assert_eq!(joined, clip);
        assert_eq!(audio_events(0, "audio/wav", &[]).len(), 1);
    }

    #[tokio::test]
    async fn a_stream_resumes_from_any_event_and_ends_with_the_outcome() {
        let (sink, mut all) = VoiceSink::detached();
        for i in 0..3 {
            sink.send(VoiceEvent::Reply {
                text: format!("{i}"),
            })
            .unwrap();
        }
        let mut late = VoiceStream::new(sink.0.clone(), RequestId(0), 2);
        sink.close(ServerFrame::Cancelled { id: RequestId(0) });
        assert!(sink.send(VoiceEvent::Reply { text: "x".into() }).is_err());
        let mut seqs = Vec::new();
        while let Some(frame) = all.next().await {
            seqs.push(frame);
        }
        assert_eq!(seqs.len(), 4);
        assert!(matches!(seqs[3], ServerFrame::Cancelled { .. }));
        assert!(matches!(
            late.next().await,
            Some(ServerFrame::Event { seq: 2, .. })
        ));
        assert!(matches!(
            late.next().await,
            Some(ServerFrame::Cancelled { .. })
        ));
        assert!(late.next().await.is_none());
    }

    #[test]
    fn reply_bytes_are_bounded() {
        let (sink, _) = VoiceSink::detached();
        let big = vec![0u8; AUDIO_CHUNK_BYTES];
        let mut sent = 0;
        while sink
            .send(VoiceEvent::Audio {
                clip: 0,
                mime: "a".into(),
                data: big.clone(),
                last: false,
            })
            .is_ok()
        {
            sent += 1;
            assert!(sent * AUDIO_CHUNK_BYTES <= MAX_REPLY_BYTES + AUDIO_CHUNK_BYTES);
        }
        assert!(sent >= MAX_REPLY_BYTES / (AUDIO_CHUNK_BYTES + 1));
    }

    #[test]
    fn finished_runs_are_evicted_but_the_running_one_is_kept() {
        let mut runs = Runs::default();
        let (running, _) = new_run([1; 32]);
        runs.insert(RequestId(0), running);
        for i in 1..20u128 {
            runs.insert(
                RequestId(i),
                finished_run([0; 32], ServerFrame::Cancelled { id: RequestId(i) }),
            );
        }
        assert!(runs.get(&RequestId(0)).is_some());
        assert_eq!(runs.running(), Some(RequestId(0)));
        assert!(runs.runs.len() <= KEPT_VOICE_RUNS + 1);
        assert!(runs.get(&RequestId(19)).is_some());
        assert!(runs.get(&RequestId(1)).is_none());
    }
}
