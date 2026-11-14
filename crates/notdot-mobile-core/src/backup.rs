//! The desktop's full-context (Whisper) backup wake decoder, for phones.
//!
//! Same model, decode rules (`notdot_wake_runtime::engine`) and utterance
//! cutting (`notdot_wake_kws::Utterances`) as the desktop's generic decoder.
//! Audio stays in memory, is never written or sent, and is dropped after
//! one decode.
//!
//! Bounds:
//! - one decode at a time on one worker thread, at most two decoder threads;
//! - one queued utterance (six seconds at most), so memory does not grow;
//! - three seconds per decode, after which it is aborted;
//! - `reset()` and drop cancel the decode in progress through an epoch
//!   checked by Whisper's abort callback, so neither waits for a full decode.
//!
//! The model loads and warms up (on synthetic silence) on the worker thread,
//! so creating the engine does not block the caller; the keyword spotter
//! works meanwhile.
pub use crate::backup_types::{BackupState, BackupStats};
use notdot_wake_kws::Utterances;
use notdot_wake_runtime::engine::{self, DecodeLimits};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use whisper_rs::{WhisperContext, WhisperContextParameters};

/// File name of the backup model, as bundled for the desktop.
pub const MODEL: &str = "ggml-tiny.en-q5_1.bin";

pub const BUDGET: Duration = Duration::from_secs(3);

struct Job {
    epoch: u64,
    samples: Vec<f32>,
    rate: u32,
}

enum Event {
    Loaded(Result<(), String>),
    Decoded {
        epoch: u64,
        result: Result<bool, String>,
        took: Duration,
    },
}

pub struct Backup {
    sender: Option<mpsc::SyncSender<Job>>,
    events: mpsc::Receiver<Event>,
    epoch: Arc<AtomicU64>,
    thread: Option<JoinHandle<()>>,
    utterances: Utterances,
    busy: bool,
    pub state: BackupState,
    pub error: Option<String>,
    pub stats: BackupStats,
}

/// Load the model and run one warm-up decode on synthetic silence (never
/// microphone audio), as the desktop does before using it. Unlike the
/// desktop's `prepare`, the warm-up is cancelled by the engine's epoch, so
/// dropping a new engine does not wait for it.
fn load(
    model: &Path,
    phrase: &str,
    epoch: Arc<AtomicU64>,
    threads: i32,
) -> Result<WhisperContext, String> {
    whisper_rs::install_logging_hooks();
    let mut params = WhisperContextParameters::default();
    params.use_gpu(false);
    let context = WhisperContext::new_with_params(model, params)
        .map_err(|_| "Could not load the backup wake recognizer".to_owned())?;
    let expected = epoch.load(Ordering::Relaxed);
    engine::recognize(
        &context,
        phrase,
        &[0.; 16_000],
        16_000,
        epoch,
        expected,
        DecodeLimits {
            budget: Duration::from_secs(8),
            threads,
        },
    )?;
    Ok(context)
}

/// One decode on the worker: audio, its rate, the shared epoch and the
/// epoch the job belongs to (a mismatch aborts it). True if addressed.
pub(crate) type Decode = Box<dyn FnMut(&[f32], u32, Arc<AtomicU64>, u64) -> Result<bool, String>>;
/// Builds the decoder on the worker thread, since loading takes a while.
pub(crate) type Load = Box<dyn FnOnce(Arc<AtomicU64>) -> Result<Decode, String> + Send>;

/// The desktop's Whisper decoder for `models`.
pub(crate) fn whisper(models: &Path, phrase: &str) -> Load {
    let model: PathBuf = models.join(MODEL);
    let phrase = phrase.to_owned();
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, if cfg!(target_os = "android") { 6 } else { 2 }) as i32;
    Box::new(move |epoch| {
        let context = load(&model, &phrase, epoch, threads)?;
        let limits = DecodeLimits {
            budget: BUDGET,
            threads,
        };
        Ok(Box::new(move |samples: &[f32], rate, epoch, expected| {
            engine::recognize(&context, &phrase, samples, rate, epoch, expected, limits)
        }) as Decode)
    })
}

impl Backup {
    pub fn start(models: &Path, phrase: &str) -> Result<Self, String> {
        Self::start_with(whisper(models, phrase))
    }

    pub(crate) fn start_with(load: Load) -> Result<Self, String> {
        let (sender, jobs) = mpsc::sync_channel::<Job>(1);
        // Bounded: one load event plus at most one decode result per job.
        let (results, events) = mpsc::sync_channel::<Event>(4);
        let epoch = Arc::new(AtomicU64::new(0));
        let worker_epoch = epoch.clone();
        let thread = std::thread::Builder::new()
            .name("notdot-wake-backup".into())
            .spawn(move || {
                let mut decode = match load(worker_epoch.clone()) {
                    Ok(decode) => {
                        let _ = results.send(Event::Loaded(Ok(())));
                        decode
                    }
                    Err(error) => {
                        let _ = results.send(Event::Loaded(Err(error)));
                        return;
                    }
                };
                while let Ok(job) = jobs.recv() {
                    let started = Instant::now();
                    let result = decode(&job.samples, job.rate, worker_epoch.clone(), job.epoch);
                    drop(job.samples);
                    let event = Event::Decoded {
                        epoch: job.epoch,
                        result,
                        took: started.elapsed(),
                    };
                    if results.send(event).is_err() {
                        break;
                    }
                }
            })
            .map_err(|_| "Could not start the backup wake recognizer".to_owned())?;
        Ok(Self {
            sender: Some(sender),
            events,
            epoch,
            thread: Some(thread),
            utterances: Utterances::default(),
            busy: false,
            state: BackupState::Loading,
            error: None,
            stats: BackupStats::default(),
        })
    }

    /// Feed one packet. True when a finished decode heard the wake phrase.
    pub fn accept(&mut self, rate: u32, samples: &[f32]) -> bool {
        let mut heard = false;
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Loaded(Ok(())) => self.state = BackupState::Ready,
                Event::Loaded(Err(error)) => {
                    self.state = BackupState::Unavailable;
                    self.error = Some(error);
                    // Nothing will take work; stop collecting audio.
                    self.sender = None;
                }
                Event::Decoded {
                    epoch,
                    result,
                    took,
                } => {
                    self.busy = false;
                    let ms = u64::try_from(took.as_millis()).unwrap_or(u64::MAX);
                    self.stats.decodes += 1;
                    self.stats.decode_ms_total = self.stats.decode_ms_total.saturating_add(ms);
                    self.stats.decode_ms_max = self.stats.decode_ms_max.max(ms);
                    if epoch != self.epoch.load(Ordering::Relaxed) {
                        continue;
                    }
                    match result {
                        Ok(true) => {
                            self.stats.wakes += 1;
                            self.error = None;
                            heard = true;
                        }
                        Ok(false) => self.error = None,
                        Err(error) => {
                            self.stats.failures += 1;
                            self.error = Some(error);
                            self.utterances.clear();
                        }
                    }
                }
            }
        }
        if heard {
            return true;
        }
        self.stats.dropped = self.utterances.dropped();
        if self.state != BackupState::Ready || self.sender.is_none() {
            return false;
        }
        if let Some(samples) = self.utterances.push(rate, samples, !self.busy) {
            let job = Job {
                epoch: self.epoch.load(Ordering::Relaxed),
                samples,
                rate,
            };
            self.busy = self
                .sender
                .as_ref()
                .is_some_and(|sender| sender.try_send(job).is_ok());
        }
        false
    }

    /// Cancel any decode in progress and forget collected audio.
    pub fn reset(&mut self) {
        self.epoch.fetch_add(1, Ordering::Relaxed);
        self.utterances.reset();
    }

    /// Whether a decode is running.
    pub fn is_busy(&self) -> bool {
        self.busy
    }
}

impl Drop for Backup {
    fn drop(&mut self) {
        // Abort the current decode, end the worker loop, and wait for it so
        // the model's memory is released before the engine is gone.
        self.epoch.fetch_add(1, Ordering::Relaxed);
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Test decoders: a delay in front of any decoder (a slow phone), honouring
/// cancellation as Whisper's abort callback does.
#[cfg(test)]
pub(crate) fn slowed(load: Load, delay: Duration) -> Load {
    Box::new(move |epoch| {
        let mut decode = load(epoch)?;
        Ok(Box::new(
            move |samples: &[f32], rate, epoch: Arc<AtomicU64>, expected| {
                let until = Instant::now() + delay;
                while Instant::now() < until {
                    if epoch.load(Ordering::Relaxed) != expected {
                        return Err("Wake recognition was cancelled".into());
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                decode(samples, rate, epoch, expected)
            },
        ) as Decode)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A decoder that takes `delay`, records each utterance's length and
    /// answers `heard`, without a model.
    fn fake(delay: Duration, heard: bool, log: Arc<Mutex<Vec<usize>>>) -> Load {
        slowed(
            Box::new(move |_| {
                Ok(Box::new(move |samples: &[f32], _, _, _| {
                    log.lock().unwrap().push(samples.len());
                    Ok(heard)
                }) as Decode)
            }),
            delay,
        )
    }

    const RATE: u32 = 16_000;
    const PACKET: usize = 320;

    fn started(load: Load) -> Backup {
        let mut backup = Backup::start_with(load).unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while backup.state == BackupState::Loading && Instant::now() < until {
            backup.accept(RATE, &[]);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(backup.state, BackupState::Ready);
        for _ in 0..50 {
            assert!(!backup.accept(RATE, &[0.0001; PACKET]));
        }
        backup
    }

    /// `packets` of speech then a 400 ms pause, fed faster than real time.
    fn say(backup: &mut Backup, packets: usize) -> bool {
        let mut heard = false;
        for _ in 0..packets {
            heard |= backup.accept(RATE, &[0.05; PACKET]);
        }
        for _ in 0..20 {
            heard |= backup.accept(RATE, &[0.0001; PACKET]);
        }
        heard
    }

    /// Silence until the decoder is idle with nothing waiting (bounded).
    fn drain(backup: &mut Backup) -> bool {
        let mut heard = false;
        let until = Instant::now() + Duration::from_secs(10);
        while (backup.is_busy() || backup.utterances.waiting() > 0) && Instant::now() < until {
            heard |= backup.accept(RATE, &[0.0001; PACKET]);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!backup.is_busy());
        heard
    }

    /// Overload: eight utterances while the first decodes. Two wait, the
    /// rest (oldest first) are dropped and counted; every decode gets one
    /// utterance of its own, the newest last.
    #[test]
    fn overload_keeps_the_newest_utterances_and_counts_the_rest() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut backup = started(fake(Duration::from_millis(500), false, log.clone()));
        for n in 0..8 {
            assert!(!say(&mut backup, 15 + n));
            assert!(backup.utterances.waiting() <= Utterances::MAX_WAITING);
        }
        assert!(!drain(&mut backup));
        // Lead-in (the 2 quiet packets after the previous pause ended it),
        // speech, and the 350 ms pause (18 packets).
        let len = |n: usize| (2 + 15 + n + 18) * PACKET;
        let decoded = log.lock().unwrap().clone();
        assert_eq!(decoded.len(), 3);
        assert_eq!(decoded[1..], [len(6), len(7)]);
        assert_eq!(backup.stats.dropped, 5);
        assert_eq!(backup.stats.decodes, 3);
    }

    /// A reset cancels the running decode and forgets waiting utterances;
    /// a result for the old epoch never wakes, even from a decoder that
    /// would say yes to everything.
    #[test]
    fn reset_rejects_running_and_waiting_utterances() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut backup = started(fake(Duration::from_millis(500), true, log.clone()));
        assert!(!say(&mut backup, 20));
        assert!(backup.is_busy());
        assert!(!say(&mut backup, 20));
        assert!(!say(&mut backup, 20));
        assert_eq!(backup.utterances.waiting(), 2);
        backup.reset();
        assert!(!drain(&mut backup), "a reset utterance woke");
        assert_eq!(backup.stats.wakes, 0);
        // The cancelled decode never reached the decoder; nothing else ran.
        assert!(log.lock().unwrap().is_empty());
        // Afterwards it works again.
        assert!(!say(&mut backup, 20));
        assert!(drain(&mut backup));
        assert_eq!(backup.stats.wakes, 1);
    }
}
