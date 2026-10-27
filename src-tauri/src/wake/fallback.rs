//! Local, speech-gated recognition for phrases the small keyword model misses.
//! Audio is bounded, ephemeral, and never sent to a voice provider or written.
use super::VoiceGate;
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use std::{
    collections::VecDeque,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

struct Job {
    epoch: u64,
    samples: Vec<f32>,
    rate: u32,
}
pub(super) struct Fallback {
    sender: Option<mpsc::SyncSender<Job>>,
    receiver: mpsc::Receiver<(u64, Result<bool, String>)>,
    thread: Option<JoinHandle<()>>,
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
        whisper_rs::install_logging_hooks();
        let mut context_params = WhisperContextParameters::default();
        context_params.use_gpu(false);
        let context = WhisperContext::new_with_params(path, context_params)
            .map_err(|_| "Could not load the bundled local wake recognizer")?;
        let (sender, jobs) = mpsc::sync_channel::<Job>(1);
        let (results, receiver) = mpsc::sync_channel(1);
        let epoch = Arc::new(AtomicU64::new(0));
        let worker_epoch = epoch.clone();
        let phrase = phrase.to_owned();
        let thread = std::thread::Builder::new()
            .name("notdot-wake-fallback".into())
            .spawn(move || {
                while let Ok(job) = jobs.recv() {
                    let result = recognize(
                        &context,
                        &phrase,
                        &job.samples,
                        job.rate,
                        worker_epoch.clone(),
                        job.epoch,
                    );
                    let _ = results.try_send((job.epoch, result));
                }
            })
            .map_err(|_| "Could not start local wake recognition")?;
        Ok(Self {
            sender: Some(sender),
            receiver,
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
        if let Ok((epoch, result)) = self.receiver.try_recv() {
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
        self.epoch.fetch_add(1, Ordering::Relaxed);
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
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
    whisper_rs::install_logging_hooks();
    if epoch.load(Ordering::Relaxed) != expected {
        return Ok(false);
    }
    let audio = if rate == 16000 {
        samples.to_vec()
    } else {
        let params = SincInterpolationParameters {
            sinc_len: 128,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 128,
            window: WindowFunction::BlackmanHarris2,
        };
        let mut resampler =
            SincFixedIn::<f32>::new(16000. / f64::from(rate), 1., params, samples.len(), 1)
                .map_err(|e| e.to_string())?;
        resampler
            .process(&[samples], None)
            .map_err(|e| e.to_string())?
            .remove(0)
    };
    let mut state = context.create_state().map_err(|e| e.to_string())?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_n_threads(2);
    params.set_max_tokens(32);
    params.set_temperature_inc(0.);
    params.set_suppress_nst(true);
    params.set_language(Some("en"));
    params.set_translate(false);
    params.set_no_context(true);
    params.set_no_timestamps(true);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_initial_prompt(phrase.strip_prefix("Hey ").unwrap_or(phrase));
    let deadline = Instant::now() + Duration::from_secs(3);
    let cancelled = epoch.clone();
    let abort: Box<dyn FnMut() -> bool> = Box::new(move || {
        cancelled.load(Ordering::Relaxed) != expected || Instant::now() > deadline
    });
    params.set_abort_callback_safe::<_, Box<dyn FnMut() -> bool>>(Some(abort));
    if let Err(e) = state.full(params, &audio) {
        return if epoch.load(Ordering::Relaxed) != expected {
            Ok(false)
        } else {
            Err(format!("Local wake recognition could not finish: {e}"))
        };
    }
    let mut text = String::new();
    for segment in state.as_iter() {
        if segment.no_speech_probability() > 0.6 {
            return Ok(false);
        }
        text.push_str(segment.to_str().map_err(|e| e.to_string())?);
    }
    Ok(addressed(&text, phrase))
}
fn addressed(text: &str, phrase: &str) -> bool {
    let words = |s: &str| {
        s.split(|c: char| !c.is_ascii_alphabetic())
            .filter(|s| !s.is_empty())
            .map(str::to_ascii_lowercase)
            .collect::<Vec<_>>()
    };
    let expected = words(phrase);
    let actual = words(text);
    !expected.is_empty() && actual.starts_with(&expected)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn short_pause_inside_wake_phrase_does_not_split_the_decode() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
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
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut p = WhisperContextParameters::default();
        p.use_gpu(false);
        let context =
            WhisperContext::new_with_params(root.join("resources/wake/ggml-tiny.en-q5_1.bin"), p)
                .unwrap();
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
            let heard = recognize(
                &context,
                "Hey Ava",
                &samples,
                rate,
                Arc::new(AtomicU64::new(0)),
                0,
            )
            .unwrap();
            assert_eq!(heard, expected, "{name}");
        }
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
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
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
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
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
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
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
