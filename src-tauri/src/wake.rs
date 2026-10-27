//! On-device keyword spotting. One worker owns inference and microphone capture;
//! the real-time callback only downmixes into a bounded queue. No network access.
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use notdot_core::settings::Settings;
use serde::Serialize;
use sherpa_onnx::{KeywordSpotter, KeywordSpotterConfig, OnlineStream};
use std::{
    io::Cursor,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::{Duration, Instant},
};

pub fn phrase(name: &str, name_only: bool) -> Result<String, String> {
    if name
        .chars()
        .any(|c| !c.is_ascii_alphabetic() && !c.is_whitespace() && c != '-' && c != '\'')
    {
        return Err("Wake phrases currently support English names using letters, spaces, apostrophes and hyphens. Turn wake listening off to use another name.".into());
    }
    let name = name
        .replace('-', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if name.is_empty() || name.len() > 80 {
        return Err("Choose a name for the wake phrase".into());
    }
    Ok(if name_only {
        name
    } else {
        format!("Hey {name}")
    })
}
#[derive(Clone, PartialEq, Eq)]
struct Config {
    enabled: bool,
    muted: bool,
    phrase: String,
}
#[derive(Clone, Default, Serialize)]
pub struct Status {
    pub phase: String,
    pub phrase: String,
    pub message: String,
    pub rms: f32,
    pub request_id: Option<u64>,
    pub muted: bool,
}
struct Inner {
    config: Config,
    revision: u64,
    epoch: u64,
    lease: Option<Instant>,
    status: Status,
    pending: Option<(u64, Vec<u8>, Instant)>,
}
pub struct Wake {
    inner: Arc<Mutex<Inner>>,
}
impl Wake {
    pub fn new(models: PathBuf, settings: &Settings) -> Self {
        let config = config(settings);
        let inner = Arc::new(Mutex::new(Inner {
            status: Status::default(),
            config,
            revision: 1,
            epoch: 0,
            lease: None,
            pending: None,
        }));
        let weak = Arc::downgrade(&inner);
        std::thread::Builder::new()
            .name("notdot-wake".into())
            .spawn(move || {
                let mut worker = Worker::default();
                while let Some(inner) = weak.upgrade() {
                    worker.tick(&models, &inner);
                    let delay = {
                        let i = inner.lock().expect("wake state");
                        if i.config.enabled
                            && !i.config.muted
                            && i.lease
                                .is_some_and(|t| t.elapsed() < Duration::from_secs(3))
                        {
                            10
                        } else {
                            250
                        }
                    };
                    drop(inner);
                    std::thread::sleep(Duration::from_millis(delay));
                }
            })
            .expect("wake worker thread");
        Self { inner }
    }
    pub fn status(&self) -> Status {
        self.inner.lock().expect("wake state").status.clone()
    }
    pub fn configure(&self, settings: &Settings) {
        let mut i = self.inner.lock().expect("wake state");
        i.config = config(settings);
        i.revision = i.revision.wrapping_add(1);
        i.epoch = i.epoch.wrapping_add(1);
        i.lease = None;
        i.pending = None;
        i.status.request_id = None;
    }
    pub fn ready(&self, ready: bool) {
        let mut i = self.inner.lock().expect("wake state");
        if ready {
            i.lease = Some(Instant::now());
        } else {
            if i.lease.is_some() {
                i.epoch = i.epoch.wrapping_add(1);
            }
            i.lease = None;
            i.pending = None;
            i.status.request_id = None;
        }
    }
    pub fn take(&self, id: u64) -> Result<Vec<u8>, String> {
        let mut i = self.inner.lock().expect("wake state");
        if !i.config.enabled
            || i.config.muted
            || !i
                .lease
                .is_some_and(|t| t.elapsed() < Duration::from_secs(3))
        {
            i.pending = None;
            return Err("Wake capture was cancelled".into());
        }
        if !i
            .pending
            .as_ref()
            .is_some_and(|(n, _, at)| *n == id && at.elapsed() < Duration::from_secs(10))
        {
            return Err("Wake request is no longer available".into());
        }
        let (_, bytes, _) = i.pending.take().expect("pending checked");
        i.epoch = i.epoch.wrapping_add(1);
        i.lease = None;
        i.status.request_id = None;
        i.status.phase = "paused".into();
        Ok(bytes)
    }
}
fn config(settings: &Settings) -> Config {
    Config {
        enabled: settings.wake_enabled,
        muted: settings.microphone_muted,
        phrase: phrase(&settings.name, settings.wake_name_only).unwrap_or_default(),
    }
}

struct Detector {
    kws: KeywordSpotter,
    stream: OnlineStream,
    require_hey: bool,
    elapsed: f64,
    voice_gate: VoiceGate,
    voiced: std::collections::VecDeque<(f64, f64, bool)>,
}
impl Detector {
    fn new(models: &Path, phrase: &str) -> Result<Self, String> {
        let lexicon = std::fs::read_to_string(models.join("phonetic/en.phone"))
            .map_err(|_| "Bundled wake pronunciation dictionary is unavailable")?;
        let pronunciations: std::collections::HashMap<&str, Vec<&str>> = lexicon
            .lines()
            .filter_map(|l| {
                let mut parts = l.split_whitespace();
                Some((parts.next()?, parts.collect()))
            })
            .collect();
        let upper = phrase.to_uppercase();
        let phones: Option<Vec<_>> = upper
            .split_whitespace()
            .map(|word| pronunciations.get(word))
            .collect();
        let require_hey = phones.is_some() && upper.starts_with("HEY ");
        let (models, tokens) = if let Some(phones) = phones {
            (
                models.join("phonetic"),
                phones
                    .into_iter()
                    .flatten()
                    .map(|s| (*s).to_owned())
                    .collect::<Vec<_>>(),
            )
        } else {
            let tokenizer =
                sentencepiece_rs::SentencePieceProcessor::open(models.join("bpe.model"))
                    .map_err(|_| "Bundled wake tokenizer is unavailable")?;
            (
                models.to_path_buf(),
                tokenizer
                    .encode(&upper)
                    .map_err(|_| "Could not encode wake phrase")?,
            )
        };

        let vocabulary = std::fs::read_to_string(models.join("tokens.txt"))
            .map_err(|_| "Bundled wake vocabulary is unavailable")?;
        if tokens.is_empty()
            || tokens.iter().any(|t| {
                t == "<unk>"
                    || !vocabulary
                        .lines()
                        .any(|l| l.split_whitespace().next() == Some(t.as_str()))
            })
        {
            return Err("This name cannot be encoded by the English wake model".into());
        }
        let path = |name| models.join(name).to_string_lossy().into_owned();
        let mut c = KeywordSpotterConfig::default();
        c.model_config.transducer.encoder = Some(path("encoder.onnx"));
        c.model_config.transducer.decoder = Some(path("decoder.onnx"));
        c.model_config.transducer.joiner = Some(path("joiner.onnx"));
        c.model_config.tokens = Some(path("tokens.txt"));
        c.model_config.provider = Some("cpu".into());
        c.model_config.num_threads = 1;
        let mut keywords = format!("{} :3.0 #0.20 @wake\n", tokens.join(" "));
        // Hey Ava shares the EY vowel across the word boundary. Model both
        // separated and coarticulated vowels without dropping other phonemes.
        if require_hey && tokens.get(1) == tokens.get(2) {
            let mut joined = tokens.clone();
            joined.remove(2);
            keywords.push_str(&format!("{} :3.0 #0.20 @wake\n", joined.join(" ")));
        }
        c.keywords_buf = Some(keywords);
        let kws = KeywordSpotter::create(&c).ok_or("Could not load the bundled wake model")?;
        let stream = kws.create_stream();
        Ok(Self {
            kws,
            stream,
            require_hey,
            elapsed: 0.,
            voice_gate: VoiceGate::default(),
            voiced: Default::default(),
        })
    }
    fn accept(&mut self, rate: u32, samples: &[f32]) -> bool {
        // Bound the native feature history too. Prefer resetting in quiet audio.
        if self.elapsed >= 120. || (self.elapsed >= 60. && energy(samples) < 0.008) {
            self.reset();
        }
        let start = self.elapsed;
        self.elapsed += samples.len() as f64 / rate as f64;
        self.voiced.push_back((
            start,
            self.elapsed,
            self.voice_gate.observe(energy(samples)),
        ));
        while self
            .voiced
            .front()
            .is_some_and(|(_, end, _)| *end < self.elapsed - 8.)
        {
            self.voiced.pop_front();
        }
        self.stream.accept_waveform(rate as i32, samples);
        while self.kws.is_ready(&self.stream) {
            self.kws.decode(&self.stream);
            if let Some(result) = self
                .kws
                .get_result(&self.stream)
                .filter(|r| !r.keyword.is_empty())
            {
                // H is unvoiced and often aligned before its acoustic onset.
                // Check speech across the explicit Hey prefix, not only at H.
                let first = result
                    .timestamps
                    .first()
                    .map(|t| f64::from(result.start_time + t));
                let prefix_end = result
                    .timestamps
                    .get(usize::from(self.require_hey))
                    .map(|t| f64::from(result.start_time + t));
                let supported = first.zip(prefix_end).is_some_and(|(first, end)| {
                    self.voiced.iter().any(|(start, stop, voice)| {
                        *voice && *start <= end + 0.10 && *stop >= first - 0.04
                    })
                });

                self.kws.reset(&self.stream);
                // An inserted H can stretch across the name's opening vowel.
                // Require the short H-to-EY transition for the explicit Hey prefix.
                let prefix = !self.require_hey
                    || result
                        .timestamps
                        .get(1)
                        .zip(result.timestamps.first())
                        .is_some_and(|(ey, h)| *ey > *h && ey - h <= 0.16);
                #[cfg(test)]
                eprintln!(
                    "wake candidate: supported={supported} prefix={prefix} timestamps={:?}",
                    result.timestamps
                );
                if supported && prefix {
                    return true;
                }
            }
        }
        false
    }

    fn reset(&mut self) {
        self.stream = self.kws.create_stream();
        self.elapsed = 0.;
        self.voiced.clear();
    }
}

struct Microphone {
    _stream: cpal::Stream,
    receiver: Receiver<Vec<f32>>,
    failed: Arc<AtomicBool>,
    rate: u32,
}
impl Microphone {
    fn open() -> Result<Self, String> {
        let device = cpal::default_host()
            .default_input_device()
            .ok_or("No microphone is available")?;
        let supported = device
            .default_input_config()
            .map_err(|_| "Could not read microphone configuration")?;
        let format = supported.sample_format();
        let cfg: cpal::StreamConfig = supported.into();
        if cfg.sample_rate < 8000
            || cfg.sample_rate > 192000
            || cfg.channels == 0
            || cfg.channels > 32
        {
            return Err("Unsupported microphone configuration".into());
        }
        let (sender, receiver) = mpsc::sync_channel(16);
        let failed = Arc::new(AtomicBool::new(false));
        let stream = match format {
            cpal::SampleFormat::F32 => input::<f32>(&device, &cfg, sender, failed.clone()),
            cpal::SampleFormat::I16 => input::<i16>(&device, &cfg, sender, failed.clone()),
            cpal::SampleFormat::U16 => input::<u16>(&device, &cfg, sender, failed.clone()),
            _ => return Err("Unsupported microphone sample format".into()),
        }.map_err(|_| "Microphone could not start. Check OS microphone permission and the selected input device.")?;
        stream.play().map_err(|_| "Microphone could not start")?;
        Ok(Self {
            _stream: stream,
            receiver,
            failed,
            rate: cfg.sample_rate,
        })
    }
}
fn input<T>(
    device: &cpal::Device,
    cfg: &cpal::StreamConfig,
    sender: SyncSender<Vec<f32>>,
    failed: Arc<AtomicBool>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let channels = usize::from(cfg.channels);
    let frame_size = (cfg.sample_rate / 50) as usize;
    let mut frame = Vec::with_capacity(frame_size);
    let error = failed.clone();
    device.build_input_stream(
        *cfg,
        move |samples: &[T], _| {
            use cpal::Sample;
            if failed.load(Ordering::Relaxed) {
                return;
            }
            // Fixed 20 ms packets give the queue a stable 320 ms budget even
            // when a device supplies very small callbacks. No inference or I/O.
            for channels in samples.chunks_exact(channels) {
                let sample = channels
                    .iter()
                    .map(|s| {
                        let v = f32::from_sample(*s);
                        if v.is_finite() { v.clamp(-1., 1.) } else { 0. }
                    })
                    .sum::<f32>()
                    / channels.len() as f32;
                frame.push(sample);
                if frame.len() == frame_size {
                    let packet = std::mem::replace(&mut frame, Vec::with_capacity(frame_size));
                    if sender.try_send(packet).is_err() {
                        failed.store(true, Ordering::Relaxed);
                        return;
                    }
                }
            }
        },
        move |_| {
            error.store(true, Ordering::Relaxed);
        },
        None,
    )
}

/// Sample-time endpointing keeps working independently of webview animation frames.
/// Estimate a quiet floor from a bounded two-second window of 20 ms packets.
/// Keep the old upper threshold in loud environments, but support quiet inputs.
struct VoiceGate {
    levels: [f32; 100],
    count: usize,
    cursor: usize,
}
impl Default for VoiceGate {
    fn default() -> Self {
        Self {
            levels: [0.; 100],
            count: 0,
            cursor: 0,
        }
    }
}
impl VoiceGate {
    fn observe(&mut self, rms: f32) -> bool {
        let rms = if rms.is_finite() { rms.max(0.) } else { 0. };
        self.levels[self.cursor] = rms;
        self.cursor = (self.cursor + 1) % 100;
        self.count = (self.count + 1).min(100);
        let floor = self.levels[..self.count]
            .iter()
            .copied()
            .fold(f32::INFINITY, f32::min);
        let threshold = (floor * 4.).clamp(0.00015, 0.015);
        rms >= threshold
    }
}

#[derive(Default)]
struct Capture {
    samples: Vec<f32>,
    total: usize,
    speech: usize,
    silence: usize,
    voice_gate: VoiceGate,
}
enum End {
    Continue,
    Submit,
    Empty,
}
impl Capture {
    fn accept(&mut self, samples: &[f32], rate: u32) -> End {
        let rms = energy(samples);
        self.total += samples.len();
        if self.voice_gate.observe(rms) {
            self.speech += samples.len();
            self.silence = 0;
        } else {
            self.silence += samples.len();
        }
        self.samples.extend_from_slice(samples);
        if self.speech >= rate as usize / 5 && self.silence >= rate as usize * 6 / 5 {
            return End::Submit;
        }
        if self.total >= (rate as usize * 30).min(4_000_000) {
            return if self.speech >= rate as usize / 5 {
                End::Submit
            } else {
                End::Empty
            };
        }
        if self.speech < rate as usize / 5 && self.total >= rate as usize * 8 {
            return End::Empty;
        }
        End::Continue
    }
    fn wav(&self, rate: u32) -> Result<Vec<u8>, String> {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = hound::WavWriter::new(
                &mut cursor,
                hound::WavSpec {
                    channels: 1,
                    sample_rate: rate,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                },
            )
            .map_err(|e| e.to_string())?;
            for s in &self.samples {
                writer
                    .write_sample((s.clamp(-1., 1.) * i16::MAX as f32) as i16)
                    .map_err(|e| e.to_string())?;
            }
            writer.finalize().map_err(|e| e.to_string())?;
        }
        Ok(cursor.into_inner())
    }
}
fn energy(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        0.
    } else {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }
}
#[derive(Default)]
struct Worker {
    revision: u64,
    epoch: u64,
    detector: Option<Detector>,
    mic: Option<Microphone>,
    capture: Option<Capture>,
    failed: bool,
    sequence: u64,
    cooldown: Option<Instant>,
    last_audio: Option<Instant>,
}
impl Worker {
    fn error(&mut self, inner: &Mutex<Inner>, message: String) {
        self.mic = None;
        self.capture = None;
        self.failed = true;
        let mut i = inner.lock().expect("wake state");
        i.status.phase = "error".into();
        i.status.message = message;
        i.status.rms = 0.;
        i.pending = None;
    }
    fn tick(&mut self, models: &Path, inner: &Mutex<Inner>) {
        let (config, revision, epoch, allowed, pending) = {
            let i = inner.lock().expect("wake state");
            (
                i.config.clone(),
                i.revision,
                i.epoch,
                i.lease
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(3)),
                i.pending.is_some(),
            )
        };
        if epoch != self.epoch {
            self.epoch = epoch;
            self.mic = None;
            self.capture = None;
            if let Some(d) = &mut self.detector {
                d.reset();
            }
        }
        if revision != self.revision {
            self.revision = revision;
            self.detector = None;
            self.mic = None;
            self.capture = None;
            self.failed = false;
            let mut i = inner.lock().expect("wake state");
            i.status = Status {
                phrase: config.phrase.clone(),
                muted: config.muted,
                ..Status::default()
            };
        }
        if !config.enabled
            || config.muted
            || !allowed
            || pending
            || self
                .cooldown
                .is_some_and(|t| t.elapsed() < Duration::from_millis(1800))
        {
            if self.mic.take().is_some()
                && let Some(d) = &mut self.detector
            {
                d.reset();
            }
            self.capture = None;
            let mut i = inner.lock().expect("wake state");
            if !allowed {
                i.pending = None;
                i.status.request_id = None;
            }
            if !self.failed {
                i.status.phase = if !config.enabled {
                    "disabled"
                } else if config.muted {
                    "muted"
                } else if pending && allowed {
                    "ready"
                } else {
                    "paused"
                }
                .into();
            }
            if !self.failed {
                i.status.message = if !config.enabled { "Wake listening is off." } else if config.muted { "Microphone is muted." } else if pending && allowed { "Request recorded. Preparing a reply." } else { "Listening is paused while notdot answers, takes a short break or Settings is open." }.into();
            }
            i.status.rms = 0.;
            return;
        }
        if self.failed {
            return;
        }
        if self.detector.is_none() {
            inner.lock().expect("wake state").status.phase = "loading".into();
            match Detector::new(models, &config.phrase) {
                Ok(d) => self.detector = Some(d),
                Err(e) => {
                    self.error(inner, e);
                    return;
                }
            }
        }
        if self.mic.is_none() {
            match Microphone::open() {
                Ok(mic) => {
                    self.mic = Some(mic);
                    self.last_audio = Some(Instant::now());
                }
                Err(e) => {
                    self.error(inner, e);
                    return;
                }
            }
        }
        if self
            .last_audio
            .is_some_and(|at| at.elapsed() > Duration::from_secs(2))
        {
            self.error(
                inner,
                "Microphone stopped supplying audio. Save wake settings to retry.".into(),
            );
            return;
        }
        let mic = self.mic.as_ref().expect("mic initialized");
        if mic.failed.load(Ordering::Relaxed) {
            self.error(inner, "Microphone disconnected or audio processing fell behind. Save wake settings to retry.".into());
            return;
        }
        let rate = mic.rate;
        for _ in 0..16 {
            let Ok(samples) = self
                .mic
                .as_ref()
                .expect("mic initialized")
                .receiver
                .try_recv()
            else {
                break;
            };
            self.last_audio = Some(Instant::now());
            // Check cancellation after receiving audio; discarded frames never reach STT.
            {
                let i = inner.lock().expect("wake state");
                if i.revision != revision
                    || i.epoch != epoch
                    || !i
                        .lease
                        .is_some_and(|t| t.elapsed() < Duration::from_secs(3))
                {
                    self.mic = None;
                    self.capture = None;
                    return;
                }
            }
            if self.frame(inner, &samples, rate, revision, epoch) {
                return;
            }
        }
    }
    fn frame(
        &mut self,
        inner: &Mutex<Inner>,
        samples: &[f32],
        rate: u32,
        revision: u64,
        epoch: u64,
    ) -> bool {
        {
            let i = inner.lock().expect("wake state");
            if i.revision != revision
                || i.epoch != epoch
                || !i.config.enabled
                || i.config.muted
                || !i
                    .lease
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(3))
            {
                self.capture = None;
                self.mic = None;
                return true;
            }
        }
        let rms = energy(samples);
        if let Some(capture) = &mut self.capture {
            match capture.accept(samples, rate) {
                End::Continue => {}
                end => {
                    let wav = if matches!(end, End::Submit) {
                        capture.wav(rate).ok()
                    } else {
                        None
                    };
                    self.capture = None;
                    self.mic = None;
                    self.detector.as_mut().expect("detector").reset();
                    self.cooldown = Some(Instant::now());
                    let mut i = inner.lock().expect("wake state");
                    if i.revision != revision
                        || i.epoch != epoch
                        || !i
                            .lease
                            .is_some_and(|t| t.elapsed() < Duration::from_secs(3))
                    {
                        return true;
                    }
                    if let Some(bytes) = wav {
                        self.sequence = self.sequence.wrapping_add(1);
                        i.pending = Some((self.sequence, bytes, Instant::now()));
                        i.status.request_id = Some(self.sequence);
                        i.status.phase = "ready".into();
                    } else {
                        i.status.phase = "paused".into();
                        i.status.message =
                            "No request heard. Say the wake phrase to try again.".into();
                    }
                    i.status.rms = 0.;
                    return true;
                }
            }
        } else if self
            .detector
            .as_mut()
            .expect("detector")
            .accept(rate, samples)
        {
            self.capture = Some(Capture::default());
            inner.lock().expect("wake state").status.message =
                "I'm listening. Speak your request; silence ends recording.".into();
        }
        let mut i = inner.lock().expect("wake state");
        if i.revision != revision
            || i.epoch != epoch
            || !i
                .lease
                .is_some_and(|t| t.elapsed() < Duration::from_secs(3))
        {
            self.capture = None;
            self.mic = None;
            return true;
        }
        i.status.phase = if self.capture.is_some() {
            "capturing"
        } else {
            "listening"
        }
        .into();
        if self.capture.is_none() {
            i.status.message = format!("Listening on this computer for {}.", i.config.phrase);
        }
        i.status.rms = rms;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Production has one inference worker. Parallel fixture constructors race
    // ONNX Runtime's process-global default LoggingManager on Linux. Keep the
    // complete model fixtures serial (including teardown), retaining assertions.
    static MODEL_FIXTURE: Mutex<()> = Mutex::new(());
    fn models() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/wake")
    }
    fn audio(name: &str) -> Vec<f32> {
        let mut reader = hound::WavReader::open(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(name),
        )
        .unwrap();
        assert_eq!(reader.spec().sample_rate, 16000);
        reader
            .samples::<i16>()
            .map(|v| v.unwrap() as f32 / 32768.)
            .collect()
    }
    fn detects(detector: &mut Detector, samples: &[f32]) -> bool {
        detects_rate(detector, samples, 16000)
    }
    fn detects_rate(detector: &mut Detector, samples: &[f32], rate: u32) -> bool {
        detector.reset();
        let _ = detector.accept(rate, &vec![0.; rate as usize / 2]);
        let mut detected = false;
        for chunk in samples.chunks(rate as usize / 50) {
            detected |= detector.accept(rate, chunk);
        }
        for _ in 0..40 {
            detected |= detector.accept(rate, &vec![0.; rate as usize / 50]);
        }
        detected
    }
    #[test]
    #[ignore = "private local human fixture; audio must not be committed"]
    fn local_human_wake_fixture() {
        let _models = MODEL_FIXTURE.lock().unwrap();
        let path = std::env::var("NOTDOT_WAKE_FIXTURE").expect("set private WAV path");
        let mut reader = hound::WavReader::open(path).unwrap();
        assert_eq!(reader.spec().sample_rate, 16000);
        assert_eq!(reader.spec().channels, 1);
        let samples: Vec<_> = reader
            .samples::<i16>()
            .map(|s| s.unwrap() as f32 / 32768.)
            .take(480001)
            .collect();
        assert!(samples.len() <= 480000);
        for gain in [1., 8., 32.] {
            let adjusted: Vec<_> = samples.iter().map(|s| (s * gain).clamp(-1., 1.)).collect();
            let mut detector = Detector::new(&models(), "Hey Ava").unwrap();
            let detected = detects(&mut detector, &adjusted);
            eprintln!("local human fixture gain={gain}, detected={detected}");
            assert!(detected, "human wake phrase missed at gain {gain}");
        }
    }
    #[test]
    fn bundled_detector_hears_custom_phrases_and_rejects_unrelated_audio() {
        let _models = MODEL_FIXTURE.lock().unwrap();
        let mut d = Detector::new(&models(), &phrase("Ava", false).unwrap()).unwrap();
        assert!(detects(&mut d, &audio("hey-ava-uk.wav")));
        let at_48k: Vec<_> = audio("hey-ava.wav")
            .into_iter()
            .flat_map(|s| [s; 3])
            .collect();
        assert!(detects_rate(&mut d, &at_48k, 48000));
        assert!(
            detects(&mut d, &audio("hey-ava.wav")),
            "Hey Ava fixture must activate"
        );
        assert!(!detects(&mut d, &audio("negative.wav")));
        assert!(
            !detects(&mut d, &audio("ava.wav")),
            "name alone must not activate Hey Ava"
        );
        let mut name_only = Detector::new(&models(), &phrase("Ava", true).unwrap()).unwrap();
        assert!(detects(&mut name_only, &audio("ava.wav")));
        let mut renamed = Detector::new(&models(), &phrase("Orion", false).unwrap()).unwrap();
        assert!(
            !detects(&mut renamed, &audio("hey-ava.wav")),
            "old name must stop activating after rename"
        );
    }
    #[test]
    fn real_keyword_to_capture_to_one_shot_wav_excludes_background_audio() {
        let _models = MODEL_FIXTURE.lock().unwrap();
        let wake = isolated();
        wake.inner.lock().unwrap().pending = None;
        let mut worker = Worker {
            detector: Some(Detector::new(&models(), "Hey Ava").unwrap()),
            ..Worker::default()
        };
        let mut input = vec![0.; 8000];
        input.extend(audio("hey-ava.wav"));
        input.extend(vec![0.; 16000]);
        for frame in input.chunks(320) {
            worker.frame(&wake.inner, frame, 16000, 1, 0);
            if worker.capture.is_some() {
                break;
            }
        }
        assert_eq!(wake.status().phase, "capturing");
        assert!(wake.inner.lock().unwrap().pending.is_none());
        assert!(
            worker.capture.as_ref().unwrap().samples.is_empty(),
            "no keyword or background pre-roll is retained"
        );
        for _ in 0..20 {
            assert!(!worker.frame(&wake.inner, &[0.1; 320], 16000, 1, 0));
        }
        for _ in 0..59 {
            assert!(!worker.frame(&wake.inner, &[0.; 320], 16000, 1, 0));
        }
        assert!(worker.frame(&wake.inner, &[0.; 320], 16000, 1, 0));
        let id = wake.status().request_id.unwrap();
        let bytes = wake.take(id).unwrap();
        let reader = hound::WavReader::new(Cursor::new(bytes)).unwrap();
        let samples: Vec<i16> = reader.into_samples().map(Result::unwrap).collect();
        assert_eq!(samples.len(), 80 * 320);
        assert!(samples[..20 * 320].iter().all(|v| *v == 3276));
        assert!(samples[20 * 320..].iter().all(|v| *v == 0));
        assert!(wake.take(id).is_err());
        assert!(worker.capture.is_none());
        assert!(worker.cooldown.is_some());
    }
    #[test]
    fn endpoint_requires_speech_then_silence_and_bounds_recording() {
        let mut capture = Capture::default();
        for _ in 0..100 {
            assert!(matches!(capture.accept(&[0.; 320], 16000), End::Continue));
        }
        for _ in 0..20 {
            assert!(matches!(capture.accept(&[0.1; 320], 16000), End::Continue));
        }
        for _ in 0..59 {
            assert!(matches!(capture.accept(&[0.; 320], 16000), End::Continue));
        }
        assert!(matches!(capture.accept(&[0.; 320], 16000), End::Submit));
        let wav = capture.wav(16000).unwrap();
        let reader = hound::WavReader::new(Cursor::new(wav)).unwrap();
        assert_eq!(reader.spec().sample_rate, 16000);
        assert_eq!(reader.len() as usize, capture.samples.len());
        let mut empty = Capture::default();
        assert!(matches!(
            empty.accept(&vec![0.; 16000 * 8], 16000),
            End::Empty
        ));
        let mut cue_only = Capture::default();
        assert!(matches!(
            cue_only.accept(&[0.1; 1600], 16000),
            End::Continue
        ));
        assert!(
            matches!(cue_only.accept(&vec![0.; 126400], 16000), End::Empty),
            "an activation tone without a spoken request must not reach STT"
        );
        let mut continuous = Capture::default();
        for _ in 0..29 {
            assert!(matches!(
                continuous.accept(&[0.1; 16000], 16000),
                End::Continue
            ));
        }
        assert!(matches!(
            continuous.accept(&[0.1; 16000], 16000),
            End::Submit
        ));
    }
    fn isolated() -> Wake {
        Wake {
            inner: Arc::new(Mutex::new(Inner {
                config: Config {
                    enabled: true,
                    muted: false,
                    phrase: "Hey Ava".into(),
                },
                revision: 1,
                epoch: 0,
                lease: Some(Instant::now()),
                status: Status::default(),
                pending: Some((42, vec![1, 2, 3], Instant::now())),
            })),
        }
    }
    #[test]
    fn mute_pause_expiry_and_rename_discard_pending_audio_and_takes_are_one_shot() {
        let wake = isolated();
        assert!(wake.take(41).is_err());
        assert_eq!(wake.take(42).unwrap(), vec![1, 2, 3]);
        assert!(wake.take(42).is_err());
        let wake = isolated();
        wake.ready(false);
        wake.ready(true);
        assert!(wake.take(42).is_err());
        let wake = isolated();
        wake.configure(&Settings {
            wake_enabled: true,
            microphone_muted: true,
            ..Settings::default()
        });
        wake.ready(true);
        assert!(wake.take(42).is_err());
        let wake = isolated();
        wake.configure(&Settings {
            name: "Orion".into(),
            wake_enabled: true,
            ..Settings::default()
        });
        assert!(wake.take(42).is_err());
        let wake = isolated();
        wake.inner.lock().unwrap().lease = Some(Instant::now() - Duration::from_secs(4));
        assert!(wake.take(42).is_err());
        let wake = isolated();
        wake.inner.lock().unwrap().pending.as_mut().unwrap().2 =
            Instant::now() - Duration::from_secs(11);
        assert!(wake.take(42).is_err());
    }
    #[test]
    fn phrase_validation_prevents_keyword_directive_injection() {
        assert_eq!(phrase(" Ava ", false).unwrap(), "Hey Ava");
        assert_eq!(phrase("Mary-Jane", true).unwrap(), "Mary Jane");
        for name in ["", "Ava #0.01", "Ava @other", "Ava\0", "艾娃"] {
            assert!(phrase(name, false).is_err());
        }
    }
}
