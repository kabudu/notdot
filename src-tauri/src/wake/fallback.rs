//! Local, speech-gated recognition for phrases the small keyword model misses.
//! Audio is bounded, ephemeral, and never sent to a voice provider or written.
use super::VoiceGate;
use notdot_wake_runtime::engine::DecodeLimits;
use std::time::{Duration, Instant};
use std::{
    collections::VecDeque,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread::JoinHandle,
};
use whisper_rs::{WhisperContext, WhisperContextParameters};

fn generic_context(path: &Path) -> Result<WhisperContext, String> {
    whisper_rs::install_logging_hooks();
    let mut params = WhisperContextParameters::default();
    params.use_gpu(false);
    WhisperContext::new_with_params(path, params)
        .map_err(|_| "Could not load the bundled local wake recognizer".into())
}
#[cfg(windows)]
struct AcceleratedDecoder {
    runtime: tokio::runtime::Runtime,
    client: Option<notdot_wake_runtime::client::Client>,
    resource_root: std::path::PathBuf,
    model: std::path::PathBuf,
    retry_after: Option<Instant>,
    cleanup_blocked: bool,
}
enum Decoder {
    Generic(WhisperContext),
    Unavailable {
        model: std::path::PathBuf,
        retry_after: Instant,
    },
    #[cfg(windows)]
    Accelerated(Box<AcceleratedDecoder>),
}
impl Decoder {
    fn new(model: &Path, phrase: &str) -> Result<Self, String> {
        #[cfg(windows)]
        if let Some(root) = model.parent().and_then(Path::parent) {
            let digest = option_env!("NOTDOT_WAKE_HELPER_SHA256").unwrap_or("");
            if let Some(helper) = notdot_wake_runtime::package::select(
                root,
                digest,
                notdot_wake_runtime::package::capable_cpu(),
            ) && let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                let started = runtime.block_on(notdot_wake_runtime::client::Client::start(
                    &helper, model, phrase,
                ));
                let (client, cleanup_blocked) = match started {
                    Ok(client) => (Some(client), false),
                    Err(notdot_wake_runtime::client::Failure::CleanupUnconfirmed) => (None, true),
                    Err(_) => (None, false),
                };
                let retry_after = client
                    .is_none()
                    .then(|| Instant::now() + Duration::from_secs(30));
                return Ok(Self::Accelerated(Box::new(AcceleratedDecoder {
                    runtime,
                    client,
                    resource_root: root.to_owned(),
                    model: model.to_owned(),
                    retry_after,
                    cleanup_blocked,
                })));
            }
        }
        let context = generic_context(model)?;
        notdot_wake_runtime::engine::prepare(&context, phrase, DecodeLimits::default().threads)?;
        Ok(Self::Generic(context))
    }
    fn identity(&self) -> &'static str {
        match self {
            Self::Generic(_) => "generic",
            Self::Unavailable { .. } => "unavailable",
            #[cfg(windows)]
            Self::Accelerated(a) if a.client.is_some() => "accelerated",
            #[cfg(windows)]
            Self::Accelerated(_) => "recovering",
        }
    }
    #[cfg(test)]
    fn recognize(
        &mut self,
        phrase: &str,
        samples: &[f32],
        rate: u32,
        epoch: Arc<AtomicU64>,
        expected: u64,
    ) -> Result<bool, String> {
        self.recognize_with_budget(
            phrase,
            samples,
            rate,
            epoch,
            expected,
            Duration::from_secs(3),
        )
    }
    fn recognize_with_budget(
        &mut self,
        phrase: &str,
        samples: &[f32],
        rate: u32,
        epoch: Arc<AtomicU64>,
        expected: u64,
        budget: Duration,
    ) -> Result<bool, String> {
        if epoch.load(Ordering::Relaxed) != expected {
            return Ok(false);
        }
        match self {
            Self::Unavailable { .. } => Err(
                "Some wake phrases may be missed because backup recognition is unavailable.".into(),
            ),
            Self::Generic(context) => recognize_with_limits(
                context,
                phrase,
                samples,
                rate,
                epoch,
                expected,
                DecodeLimits {
                    budget,
                    ..DecodeLimits::default()
                },
            ),
            #[cfg(windows)]
            Self::Accelerated(accelerated) => {
                let AcceleratedDecoder {
                    runtime,
                    client,
                    cleanup_blocked,
                    ..
                } = accelerated.as_mut();
                let audio = notdot_wake_runtime::engine::normalize(samples, rate)?;
                if client.is_none() {
                    return Err(
                        "Some wake phrases may be missed while recognition restarts.".into(),
                    );
                }
                let result = runtime.block_on(async {
                    client
                        .as_mut()
                        .expect("helper initialized")
                        .recognize(&audio, epoch, expected, budget)
                        .await
                });
                match result {
                    Ok(heard) => Ok(heard),
                    Err(notdot_wake_runtime::client::Failure::Cancelled) => {
                        *client = None;
                        Ok(false)
                    }
                    Err(notdot_wake_runtime::client::Failure::Unavailable) => {
                        *client = None;
                        Err("Backup wake recognition was interrupted. It is restarting.".into())
                    }
                    Err(notdot_wake_runtime::client::Failure::CleanupUnconfirmed) => {
                        *client = None;
                        *cleanup_blocked = true;
                        Err("Wake recognition could not stop safely. Restart notdot before trying again.".into())
                    }
                }
            }
        }
    }
    // Recovery never receives audio and never replays the failed utterance.
    // One idle attempt is allowed after a fault; failed attempts back off for
    // thirty seconds and require another speech attempt before trying again.
    fn recover(
        &mut self,
        phrase: &str,
        epoch: Arc<AtomicU64>,
        expected: u64,
    ) -> Option<Result<(), String>> {
        if let Self::Unavailable { model, retry_after } = self {
            if Instant::now() < *retry_after {
                return None;
            }
            *retry_after = Instant::now() + Duration::from_secs(30);
            let result = Self::new(model, phrase);
            return Some(match result {
                Ok(decoder) => {
                    *self = decoder;
                    if self.unavailable() {
                        Err("Some wake phrases may be missed. Check whether your security software blocked notdot.".into())
                    } else {
                        Ok(())
                    }
                }
                Err(error) => Err(error),
            });
        }
        #[cfg(windows)]
        if let Self::Accelerated(a) = self {
            if a.cleanup_blocked {
                return Some(Err("Wake recognition cleanup was not confirmed. Restart notdot before trying again.".into()));
            }
            if a.client.is_some() || a.retry_after.is_some_and(|at| Instant::now() < at) {
                return None;
            }
            a.retry_after = Some(Instant::now() + Duration::from_secs(30));
            let helper = notdot_wake_runtime::package::select(
                &a.resource_root,
                option_env!("NOTDOT_WAKE_HELPER_SHA256").unwrap_or(""),
                notdot_wake_runtime::package::capable_cpu(),
            );
            let Some(helper) = helper else {
                return Some(Err("Backup wake recognition is unavailable. Check whether your security software blocked notdot.".into()));
            };
            let result = a
                .runtime
                .block_on(notdot_wake_runtime::client::Client::recover(
                    &helper, &a.model, phrase, epoch, expected,
                ));
            return match result {
                Ok(client) => {
                    a.client = Some(client);
                    a.retry_after = None;
                    Some(Ok(()))
                }
                Err(notdot_wake_runtime::client::Failure::Cancelled) => {
                    a.retry_after = None;
                    None
                }
                Err(notdot_wake_runtime::client::Failure::Unavailable) => Some(Err("Backup wake recognition could not restart. Check whether your security software blocked notdot.".into())),
                Err(notdot_wake_runtime::client::Failure::CleanupUnconfirmed) => {
                    a.cleanup_blocked = true;
                    Some(Err("Wake recognition cleanup was not confirmed. Restart notdot before trying again.".into()))
                }
            };
        }
        #[cfg(not(windows))]
        let _ = (phrase, epoch, expected);
        None
    }
    fn unavailable(&self) -> bool {
        if matches!(self, Self::Unavailable { .. }) {
            return true;
        }
        #[cfg(windows)]
        if let Self::Accelerated(a) = self {
            return a.client.is_none();
        }
        false
    }
}
impl Decoder {
    fn shutdown(&mut self) -> bool {
        #[cfg(windows)]
        if let Self::Accelerated(accelerated) = self
            && let Some(mut client) = accelerated.client.take()
        {
            return accelerated.runtime.block_on(client.shutdown());
        }
        true
    }
}
impl Drop for Decoder {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

struct Job {
    epoch: u64,
    samples: Vec<f32>,
    rate: u32,
    deadline: Instant,
}
enum Event {
    Decoded(u64, Result<bool, String>, &'static str),
    Recovered(u64, Result<(), String>, &'static str),
}
pub(super) struct Fallback {
    sender: Option<mpsc::SyncSender<Job>>,
    receiver: mpsc::Receiver<Event>,
    backend: &'static str,
    thread: Option<JoinHandle<bool>>,
    epoch: Arc<AtomicU64>,
    alive: Arc<AtomicBool>,
    busy: bool,
    gate: VoiceGate,
    tail: VecDeque<f32>,
    samples: Vec<f32>,
    silence: usize,
    pause_ms: u32,
    speech: usize,
    pub(super) error: Option<String>,
    pub(super) failures: u64,
}
impl Fallback {
    pub(super) fn new(path: &Path, phrase: &str) -> Result<Self, String> {
        // A missing or failed backup must not disable the independent primary
        // keyword detector. The worker can retry its setup after a later phrase.
        let (mut decoder, setup_error) = match Decoder::new(path, phrase) {
            Ok(decoder) => (decoder, None),
            Err(error) => (
                Decoder::Unavailable {
                    model: path.to_owned(),
                    retry_after: Instant::now() + Duration::from_secs(30),
                },
                Some(format!("Backup wake recognition could not start: {error}")),
            ),
        };
        let backend = decoder.identity();
        let initial_error = setup_error.or_else(|| decoder.unavailable().then(||
            "Some wake phrases may be missed. Check whether your security software blocked notdot.".to_owned()
        ));
        let failures = u64::from(initial_error.is_some());
        let (sender, jobs) = mpsc::sync_channel::<Job>(1);
        let (results, receiver) = mpsc::sync_channel(2);
        let epoch = Arc::new(AtomicU64::new(0));
        let worker_epoch = epoch.clone();
        let alive = Arc::new(AtomicBool::new(true));
        let worker_alive = alive.clone();
        let phrase = phrase.to_owned();
        let thread = std::thread::Builder::new()
            .name("notdot-wake-fallback".into())
            .spawn(move || {
                while let Ok(job) = jobs.recv() {
                    let result = match job.deadline.checked_duration_since(Instant::now()).filter(|budget| !budget.is_zero()) {
                    Some(budget) => decoder.recognize_with_budget(
                        &phrase,
                        &job.samples,
                        job.rate,
                        worker_epoch.clone(),
                        job.epoch,
                        budget,
                    ),
                    None => Err("That wake phrase expired while recognition was recovering. Please say it again.".into()),
                    };
                    let _ = results.try_send(Event::Decoded(job.epoch, result, decoder.identity()));
                    // A reset can supersede a warm-up. Retry at most once for
                    // that newer epoch; shutdown never starts a replacement.
                    for _ in 0..2 {
                        if !worker_alive.load(Ordering::Relaxed) { break; }
                        let expected = worker_epoch.load(Ordering::Relaxed);
                        if let Some(result) = decoder.recover(&phrase, worker_epoch.clone(), expected) {
                            let _ = results.try_send(Event::Recovered(expected, result, decoder.identity()));
                        }
                        if worker_epoch.load(Ordering::Relaxed) == expected { break; }
                    }
                }
                decoder.shutdown()
            })
            .map_err(|_| "Could not start local wake recognition")?;
        Ok(Self {
            sender: Some(sender),
            receiver,
            backend,
            thread: Some(thread),
            epoch,
            alive,
            busy: false,
            gate: VoiceGate::default(),
            tail: VecDeque::new(),
            samples: Vec::new(),
            silence: 0,
            // Keep a natural short pause between the greeting and name in one decode.
            pause_ms: 350,
            speech: 0,
            error: initial_error,
            failures,
        })
    }
    pub(super) fn shutdown(&mut self) -> bool {
        self.alive.store(false, Ordering::Relaxed);
        self.epoch.fetch_add(1, Ordering::Relaxed);
        self.sender.take();
        self.thread
            .take()
            .is_none_or(|thread| thread.join().unwrap_or(false))
    }
    pub(super) fn backend(&self) -> &'static str {
        self.backend
    }
    #[cfg(test)]
    pub(super) fn is_busy(&self) -> bool {
        self.busy
    }
    pub(super) fn reset(&mut self) {
        self.epoch.fetch_add(1, Ordering::Relaxed);
        self.tail.clear();
        self.samples.clear();
        self.speech = 0;
        self.silence = 0;
        self.gate = VoiceGate::default();
    }
    pub(super) fn accept(&mut self, rate: u32, samples: &[f32]) -> bool {
        while let Ok(event) = self.receiver.try_recv() {
            if let Event::Recovered(epoch, result, backend) = event {
                if epoch == self.epoch.load(Ordering::Relaxed) {
                    self.backend = backend;
                    match result {
                        Ok(()) => self.error = None,
                        Err(error) => {
                            self.failures = self.failures.saturating_add(1);
                            self.error = Some(error);
                        }
                    }
                }
                continue;
            }
            let Event::Decoded(epoch, result, backend) = event else {
                unreachable!()
            };
            self.backend = backend;
            self.busy = false;
            if epoch == self.epoch.load(Ordering::Relaxed) {
                match result {
                    Ok(heard) => {
                        self.error = None;
                        if heard {
                            return true;
                        }
                    }
                    Err(e) => {
                        self.failures = self.failures.saturating_add(1);
                        self.error = Some(e);
                        self.samples.clear();
                        self.tail.clear();
                        self.speech = 0;
                        self.silence = 0;
                    }
                }
            }
        }
        let voiced = self.gate.observe(super::energy(samples));
        if self.samples.is_empty() {
            if voiced {
                self.samples.extend(self.tail.drain(..));
            } else {
                self.tail.extend(samples.iter().copied());
                while self.tail.len() > rate as usize / 2 {
                    self.tail.pop_front();
                }
                return false;
            }
        }
        let remaining = (rate as usize * 6).saturating_sub(self.samples.len());
        self.samples
            .extend_from_slice(&samples[..samples.len().min(remaining)]);
        if voiced {
            self.silence = 0;
            self.speech += samples.len();
        } else {
            self.silence += samples.len();
        }
        if !self.busy
            && (self.silence >= rate as usize * self.pause_ms as usize / 1000
                || self.samples.len() >= rate as usize * 6)
        {
            let samples = std::mem::take(&mut self.samples);
            if self.speech >= rate as usize / 5 && !self.busy {
                let job = Job {
                    epoch: self.epoch.load(Ordering::Relaxed),
                    samples,
                    rate,
                    deadline: Instant::now() + Duration::from_secs(3),
                };
                self.busy = self
                    .sender
                    .as_ref()
                    .is_some_and(|s| s.try_send(job).is_ok());
            }
            self.speech = 0;
            self.silence = 0;
        }
        false
    }
}
impl Drop for Fallback {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}
#[cfg(test)]
fn recognize(
    context: &WhisperContext,
    phrase: &str,
    samples: &[f32],
    rate: u32,
    epoch: Arc<AtomicU64>,
    expected: u64,
) -> Result<bool, String> {
    recognize_with_limits(
        context,
        phrase,
        samples,
        rate,
        epoch,
        expected,
        DecodeLimits::default(),
    )
}
fn recognize_with_limits(
    context: &WhisperContext,
    phrase: &str,
    samples: &[f32],
    rate: u32,
    epoch: Arc<AtomicU64>,
    expected: u64,
    limits: DecodeLimits,
) -> Result<bool, String> {
    notdot_wake_runtime::engine::recognize(context, phrase, samples, rate, epoch, expected, limits)
}
#[cfg(test)]
fn addressed(text: &str, phrase: &str) -> bool {
    notdot_wake_runtime::engine::addressed(text, phrase)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn interrupted_accelerator_recovers_without_permanent_generic_fallback() {
        let _models = super::super::model_fixture_guard();
        if !notdot_wake_runtime::package::capable_cpu()
            || option_env!("NOTDOT_WAKE_HELPER_SHA256")
                .unwrap_or("")
                .is_empty()
        {
            return;
        }
        let root = super::super::fixture_root();
        let mut decoder = Decoder::new(
            &root.join("resources/wake/ggml-tiny.en-q5_1.bin"),
            "Hey Ava",
        )
        .unwrap();
        let Decoder::Accelerated(a) = &mut decoder else {
            panic!("expected the packaged accelerator");
        };
        assert!(
            a.runtime
                .block_on(a.client.as_mut().expect("helper ready").shutdown())
        );
        let mut wav = hound::WavReader::open(root.join("tests/fixtures/hey-ava-uk.wav")).unwrap();
        let samples: Vec<_> = wav
            .samples::<i16>()
            .map(|s| f32::from(s.unwrap()) / 32768.)
            .collect();
        let epoch = Arc::new(AtomicU64::new(0));
        assert!(
            decoder
                .recognize("Hey Ava", &samples, 16000, epoch.clone(), 0)
                .is_err()
        );
        assert_eq!(decoder.identity(), "recovering");
        assert!(matches!(
            decoder.recover("Hey Ava", epoch.clone(), 0),
            Some(Ok(()))
        ));
        assert_eq!(decoder.identity(), "accelerated");
        assert!(
            decoder
                .recognize("Hey Ava", &samples, 16000, epoch.clone(), 0)
                .unwrap()
        );
        let directory = tempfile::tempdir().unwrap();
        let Decoder::Accelerated(a) = &mut decoder else {
            unreachable!()
        };
        let original_root = a.resource_root.clone();
        assert!(a.runtime.block_on(a.client.as_mut().unwrap().shutdown()));
        a.client = None;
        a.resource_root = directory.path().to_owned();
        assert!(matches!(
            decoder.recover("Hey Ava", epoch.clone(), 0),
            Some(Err(_))
        ));
        assert!(
            decoder.recover("Hey Ava", epoch, 0).is_none(),
            "a failed recovery must back off"
        );
        let Decoder::Accelerated(a) = &mut decoder else {
            unreachable!()
        };
        a.resource_root = original_root;
        a.retry_after = None;
        a.cleanup_blocked = true;
        assert!(
            matches!(decoder.recover("Hey Ava", Arc::new(AtomicU64::new(0)), 0), Some(Err(error)) if error.contains("cleanup")),
            "unconfirmed old-child cleanup must block a replacement even with valid resources"
        );
        assert!(decoder.shutdown());
        assert!(notdot_wake_runtime::client::all_helpers_reaped());
    }
    #[test]
    fn failed_backup_setup_remains_a_bounded_recoverable_state() {
        let _models = super::super::model_fixture_guard();
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("missing-model.bin");
        let mut fallback = Fallback::new(&model, "Hey Ava").unwrap();
        assert_eq!(fallback.backend(), "unavailable");
        assert!(fallback.error.is_some());
        fallback
            .sender
            .as_ref()
            .unwrap()
            .send(Job {
                epoch: 0,
                samples: vec![0.1; 320],
                rate: 16000,
                deadline: Instant::now() - Duration::from_secs(1),
            })
            .unwrap();
        fallback.busy = true;
        let until = Instant::now() + Duration::from_secs(2);
        while fallback.busy && Instant::now() < until {
            assert!(!fallback.accept(16000, &[0.; 320]));
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!fallback.busy);
        assert!(fallback.error.as_deref().unwrap().contains("expired"));
        assert!(fallback.shutdown());
    }
    #[test]
    fn recovery_metadata_cannot_match_audio_or_finish_a_newer_job() {
        let _models = super::super::model_fixture_guard();
        let dir = tempfile::tempdir().unwrap();
        let mut fallback = Fallback::new(&dir.path().join("missing.bin"), "Hey Ava").unwrap();
        let (events, receiver) = mpsc::sync_channel(2);
        fallback.receiver = receiver;
        fallback.busy = true;
        events
            .send(Event::Recovered(99, Ok(()), "generic"))
            .unwrap();
        assert!(!fallback.accept(16000, &[0.; 320]));
        assert_eq!(fallback.backend(), "unavailable");
        assert!(fallback.error.is_some());
        assert!(fallback.busy);
        events.send(Event::Recovered(0, Ok(()), "generic")).unwrap();
        assert!(!fallback.accept(16000, &[0.; 320]));
        assert_eq!(fallback.backend(), "generic");
        assert!(fallback.error.is_none());
        assert!(fallback.busy);
        assert!(fallback.shutdown());
    }
    #[test]
    #[ignore = "bounded local wake decode comparison; synthetic audio, no microphone or provider"]
    fn decode_budget_diagnostic() {
        let _models = super::super::model_fixture_guard();
        let root = super::super::fixture_root();
        for (flash_attn, threads) in [(false, 2), (true, 2), (false, 4)] {
            let mut p = WhisperContextParameters::default();
            p.use_gpu(false).flash_attn(flash_attn);
            let context = WhisperContext::new_with_params(
                root.join("resources/wake/ggml-tiny.en-q5_1.bin"),
                p,
            )
            .unwrap();
            let mut correct = 0;
            let start = Instant::now();
            for (name, expected) in [
                ("hey-ava.wav", true),
                ("hey-ava-uk.wav", true),
                ("hey-ava-us.wav", true),
                ("ava.wav", false),
                ("negative.wav", false),
            ] {
                let mut wav =
                    hound::WavReader::open(root.join("tests/fixtures").join(name)).unwrap();
                let rate = wav.spec().sample_rate;
                let mut samples: Vec<_> = wav
                    .samples::<i16>()
                    .map(|s| f32::from(s.unwrap()) / 32768.)
                    .collect();
                samples.resize(samples.len() + rate as usize / 2, 0.);
                let item_start = Instant::now();
                let result = recognize_with_limits(
                    &context,
                    "Hey Ava",
                    &samples,
                    rate,
                    Arc::new(AtomicU64::new(0)),
                    0,
                    DecodeLimits {
                        budget: Duration::from_secs(15),
                        threads,
                    },
                );
                correct += usize::from(result.as_ref().is_ok_and(|heard| *heard == expected));
                eprintln!(
                    "wake diagnostic flash_attn={flash_attn} threads={threads} fixture={name} elapsed_ms={} result={result:?}",
                    item_start.elapsed().as_millis()
                );
            }
            eprintln!(
                "wake diagnostic flash_attn={flash_attn} threads={threads} elapsed_ms={} correct={correct}/5",
                start.elapsed().as_millis()
            );
        }
    }

    #[test]
    fn short_pause_inside_wake_phrase_does_not_split_the_decode() {
        let _models = super::super::model_fixture_guard();
        let root = super::super::fixture_root();
        let mut f = Fallback::new(
            &root.join("resources/wake/ggml-tiny.en-q5_1.bin"),
            "Hey Ava",
        )
        .unwrap();
        for _ in 0..25 {
            assert!(!f.accept(16000, &[0.; 320]));
        }
        for _ in 0..15 {
            assert!(!f.accept(16000, &[0.03; 320]));
        }
        for _ in 0..13 {
            assert!(!f.accept(16000, &[0.; 320]));
        }
        assert!(
            !f.busy,
            "a natural pause must not send the greeting without the name"
        );
        assert!(!f.samples.is_empty());
    }
    #[test]
    fn only_direct_address_matches_the_configured_phrase() {
        assert!(addressed("Hey, Ava!", "Hey Ava"));
        assert!(addressed("Hey Ava, help me", "Hey Ava"));
        for s in [
            "Ava",
            "The weather is pleasant today",
            "Don't say Hey Ava",
            "Hey Eva",
            "hey avalanche",
        ] {
            assert!(!addressed(s, "Hey Ava"));
        }
        assert!(addressed("Ava!", "Ava"));
    }
    #[test]
    fn bundled_phrases_reject_name_only_unrelated_audio_and_cancelled_work() {
        let _models = super::super::model_fixture_guard();
        let root = super::super::fixture_root();
        let model = root.join("resources/wake/ggml-tiny.en-q5_1.bin");
        let mut decoder = Decoder::new(&model, "Hey Ava").unwrap();
        #[cfg(windows)]
        if notdot_wake_runtime::package::capable_cpu()
            && !option_env!("NOTDOT_WAKE_HELPER_SHA256")
                .unwrap_or("")
                .is_empty()
        {
            assert_eq!(
                decoder.identity(),
                "accelerated",
                "native fixture resources must include the verified packaged helper"
            );
        }
        for (name, expected) in [
            ("hey-ava.wav", true),
            ("hey-ava-uk.wav", true),
            ("hey-ava-us.wav", true),
            ("ava.wav", false),
            ("negative.wav", false),
        ] {
            let mut wav = hound::WavReader::open(root.join("tests/fixtures").join(name)).unwrap();
            let rate = wav.spec().sample_rate;
            let mut samples: Vec<_> = wav
                .samples::<i16>()
                .map(|s| f32::from(s.unwrap()) / 32768.)
                .collect();
            samples.resize(samples.len() + rate as usize / 2, 0.);
            let backend = decoder.identity();
            let started = Instant::now();
            let heard = decoder
                .recognize("Hey Ava", &samples, rate, Arc::new(AtomicU64::new(0)), 0)
                .unwrap_or_else(|error| {
                    panic!(
                        "{name}: backend={backend}, after={}, elapsed={:?}: {error}",
                        decoder.identity(),
                        started.elapsed()
                    )
                });
            assert_eq!(
                heard,
                expected,
                "{name}: backend={backend}, after={}, elapsed={:?}",
                decoder.identity(),
                started.elapsed()
            );
        }
        // Keep direct generic cancellation/deadline coverage separate from the
        // correctness corpus, which must exercise the production-selected path.
        let context = generic_context(&model).unwrap();
        let timeout = recognize_with_limits(
            &context,
            "Hey Ava",
            &[0.01; 16000],
            16000,
            Arc::new(AtomicU64::new(0)),
            0,
            DecodeLimits {
                budget: Duration::ZERO,
                threads: 2,
            },
        );
        assert_eq!(
            timeout.unwrap_err(),
            "Local wake recognition exceeded its time limit"
        );
        assert!(
            !recognize(
                &context,
                "Hey Ava",
                &[0.; 16000],
                16000,
                Arc::new(AtomicU64::new(1)),
                0
            )
            .unwrap()
        );
    }
    #[test]
    fn production_framed_fallback_corpus_bypasses_keyword_spotting() {
        let _models = super::super::model_fixture_guard();
        let root = super::super::fixture_root();
        let model = root.join("resources/wake/ggml-tiny.en-q5_1.bin");
        let mut framed = Vec::new();
        for (name, expected) in [
            ("hey-ava.wav", true),
            ("hey-ava-uk.wav", true),
            ("hey-ava-us.wav", true),
            ("ava.wav", false),
            ("negative.wav", false),
        ] {
            for rate in [16000, 48000] {
                let mut fallback = Fallback::new(&model, "Hey Ava").unwrap();
                // Intercept the actual production gate's job, without allowing a
                // primary KWS match to hide a fallback transcription failure.
                let (sender, jobs) = mpsc::sync_channel(1);
                fallback.sender = Some(sender);
                fallback.reset();
                fallback.accept(rate, &vec![0.; rate as usize / 2]);
                let mut wav =
                    hound::WavReader::open(root.join("tests/fixtures").join(name)).unwrap();
                assert_eq!(wav.spec().sample_rate, 16000);
                let audio: Vec<_> = wav
                    .samples::<i16>()
                    .map(|sample| f32::from(sample.unwrap()) / 32768.)
                    .flat_map(|sample| vec![sample; rate as usize / 16000])
                    .collect();
                for frame in audio.chunks(rate as usize / 50) {
                    assert!(!fallback.accept(rate, frame));
                }
                for _ in 0..40 {
                    assert!(!fallback.accept(rate, &vec![0.; rate as usize / 50]));
                }
                let job = jobs
                    .try_recv()
                    .expect("production framing must queue the fixture");
                if name == "hey-ava.wav" && rate == 48000 {
                    assert_eq!(job.samples.len(), 81600, "retain the diagnosed framing");
                }
                assert!(
                    fallback.shutdown(),
                    "framing worker must release its decoder"
                );
                framed.push((name, expected, job));
            }
        }
        let mut decoder = Decoder::new(&model, "Hey Ava").unwrap();
        #[cfg(windows)]
        if notdot_wake_runtime::package::capable_cpu()
            && !option_env!("NOTDOT_WAKE_HELPER_SHA256")
                .unwrap_or("")
                .is_empty()
        {
            assert_eq!(decoder.identity(), "accelerated");
        }
        for (name, expected, job) in framed {
            let backend = decoder.identity();
            let started = Instant::now();
            let heard = decoder.recognize(
                "Hey Ava",
                &job.samples,
                job.rate,
                Arc::new(AtomicU64::new(job.epoch)),
                job.epoch,
            );
            eprintln!(
                "framed_wake fixture={name} rate={} backend={backend} after={} elapsed_ms={} result={heard:?} expected={expected}",
                job.rate,
                decoder.identity(),
                started.elapsed().as_millis()
            );
            assert_eq!(
                heard.expect("production fallback must finish within its unchanged decode budget"),
                expected,
                "{name} at{}Hz: backend={backend}, after={}, elapsed={:?}",
                job.rate,
                decoder.identity(),
                started.elapsed()
            );
        }
        assert!(decoder.shutdown(), "tested decoder must be released");
    }

    #[test]
    fn reset_discards_an_in_flight_match_and_quiet_audio_does_not_queue_work() {
        let _models = super::super::model_fixture_guard();
        let root = super::super::fixture_root();
        let mut f = Fallback::new(
            &root.join("resources/wake/ggml-tiny.en-q5_1.bin"),
            "Hey Ava",
        )
        .unwrap();
        for _ in 0..100 {
            assert!(!f.accept(16000, &[0.; 320]));
        }
        assert!(!f.busy);
        let mut wav = hound::WavReader::open(root.join("tests/fixtures/hey-ava.wav")).unwrap();
        let audio: Vec<_> = wav
            .samples::<i16>()
            .map(|s| f32::from(s.unwrap()) / 32768.)
            .collect();
        for chunk in audio.chunks(320) {
            assert!(!f.accept(16000, chunk));
        }
        for _ in 0..30 {
            assert!(!f.accept(16000, &[0.; 320]));
        }
        assert!(f.busy);
        f.reset();
        for _ in 0..400 {
            assert!(!f.accept(16000, &[0.; 320]));
            if !f.busy {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!f.busy);
        assert!(f.error.is_none());
    }
    #[test]
    #[ignore = "private local audio fixture through the asynchronous speech gate"]
    fn local_stream_fixture() {
        let _models = super::super::model_fixture_guard();
        let root = super::super::fixture_root();
        let path = std::env::var("NOTDOT_WAKE_FIXTURE").unwrap();
        let mut wav = hound::WavReader::open(path).unwrap();
        let rate = wav.spec().sample_rate;
        let audio: Vec<_> = wav
            .samples::<i16>()
            .map(|s| f32::from(s.unwrap()) / 32768.)
            .take(rate as usize * 30 + 1)
            .collect();
        assert!(audio.len() <= rate as usize * 30);
        let mut fallback = Fallback::new(
            &root.join("resources/wake/ggml-tiny.en-q5_1.bin"),
            "Hey Ava",
        )
        .unwrap();
        if let Ok(pause) = std::env::var("NOTDOT_WAKE_PAUSE_MS") {
            fallback.pause_ms = pause.parse().unwrap();
            assert!([200, 300, 350, 500].contains(&fallback.pause_ms));
        }
        let start = Instant::now();
        let mut supplied = 0usize;
        let mut heard = false;
        for chunk in audio.chunks(rate as usize / 50) {
            supplied += chunk.len();
            let deadline = start + Duration::from_secs_f64(supplied as f64 / f64::from(rate));
            if let Some(wait) = deadline.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
            heard |= fallback.accept(rate, chunk);
            if heard {
                break;
            }
        }
        for _ in 0..200 {
            heard |= fallback.accept(rate, &vec![0.; rate as usize / 50]);
            if heard {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        eprintln!(
            "stream activation_ms={} matched={heard}",
            start.elapsed().as_millis()
        );
        assert!(fallback.error.is_none(), "{:?}", fallback.error);
        let expected = std::env::var("NOTDOT_WAKE_EXPECTED").map_or(true, |s| s != "false");
        assert_eq!(heard, expected);
    }
    #[test]
    #[ignore = "private local audio fixture; never commit the recording"]
    fn local_phrase_fixture() {
        let _models = super::super::model_fixture_guard();
        let root = super::super::fixture_root();
        let path = std::env::var("NOTDOT_WAKE_FIXTURE").unwrap();
        let mut wav = hound::WavReader::open(path).unwrap();
        let rate = wav.spec().sample_rate;
        assert_eq!(wav.spec().channels, 1);
        let audio: Vec<_> = wav
            .samples::<i16>()
            .map(|s| f32::from(s.unwrap()) / 32768.)
            .take(rate as usize * 30 + 1)
            .collect();
        assert!(audio.len() <= rate as usize * 30);
        let mut p = WhisperContextParameters::default();
        p.use_gpu(false);
        let context =
            WhisperContext::new_with_params(root.join("resources/wake/ggml-tiny.en-q5_1.bin"), p)
                .unwrap();
        let start = Instant::now();
        let heard = recognize(
            &context,
            "Hey Ava",
            &audio,
            rate,
            Arc::new(AtomicU64::new(0)),
            0,
        )
        .unwrap();
        eprintln!(
            "local wake inference_ms={} matched={heard}",
            start.elapsed().as_millis()
        );
        let expected = std::env::var("NOTDOT_WAKE_EXPECT").unwrap_or("true".into()) == "true";
        assert_eq!(heard, expected);
    }
}
