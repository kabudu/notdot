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

const BUDGET: Duration = Duration::from_secs(3);

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

impl Backup {
    pub fn start(models: &Path, phrase: &str) -> Result<Self, String> {
        let model: PathBuf = models.join(MODEL);
        let phrase = phrase.to_owned();
        let threads = std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .clamp(1, 2) as i32;
        let limits = DecodeLimits {
            budget: BUDGET,
            threads,
        };
        let (sender, jobs) = mpsc::sync_channel::<Job>(1);
        // Bounded: one load event plus at most one decode result per job.
        let (results, events) = mpsc::sync_channel::<Event>(4);
        let epoch = Arc::new(AtomicU64::new(0));
        let worker_epoch = epoch.clone();
        let thread = std::thread::Builder::new()
            .name("notdot-wake-backup".into())
            .spawn(move || {
                let context = match load(&model, &phrase, worker_epoch.clone(), threads) {
                    Ok(context) => {
                        let _ = results.send(Event::Loaded(Ok(())));
                        context
                    }
                    Err(error) => {
                        let _ = results.send(Event::Loaded(Err(error)));
                        return;
                    }
                };
                while let Ok(job) = jobs.recv() {
                    let started = Instant::now();
                    let result = engine::recognize(
                        &context,
                        &phrase,
                        &job.samples,
                        job.rate,
                        worker_epoch.clone(),
                        job.epoch,
                        limits,
                    );
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

    /// Whether a decode is running (tests wait for results with it).
    #[cfg(test)]
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
