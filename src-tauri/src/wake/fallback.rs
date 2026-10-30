//! Local, speech-gated recognition for phrases the small keyword model misses.
//! Audio is bounded, ephemeral, and never sent to a voice provider or written.
use super::VoiceGate;
use notdot_wake_runtime::engine::DecodeLimits;
#[cfg(any(windows, test))]
use std::time::{Duration, Instant};
use std::{
    collections::VecDeque,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
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
enum Decoder {
    Generic(WhisperContext),
    #[cfg(windows)]
    Accelerated {
        runtime: tokio::runtime::Runtime,
        client: Option<notdot_wake_runtime::client::Client>,
        resource_root: std::path::PathBuf,
        model: std::path::PathBuf,
    },
    #[cfg(windows)]
    GenericPending(std::path::PathBuf),
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
            ) {
                if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    if let Ok(client) = runtime.block_on(
                        notdot_wake_runtime::client::Client::start(&helper, model, phrase),
                    ) {
                        return Ok(Self::Accelerated {
                            runtime,
                            client: Some(client),
                            resource_root: root.to_owned(),
                            model: model.to_owned(),
                        });
                    }
                }
            }
        }
        #[cfg(not(windows))]
        let _ = phrase;
        Ok(Self::Generic(generic_context(model)?))
    }
    fn identity(&self) -> &'static str {
        match self {
            Self::Generic(_) => "generic",
            #[cfg(windows)]
            Self::Accelerated { .. } => "accelerated",
            #[cfg(windows)]
            Self::GenericPending(_) => "generic",
        }
    }
    fn recognize(
        &mut self,
        phrase: &str,
        samples: &[f32],
        rate: u32,
        epoch: Arc<AtomicU64>,
        expected: u64,
    ) -> Result<bool, String> {
        if epoch.load(Ordering::Relaxed) != expected {
            return Ok(false);
        }
        #[cfg(windows)]
        if let Self::GenericPending(path) = self {
            let context = generic_context(path)?;
            *self = Self::Generic(context);
        }
        match self {
            Self::Generic(context) => recognize(context, phrase, samples, rate, epoch, expected),
            #[cfg(windows)]
            Self::Accelerated {
                runtime,
                client,
                resource_root,
                model,
            } => {
                let audio = notdot_wake_runtime::engine::normalize(samples, rate)?;
                let start = Instant::now();
                let result = runtime.block_on(async {
                    if client.is_none() {
                        let helper = notdot_wake_runtime::package::select(
                            resource_root,
                            option_env!("NOTDOT_WAKE_HELPER_SHA256").unwrap_or(""),
                            notdot_wake_runtime::package::capable_cpu(),
                        )
                        .ok_or(notdot_wake_runtime::client::Failure::Unavailable)?;
                        *client = Some(
                            notdot_wake_runtime::client::Client::start_cancellable(
                                &helper,
                                model,
                                phrase,
                                Some((epoch.clone(), expected)),
                            )
                            .await?,
                        );
                    }
                    client
                        .as_mut()
                        .expect("helper initialized")
                        .recognize(
                            &audio,
                            epoch,
                            expected,
                            Duration::from_secs(3).saturating_sub(start.elapsed()),
                        )
                        .await
                });
                match result {
                    Ok(heard) => Ok(heard),
                    Err(notdot_wake_runtime::client::Failure::Cancelled) => {
                        *client = None;
                        Ok(false)
                    }
                    Err(notdot_wake_runtime::client::Failure::Unavailable) => {
                        // Do not decode this utterance twice or extend its deadline.
                        // A subsequent utterance uses the unchanged generic decoder.
                        let path = model.clone();
                        *self = Self::GenericPending(path);
                        Ok(false)
                    }
                }
            }
            #[cfg(windows)]
            Self::GenericPending(_) => unreachable!("initialized above"),
        }
    }
}
impl Decoder {
    fn shutdown(&mut self) -> bool {
        #[cfg(windows)]
        if let Self::Accelerated {
            runtime, client, ..
        } = self
            && let Some(mut client) = client.take()
        {
            return runtime.block_on(client.shutdown());
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
}
pub(super) struct Fallback {
    sender: Option<mpsc::SyncSender<Job>>,
    receiver: mpsc::Receiver<(u64, Result<bool, String>, &'static str)>,
    backend: &'static str,
    thread: Option<JoinHandle<bool>>,
    epoch: Arc<AtomicU64>,
    busy: bool,
    gate: VoiceGate,
    tail: VecDeque<f32>,
    samples: Vec<f32>,
    silence: usize,
    pause_ms: u32,
    speech: usize,
    pub(super) error: Option<String>,
}
impl Fallback {
    pub(super) fn new(path: &Path, phrase: &str) -> Result<Self, String> {
        let mut decoder = Decoder::new(path, phrase)?;
        let backend = decoder.identity();
        let (sender, jobs) = mpsc::sync_channel::<Job>(1);
        let (results, receiver) = mpsc::sync_channel(1);
        let epoch = Arc::new(AtomicU64::new(0));
        let worker_epoch = epoch.clone();
        let phrase = phrase.to_owned();
        let thread = std::thread::Builder::new()
            .name("notdot-wake-fallback".into())
            .spawn(move || {
                while let Ok(job) = jobs.recv() {
                    let result = decoder.recognize(
                        &phrase,
                        &job.samples,
                        job.rate,
                        worker_epoch.clone(),
                        job.epoch,
                    );
                    let _ = results.try_send((job.epoch, result, decoder.identity()));
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
            busy: false,
            gate: VoiceGate::default(),
            tail: VecDeque::new(),
            samples: Vec::new(),
            silence: 0,
            // Keep a natural short pause between the greeting and name in one decode.
            pause_ms: 350,
            speech: 0,
            error: None,
        })
    }
    pub(super) fn shutdown(&mut self) -> bool {
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
        if let Ok((epoch, result, backend)) = self.receiver.try_recv() {
            self.backend = backend;
            self.busy = false;
            if epoch == self.epoch.load(Ordering::Relaxed) {
                match result {
                    Ok(true) => return true,
                    Ok(false) => {}
                    Err(e) => self.error = Some(e),
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
                .unwrap();
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
