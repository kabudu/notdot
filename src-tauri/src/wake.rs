//! On-device keyword spotting. One worker owns inference and microphone capture;
//! the real-time callback only downmixes into a bounded queue. No network access.
mod fallback;
mod speech;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use notdot_core::settings::Settings;
use serde::Serialize;
use sherpa_onnx::{KeywordSpotter, KeywordSpotterConfig, OnlineStream};
use std::{
    io::Cursor,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
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
pub enum CaptureEvent {
    Start {
        id: u64,
        token: String,
        rate: u32,
        manual: bool,
    },
    Audio {
        token: String,
        sequence: u64,
        pcm: Vec<u8>,
    },
    End {
        token: String,
        valid: bool,
        sequence: u64,
    },
    Cancel {
        token: String,
    },
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
    pub capture_token: Option<String>,
    pub manual: bool,
    pub started_ms: u64,
    pub muted: bool,
    pub audio_frames: u64,
    pub peak_rms: f32,
    pub candidates: u64,
    pub rejected_energy: u64,
    pub rejected_prefix: u64,
    pub fallback_decoder: String,
}
struct Inner {
    config: Config,
    revision: u64,
    epoch: u64,
    suspended: Option<u64>,
    released: Option<u64>,
    release_failed: Option<u64>,
    lease: Option<Instant>,
    status: Status,
    pending: Option<(u64, Vec<u8>, Instant)>,
    manual_token: Option<String>,
    finish_requested: bool,
    manual_limit: u32,
    cancelled: notdot_core::request_cancel::Requests,
}
pub struct Wake {
    inner: Arc<Mutex<Inner>>,
}
// RAII restores permission for future frontend leases after any failed install.
// It deliberately never restores stale capture tokens or opens the microphone.
pub struct DecoderRelease {
    inner: Arc<Mutex<Inner>>,
    token: u64,
}
impl Drop for DecoderRelease {
    fn drop(&mut self) {
        let mut i = self.inner.lock().expect("wake state");
        if i.suspended == Some(self.token) {
            i.suspended = None;
            i.released = None;
            i.release_failed = None;
        }
    }
}
impl Wake {
    pub fn new(
        models: PathBuf,
        settings: &Settings,
        events: tokio::sync::mpsc::Sender<CaptureEvent>,
    ) -> Self {
        let config = config(settings);
        let inner = Arc::new(Mutex::new(Inner {
            status: Status::default(),
            config,
            revision: 1,
            epoch: 0,
            suspended: None,
            released: None,
            release_failed: None,
            lease: None,
            pending: None,
            manual_token: None,
            finish_requested: false,
            manual_limit: 60,
            cancelled: Default::default(),
        }));
        let weak = Arc::downgrade(&inner);
        std::thread::Builder::new()
            .name("notdot-wake".into())
            .spawn(move || {
                let mut worker = Worker {
                    events: Some(events),
                    ..Default::default()
                };
                while let Some(inner) = weak.upgrade() {
                    worker.tick(&models, &inner);
                    let delay = {
                        let i = inner.lock().expect("wake state");
                        if (i.config.enabled || i.manual_token.is_some())
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
        i.manual_token = None;
        i.finish_requested = false;
        i.revision = i.revision.wrapping_add(1);
        i.epoch = i.epoch.wrapping_add(1);
        i.lease = None;
        i.pending = None;
        i.status.request_id = None;
    }
    pub fn ready(&self, ready: bool) {
        let mut i = self.inner.lock().expect("wake state");
        if ready {
            if i.suspended.is_some() {
                return;
            }
            i.lease = Some(Instant::now());
        } else {
            // A delayed idle heartbeat must not cancel a newer manual click.
            // Manual capture is cancelled through its scoped identity or lease expiry.
            if i.manual_token.is_some() {
                return;
            }
            if i.lease.is_some() {
                i.epoch = i.epoch.wrapping_add(1);
            }
            i.lease = None;
            i.manual_token = None;
            i.pending = None;
            i.status.request_id = None;
        }
    }
    pub fn pause_all(&self) {
        let mut i = self.inner.lock().expect("wake state");
        i.epoch = i.epoch.wrapping_add(1);
        i.lease = None;
        i.manual_token = None;
        i.pending = None;
        i.status.request_id = None;
    }
    fn begin_decoder_release(&self) -> Result<DecoderRelease, String> {
        let mut i = self.inner.lock().expect("wake state");
        if i.suspended.is_some() {
            return Err("Listening is already paused for an update".into());
        }
        if i.manual_token.is_some() || i.status.phase == "capturing" {
            return Err("Finish recording before updating".into());
        }
        i.epoch = i.epoch.wrapping_add(1);
        let token = i.epoch;
        i.suspended = Some(token);
        i.released = None;
        i.release_failed = None;
        i.lease = None;
        i.pending = None;
        i.status.request_id = None;
        Ok(DecoderRelease {
            inner: self.inner.clone(),
            token,
        })
    }
    pub async fn release_decoder_for_update(&self) -> Result<DecoderRelease, String> {
        let guard = self.begin_decoder_release()?;
        let release = async {
            loop {
                {
                    let i = self.inner.lock().expect("wake state");
                    if i.release_failed == Some(guard.token) {
                        return Err(
                            "The wake helper could not close safely. Restart notdot before updating."
                                .to_owned(),
                        );
                    }
                    if i.released == Some(guard.token) {
                        return Ok(());
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        // Includes a possible initial three-second helper setup and child reap.
        tokio::time::timeout(Duration::from_secs(6), release)
            .await
            .map_err(|_| "Listening could not stop safely. Try the update again.".to_owned())??;
        Ok(guard)
    }
    pub fn start_manual(&self, token: &str, limit: u32) -> Result<(), String> {
        notdot_core::request_cancel::Requests::validate(token)?;
        if ![12, 60].contains(&limit) {
            return Err("Invalid recording limit".into());
        }
        let mut i = self.inner.lock().expect("wake state");
        if i.suspended.is_some() {
            return Err("Listening is paused while installing an update".into());
        }
        if i.config.muted || i.cancelled.is_cancelled(token) {
            return Err("Voice capture cancelled or muted".into());
        }
        i.manual_token = Some(token.to_owned());
        i.manual_limit = limit;
        i.finish_requested = false;
        i.pending = None;
        i.status.request_id = None;
        i.epoch = i.epoch.wrapping_add(1);
        i.lease = Some(Instant::now());
        Ok(())
    }
    pub fn finish_manual(&self, token: &str) -> Result<(), String> {
        let mut i = self.inner.lock().expect("wake state");
        if i.manual_token.as_deref() != Some(token) {
            return Err("Recording is no longer active".into());
        }
        i.finish_requested = true;
        Ok(())
    }
    pub fn cancel_capture(&self, token: &str) -> Result<(), String> {
        let mut i = self.inner.lock().expect("wake state");
        i.cancelled.cancel(token)?;
        let active = if let Some(active) = &i.manual_token {
            active == token
        } else {
            i.status.capture_token.as_deref() == Some(token)
        };
        if active {
            i.lease = None;
            i.manual_token = None;
            i.pending = None;
            i.status.request_id = None;
            i.epoch = i.epoch.wrapping_add(1);
        }
        Ok(())
    }
    pub fn take(&self, id: u64) -> Result<Vec<u8>, String> {
        let mut i = self.inner.lock().expect("wake state");
        if (!i.config.enabled && i.manual_token.is_none())
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
        i.manual_token = None;
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
    fallback: fallback::Fallback,
    kws: KeywordSpotter,
    stream: OnlineStream,
    require_hey: bool,
    candidates: u64,
    rejected_energy: u64,
    rejected_prefix: u64,
    elapsed: f64,
    voice_gate: VoiceGate,
    voiced: std::collections::VecDeque<(f64, f64, bool)>,
}
impl Detector {
    fn new(models: &Path, phrase: &str) -> Result<Self, String> {
        let fallback = fallback::Fallback::new(&models.join("ggml-tiny.en-q5_1.bin"), phrase)?;
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
            fallback,
            kws,
            stream,
            require_hey,
            candidates: 0,
            rejected_energy: 0,
            rejected_prefix: 0,
            elapsed: 0.,
            voice_gate: VoiceGate::default(),
            voiced: Default::default(),
        })
    }
    fn accept(&mut self, rate: u32, samples: &[f32]) -> bool {
        if self.fallback.accept(rate, samples) {
            return true;
        }
        // Bound the native feature history too. Prefer resetting in quiet audio.
        if self.elapsed >= 120. || (self.elapsed >= 60. && energy(samples) < 0.008) {
            self.reset_keyword();
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
                self.candidates = self.candidates.saturating_add(1);
                if !supported {
                    self.rejected_energy = self.rejected_energy.saturating_add(1);
                }
                if !prefix {
                    self.rejected_prefix = self.rejected_prefix.saturating_add(1);
                }
                if supported && prefix {
                    return true;
                }
            }
        }
        false
    }

    fn reset(&mut self) {
        self.fallback.reset();
        self.reset_keyword();
    }
    fn reset_keyword(&mut self) {
        self.stream = self.kws.create_stream();
        self.elapsed = 0.;
        self.voiced.clear();
    }
}

// Two seconds of bounded headroom tolerate batched remote-audio callbacks.
// Packets are consumed immediately; this capacity adds no deliberate wait.
const MIC_QUEUE_PACKETS: usize = 100;
const MIC_QUEUE_FULL: u8 = 1;
const MIC_DEVICE_FAILED: u8 = 2;

// A slow worker is not a disconnected microphone: consume queued input before
// consulting its last observed timestamp. Queue overflow is checked separately.
fn receive_microphone_frame(
    receiver: &Receiver<Vec<f32>>,
    last_audio: &mut Option<Instant>,
) -> Result<Option<Vec<f32>>, String> {
    match receiver.try_recv() {
        Ok(samples) => {
            *last_audio = Some(Instant::now());
            Ok(Some(samples))
        }
        Err(_) if last_audio.is_some_and(|at| at.elapsed() > Duration::from_secs(2)) => {
            Err("Microphone stopped supplying audio. Save wake settings to retry.".into())
        }
        Err(_) => Ok(None),
    }
}

struct Microphone {
    _stream: cpal::Stream,
    receiver: Receiver<Vec<f32>>,
    errors: Receiver<cpal::Error>,
    failed: Arc<AtomicU8>,
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
        let (sender, receiver) = mpsc::sync_channel(MIC_QUEUE_PACKETS);
        let failed = Arc::new(AtomicU8::new(0));
        let (error_sender, errors) = mpsc::sync_channel(1);
        if !matches!(
            format,
            cpal::SampleFormat::F32 | cpal::SampleFormat::I16 | cpal::SampleFormat::U16
        ) {
            return Err("Unsupported microphone sample format".into());
        }
        let stream = build_capture_stream(&cfg, cfg!(target_os = "windows"), |candidate| {
            match format {
                cpal::SampleFormat::F32 => input::<f32>(&device, candidate, sender.clone(), failed.clone(), error_sender.clone()),
                cpal::SampleFormat::I16 => input::<i16>(&device, candidate, sender.clone(), failed.clone(), error_sender.clone()),
                cpal::SampleFormat::U16 => input::<u16>(&device, candidate, sender.clone(), failed.clone(), error_sender.clone()),
                _ => unreachable!("sample format validated above"),
            }
        }).map_err(|_| "Microphone could not start. Check OS microphone permission and the selected input device.")?;
        stream.play().map_err(|_| "Microphone could not start")?;
        Ok(Self {
            _stream: stream,
            receiver,
            errors,
            failed,
            rate: cfg.sample_rate,
        })
    }
}
fn build_capture_stream<T>(
    config: &cpal::StreamConfig,
    windows: bool,
    mut build: impl FnMut(&cpal::StreamConfig) -> Result<T, cpal::Error>,
) -> Result<T, cpal::Error> {
    if !windows {
        return build(config);
    }
    // WASAPI's reported software buffer range is its callback period, not the
    // ring capacity. Keep that period and the default format; request 100 ms of
    // ring headroom so remote-input scheduling jitter does not discard speech.
    let mut preferred = *config;
    preferred.buffer_size = cpal::BufferSize::Fixed(config.sample_rate.div_ceil(10));
    match build(&preferred) {
        Err(error) if error.kind() == cpal::ErrorKind::UnsupportedConfig => build(config),
        result => result,
    }
}
fn input<T>(
    device: &cpal::Device,
    cfg: &cpal::StreamConfig,
    sender: SyncSender<Vec<f32>>,
    failed: Arc<AtomicU8>,
    error_sender: SyncSender<cpal::Error>,
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
            if failed.load(Ordering::Relaxed) != 0 {
                return;
            }
            if queue_input(samples, channels, frame_size, &mut frame, &sender).is_err() {
                failed.store(MIC_QUEUE_FULL, Ordering::Relaxed);
            }
        },
        move |cause| {
            if !fatal_input_notification(cause.kind()) {
                return;
            }
            let _ = error_sender.try_send(cause);
            error.store(MIC_DEVICE_FAILED, Ordering::Relaxed);
        },
        None,
    )
}

/// CPAL continues the stream after these advisory notifications. They are not
/// evidence of device loss; stopping here made remote microphones unusable.
/// Our own bounded-queue overflow remains fatal because it drops known packets.
fn fatal_input_notification(kind: cpal::ErrorKind) -> bool {
    !matches!(
        kind,
        cpal::ErrorKind::RealtimeDenied | cpal::ErrorKind::Xrun | cpal::ErrorKind::DeviceChanged
    )
}

/// Preserve every mono sample in fixed 20 ms packets, including callback tails.
/// An overflow rejects the whole capture rather than silently dropping speech.
fn queue_input<T>(
    samples: &[T],
    channels: usize,
    frame_size: usize,
    frame: &mut Vec<f32>,
    sender: &SyncSender<Vec<f32>>,
) -> Result<(), mpsc::TrySendError<Vec<f32>>>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    use cpal::Sample;
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
            let packet = std::mem::replace(frame, Vec::with_capacity(frame_size));
            sender.try_send(packet)?;
        }
    }
    Ok(())
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
    limit_secs: Option<u32>,
    total: usize,
    speech: usize,
    silence: usize,
}
enum End {
    Continue,
    Submit,
    Empty,
}
impl Capture {
    fn accept(&mut self, samples: &[f32], rate: u32, voiced: bool) -> End {
        self.total += samples.len();
        if voiced {
            self.speech += samples.len();
            self.silence = 0;
        } else {
            self.silence += samples.len();
        }
        self.samples.extend_from_slice(samples);
        if self.speech >= rate as usize / 5 && self.silence >= rate as usize * 6 / 5 {
            return End::Submit;
        }
        if self.total >= (rate as usize * self.limit_secs.unwrap_or(30) as usize).min(4_000_000) {
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
    speech_gate: Option<speech::SpeechGate>,
    mic: Option<Microphone>,
    capture: Option<Capture>,
    failed: bool,
    sequence: u64,
    cooldown: Option<Instant>,
    last_audio: Option<Instant>,
    events: Option<tokio::sync::mpsc::Sender<CaptureEvent>>,
    capture_token: Option<String>,
    stream_packet: Vec<u8>,
    stream_sequence: u64,
}
impl Worker {
    fn event(&self, event: CaptureEvent) {
        if let Some(sender) = &self.events {
            let _ = sender.try_send(event);
        }
    }
    fn begin_capture(
        &mut self,
        inner: &Mutex<Inner>,
        rate: u32,
        revision: u64,
        epoch: u64,
    ) -> bool {
        let mut i = inner.lock().expect("wake state");
        if i.revision != revision
            || i.epoch != epoch
            || i.config.muted
            || (!i.config.enabled && i.manual_token.is_none())
            || !i
                .lease
                .is_some_and(|t| t.elapsed() < Duration::from_secs(3))
        {
            return false;
        }
        self.sequence = self.sequence.wrapping_add(1);
        let manual = i.manual_token.is_some();
        let token = i
            .manual_token
            .clone()
            .unwrap_or_else(|| format!("wake-{}", self.sequence));
        self.capture_token = Some(token.clone());
        self.stream_sequence = 0;
        self.stream_packet.clear();
        self.capture = Some(Capture {
            limit_secs: Some(if manual { i.manual_limit } else { 30 }),
            ..Default::default()
        });
        self.speech_gate
            .as_mut()
            .expect("speech gate initialized")
            .reset();
        i.status.capture_token = Some(token.clone());
        i.status.manual = manual;
        i.status.started_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;
        i.status.phase = "capturing".into();
        i.status.message = "I'm listening. Speak your request; silence ends recording.".into();
        drop(i);
        self.event(CaptureEvent::Start {
            id: self.sequence,
            token,
            rate,
            manual,
        });
        true
    }
    fn stream_audio(&mut self, samples: &[f32], rate: u32) {
        for sample in samples {
            self.stream_packet
                .extend_from_slice(&((sample.clamp(-1., 1.) * 32767.) as i16).to_le_bytes());
        }
        if self.stream_packet.len() >= rate as usize / 10 * 2 {
            self.flush_stream();
        }
    }
    fn flush_stream(&mut self) {
        if self.stream_packet.is_empty() {
            return;
        }
        if let Some(token) = &self.capture_token {
            let event = CaptureEvent::Audio {
                token: token.clone(),
                sequence: self.stream_sequence,
                pcm: std::mem::take(&mut self.stream_packet),
            };
            self.event(event);
            self.stream_sequence = self.stream_sequence.wrapping_add(1);
        }
    }
    fn discard_capture(&mut self) {
        if let Some(token) = self.capture_token.take() {
            self.event(CaptureEvent::Cancel { token });
        }
        self.stream_packet.clear();
        if self.capture.take().is_some()
            && let Some(gate) = &mut self.speech_gate
        {
            gate.reset();
        }
    }
    fn error(&mut self, inner: &Mutex<Inner>, message: String) {
        self.mic = None;
        self.failed = true;
        {
            let mut i = inner.lock().expect("wake state");
            i.status.phase = "error".into();
            i.status.message = message;
            i.status.rms = 0.;
            i.pending = None;
        }
        // Publish the reason before notifying the frontend that capture ended.
        self.discard_capture();
    }
    fn tick(&mut self, models: &Path, inner: &Mutex<Inner>) {
        let suspension = inner.lock().expect("wake state").suspended;
        if let Some(token) = suspension {
            self.mic = None;
            self.discard_capture();
            // Fallback Drop cancels its inference worker, then kills/reaps its child.
            // Acknowledge only after all package/model handles have been released.
            let released = self
                .detector
                .take()
                .is_none_or(|mut detector| detector.fallback.shutdown())
                && notdot_wake_runtime::client::all_helpers_reaped();
            self.speech_gate = None;
            let mut i = inner.lock().expect("wake state");
            if i.suspended == Some(token) {
                if released && i.release_failed != Some(token) {
                    i.released = Some(token);
                } else {
                    i.release_failed = Some(token);
                }
                i.status.phase = "paused".into();
                i.status.message = "Listening is paused while notdot updates.".into();
                i.status.rms = 0.;
                i.status.fallback_decoder.clear();
            }
            return;
        }
        let (config, revision, epoch, allowed, pending, manual) = {
            let i = inner.lock().expect("wake state");
            (
                i.config.clone(),
                i.revision,
                i.epoch,
                i.lease
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(3)),
                i.pending.is_some(),
                i.manual_token.is_some(),
            )
        };
        if epoch != self.epoch {
            self.epoch = epoch;
            if manual {
                self.failed = false;
            }
            self.mic = None;
            self.discard_capture();
            if let Some(d) = &mut self.detector {
                d.reset();
            }
        }
        if revision != self.revision {
            self.revision = revision;
            self.detector = None;
            self.speech_gate = None;
            self.mic = None;
            self.discard_capture();
            self.failed = false;
            let mut i = inner.lock().expect("wake state");
            i.status = Status {
                phrase: config.phrase.clone(),
                muted: config.muted,
                ..Status::default()
            };
        }
        if (!config.enabled && !manual)
            || config.muted
            || !allowed
            || pending
            || self
                .cooldown
                .is_some_and(|t| t.elapsed() < Duration::from_millis(1800))
                && !manual
        {
            if self.mic.take().is_some()
                && let Some(d) = &mut self.detector
            {
                d.reset();
            }
            self.discard_capture();
            let mut i = inner.lock().expect("wake state");
            if !allowed {
                i.pending = None;
                i.status.request_id = None;
            }
            if !self.failed {
                i.status.phase = if !config.enabled && !manual {
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
        if self.detector.is_none() && !manual {
            inner.lock().expect("wake state").status.phase = "loading".into();
            match Detector::new(models, &config.phrase) {
                Ok(d) => self.detector = Some(d),
                Err(e) => {
                    self.error(inner, e);
                    return;
                }
            }
        }
        if self.speech_gate.is_none() {
            match speech::SpeechGate::new(models) {
                Ok(gate) => self.speech_gate = Some(gate),
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
        if manual
            && self.capture.is_none()
            && !self.begin_capture(
                inner,
                self.mic.as_ref().expect("mic initialized").rate,
                revision,
                epoch,
            )
        {
            self.mic = None;
            return;
        }
        let mic = self.mic.as_ref().expect("mic initialized");
        match mic.failed.load(Ordering::Relaxed) {
            MIC_QUEUE_FULL => {
                self.error(inner, "Microphone audio arrived faster than it could be processed. Try again; if it persists, check the input device or remote connection.".into());
                return;
            }
            MIC_DEVICE_FAILED => {
                let detail = mic
                    .errors
                    .try_recv()
                    .ok()
                    .map(|error| error.to_string().chars().take(200).collect::<String>())
                    .unwrap_or_else(|| "device unavailable".into());
                self.error(inner, format!("The microphone connection stopped: {detail}. Check the input device or remote connection, then try again."));
                return;
            }
            _ => {}
        }
        let rate = mic.rate;
        for _ in 0..16 {
            let samples = match receive_microphone_frame(
                &self.mic.as_ref().expect("mic initialized").receiver,
                &mut self.last_audio,
            ) {
                Ok(Some(samples)) => samples,
                Ok(None) => break,
                Err(error) => {
                    self.error(inner, error);
                    return;
                }
            };
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
                    self.discard_capture();
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
                || (!i.config.enabled && i.manual_token.is_none())
                || i.config.muted
                || !i
                    .lease
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(3))
            {
                self.capture = None;
                if let Some(gate) = &mut self.speech_gate {
                    gate.reset();
                }
                self.mic = None;
                return true;
            }
        }
        if let Some(error) = self
            .detector
            .as_ref()
            .and_then(|d| d.fallback.error.clone())
        {
            self.error(inner, error);
            return true;
        }
        let rms = energy(samples);
        if self.capture.is_some() {
            self.stream_audio(samples, rate);
            let voiced = match self
                .speech_gate
                .as_mut()
                .expect("speech gate initialized")
                .accept(samples, rate)
            {
                Ok(voiced) => voiced,
                Err(e) => {
                    self.error(inner, e);
                    return true;
                }
            };
            let capture = self.capture.as_mut().expect("capture initialized");
            let mut end = capture.accept(samples, rate, voiced);
            if inner.lock().expect("wake state").finish_requested {
                end = if capture.speech >= rate as usize / 5 {
                    End::Submit
                } else {
                    End::Empty
                };
            }
            match end {
                End::Continue => {}
                end => {
                    let wav = if matches!(end, End::Submit) {
                        capture.wav(rate).ok()
                    } else {
                        None
                    };
                    self.flush_stream();
                    let token = self.capture_token.take().expect("active capture token");
                    self.discard_capture();
                    self.mic = None;
                    if let Some(detector) = &mut self.detector {
                        detector.reset();
                    }
                    self.cooldown = Some(Instant::now());
                    let mut i = inner.lock().expect("wake state");
                    if i.revision != revision
                        || i.epoch != epoch
                        || !i
                            .lease
                            .is_some_and(|t| t.elapsed() < Duration::from_secs(3))
                    {
                        drop(i);
                        self.event(CaptureEvent::Cancel { token });
                        return true;
                    }
                    if let Some(bytes) = wav {
                        i.pending = Some((self.sequence, bytes, Instant::now()));
                        i.status.request_id = Some(self.sequence);
                        i.status.phase = "ready".into();
                    } else {
                        i.manual_token = None;
                        i.status.phase = "paused".into();
                        i.status.message =
                            "No request heard. Say the wake phrase to try again.".into();
                    }
                    i.status.rms = 0.;
                    i.finish_requested = false;
                    let valid = i.pending.is_some();
                    drop(i);
                    self.event(CaptureEvent::End {
                        token,
                        valid,
                        sequence: self.stream_sequence,
                    });
                    return true;
                }
            }
        } else if self
            .detector
            .as_mut()
            .expect("detector")
            .accept(rate, samples)
            && !self.begin_capture(inner, rate, revision, epoch)
        {
            self.mic = None;
            return true;
        }
        let mut i = inner.lock().expect("wake state");
        if i.revision != revision
            || i.epoch != epoch
            || !i
                .lease
                .is_some_and(|t| t.elapsed() < Duration::from_secs(3))
        {
            self.discard_capture();
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
        i.status.audio_frames = i.status.audio_frames.saturating_add(1);
        i.status.peak_rms = i.status.peak_rms.max(rms);
        if let Some(detector) = &self.detector {
            i.status.candidates = detector.candidates;
            i.status.rejected_energy = detector.rejected_energy;
            i.status.rejected_prefix = detector.rejected_prefix;
            i.status.fallback_decoder = detector.fallback.backend().into();
        }
        false
    }
}

// Cross-compiled test binaries need resources on the machine running the tests.
// This override is test-only; production always uses Tauri's bundled resources.
#[cfg(test)]
fn fixture_root() -> PathBuf {
    std::env::var_os("NOTDOT_TEST_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}
// Production has one inference owner. Model fixtures share this test-only gate:
// Linux ONNX logging is process-global, and Windows helper reap accounting and
// decode deadlines must not be distorted by unrelated concurrent fixtures.
#[cfg(test)]
fn model_fixture_guard() -> std::sync::MutexGuard<'static, ()> {
    static FIXTURES: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
    FIXTURES
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("model fixture")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires the isolated Linux PulseAudio null-sink lab"]
    fn isolated_linux_null_sink_reaches_native_capture_and_releases() {
        use std::process::{Command, Stdio};
        assert_eq!(
            std::env::var("NOTDOT_LINUX_AUDIO_TEST").as_deref(),
            Ok("disposable-null")
        );
        assert_eq!(std::env::var("HOME").as_deref(), Ok("/home/qa"));
        let source = Command::new("pactl")
            .arg("get-default-source")
            .output()
            .unwrap();
        assert!(source.status.success());
        assert_eq!(
            String::from_utf8(source.stdout).unwrap().trim(),
            "notdot_qa_sink.monitor",
            "refusing capture outside the isolated null monitor"
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tone.wav");
        let mut wav = hound::WavWriter::create(
            &path,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for sample in 0..16000 {
            wav.write_sample(
                (0.25
                    * f32::sin(std::f32::consts::TAU * 800.0 * sample as f32 / 16000.0)
                    * i16::MAX as f32) as i16,
            )
            .unwrap();
        }
        wav.finalize().unwrap();
        struct Playback(std::process::Child);
        impl Drop for Playback {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        for _ in 0..2 {
            let microphone = Microphone::open().unwrap();
            let _playback = Playback(
                Command::new("paplay")
                    .arg("--device=notdot_qa_sink")
                    .arg(&path)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap(),
            );
            let deadline = Instant::now() + Duration::from_secs(8);
            let mut peak = 0.0_f32;
            let mut samples = 0;
            while Instant::now() < deadline && samples < microphone.rate as usize / 4 {
                assert_eq!(microphone.failed.load(Ordering::Relaxed), 0);
                if let Ok(frame) = microphone.receiver.recv_timeout(Duration::from_millis(100)) {
                    let frame_peak = frame.iter().copied().map(f32::abs).fold(0.0_f32, f32::max);
                    peak = peak.max(frame_peak);
                    if frame_peak > 0.05 {
                        samples += frame.len();
                    }
                }
            }
            assert!(
                peak > 0.05 && samples >= microphone.rate as usize / 4,
                "native capture did not receive the generated null-sink tone"
            );
            assert_eq!(microphone.failed.load(Ordering::Relaxed), 0);
            drop(microphone);
        }
    }
    #[test]
    fn queued_microphone_frames_refresh_a_stale_worker_timer_before_timeout() {
        let (sender, receiver) = mpsc::sync_channel(2);
        let mut last_audio = Some(Instant::now() - Duration::from_secs(3));
        sender.send(vec![0.1; 320]).unwrap();
        assert_eq!(
            receive_microphone_frame(&receiver, &mut last_audio)
                .unwrap()
                .unwrap()
                .len(),
            320
        );
        assert!(last_audio.unwrap().elapsed() < Duration::from_secs(1));
        assert!(
            receive_microphone_frame(&receiver, &mut last_audio)
                .unwrap()
                .is_none()
        );
        // A truly empty stream still fails at the original two-second bound.
        last_audio = Some(Instant::now() - Duration::from_millis(2100));
        assert!(
            receive_microphone_frame(&receiver, &mut last_audio)
                .unwrap_err()
                .contains("stopped supplying audio")
        );
    }

    #[test]
    fn windows_capture_headroom_preserves_format_and_rounds_up_to_one_tenth_second() {
        for rate in [8000_u32, 11025, 44100, 48000, 192000] {
            let config = cpal::StreamConfig {
                channels: 2,
                sample_rate: rate,
                buffer_size: cpal::BufferSize::Default,
            };
            let mut calls = 0;
            build_capture_stream(&config, true, |requested| {
                calls += 1;
                assert_eq!(requested.channels, config.channels);
                assert_eq!(requested.sample_rate, config.sample_rate);
                let cpal::BufferSize::Fixed(frames) = requested.buffer_size else {
                    panic!("headroom must be requested");
                };
                assert!(frames * 10 >= rate);
                assert!((frames - 1) * 10 < rate);
                Ok(())
            })
            .unwrap();
            assert_eq!(calls, 1);
            assert_eq!(config.buffer_size, cpal::BufferSize::Default);
        }
    }
    #[test]
    fn capture_headroom_falls_back_once_only_for_a_rejected_configuration() {
        let config = cpal::StreamConfig {
            channels: 2,
            sample_rate: 44100,
            buffer_size: cpal::BufferSize::Default,
        };
        let mut attempts = Vec::new();
        let result: Result<(), _> = build_capture_stream(&config, true, |requested| {
            attempts.push(*requested);
            Err(cpal::Error::with_message(
                if attempts.len() == 1 {
                    cpal::ErrorKind::UnsupportedConfig
                } else {
                    cpal::ErrorKind::PermissionDenied
                },
                "synthetic build rejection",
            ))
        });
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0].buffer_size, cpal::BufferSize::Fixed(4410));
        assert_eq!(attempts[1], config);
        assert_eq!(
            result.unwrap_err().kind(),
            cpal::ErrorKind::PermissionDenied
        );
        let mut calls = 0;
        let stream = build_capture_stream(&config, true, |requested| {
            calls += 1;
            if calls == 1 {
                Err(cpal::Error::with_message(
                    cpal::ErrorKind::UnsupportedConfig,
                    "synthetic buffer rejection",
                ))
            } else {
                assert_eq!(*requested, config);
                Ok("default stream")
            }
        })
        .unwrap();
        assert_eq!(calls, 2);
        assert_eq!(stream, "default stream");
        for kind in [
            cpal::ErrorKind::PermissionDenied,
            cpal::ErrorKind::DeviceBusy,
            cpal::ErrorKind::HostUnavailable,
            cpal::ErrorKind::BackendError,
        ] {
            let mut calls = 0;
            let result: Result<(), _> = build_capture_stream(&config, true, |_| {
                calls += 1;
                Err(cpal::Error::with_message(kind, "synthetic device failure"))
            });
            assert_eq!(calls, 1);
            assert_eq!(result.unwrap_err().kind(), kind);
        }
    }
    #[test]
    fn other_platforms_keep_the_original_buffer_and_do_not_retry() {
        for buffer_size in [cpal::BufferSize::Default, cpal::BufferSize::Fixed(512)] {
            let config = cpal::StreamConfig {
                channels: 1,
                sample_rate: 48000,
                buffer_size,
            };
            let mut calls = 0;
            let result: Result<(), _> = build_capture_stream(&config, false, |requested| {
                calls += 1;
                assert_eq!(*requested, config);
                Err(cpal::Error::with_message(
                    cpal::ErrorKind::UnsupportedConfig,
                    "synthetic unsupported input",
                ))
            });
            assert!(result.is_err());
            assert_eq!(calls, 1);
        }
    }
    #[test]
    fn recoverable_audio_notifications_do_not_disconnect_the_microphone() {
        use cpal::ErrorKind;
        for advisory in [
            ErrorKind::Xrun,
            ErrorKind::RealtimeDenied,
            ErrorKind::DeviceChanged,
        ] {
            assert!(!fatal_input_notification(advisory));
        }
        for fatal in [
            ErrorKind::DeviceNotAvailable,
            ErrorKind::PermissionDenied,
            ErrorKind::HostUnavailable,
            ErrorKind::StreamInvalidated,
        ] {
            assert!(fatal_input_notification(fatal));
        }
    }
    #[test]
    fn remote_audio_burst_keeps_all_samples_and_callback_tails() {
        let (sender, receiver) = mpsc::sync_channel(MIC_QUEUE_PACKETS);
        let mut frame = Vec::with_capacity(320);
        // A one-second stereo callback exceeded the previous 320 ms queue.
        queue_input(&vec![0.25_f32; 32010], 2, 320, &mut frame, &sender).unwrap();
        let packets: Vec<_> = receiver.try_iter().collect();
        assert_eq!(packets.len(), 50);
        assert!(packets.iter().flatten().all(|&s| s == 0.25));
        assert_eq!(frame, vec![0.25; 5]);
        queue_input(&vec![0.5_f32; 630], 2, 320, &mut frame, &sender).unwrap();
        let packet = receiver.try_recv().unwrap();
        assert_eq!(&packet[..5], &[0.25; 5]);
        assert_eq!(&packet[5..], &[0.5; 315]);
        assert!(frame.is_empty());
    }
    #[test]
    fn microphone_backlog_stays_bounded_and_fails_instead_of_dropping_audio() {
        let (sender, receiver) = mpsc::sync_channel(MIC_QUEUE_PACKETS);
        let mut frame = Vec::new();
        let result = queue_input(
            &vec![0.1_f32; 320 * (MIC_QUEUE_PACKETS + 1)],
            1,
            320,
            &mut frame,
            &sender,
        );
        assert!(matches!(result, Err(mpsc::TrySendError::Full(_))));
        assert_eq!(receiver.try_iter().count(), MIC_QUEUE_PACKETS);
    }
    fn models() -> PathBuf {
        super::fixture_root().join("resources/wake")
    }
    fn audio(name: &str) -> Vec<f32> {
        let mut reader =
            hound::WavReader::open(super::fixture_root().join("tests/fixtures").join(name))
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
        for _ in 0..350 {
            if detected || !detector.fallback.is_busy() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
            detected |= detector.accept(rate, &vec![0.; rate as usize / 50]);
        }
        detected
    }
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "explicit local microphone lease; actual worker heartbeat/status only, no providers or retention"]
    fn linux_actual_worker_with_ready_lease_metadata_diagnostic() {
        assert_eq!(
            std::env::var("NOTDOT_LINUX_WAKE_MIC_DIAGNOSTIC").as_deref(),
            Ok("1")
        );
        let _models = model_fixture_guard();
        let settings = Settings {
            name: "ava".into(),
            wake_enabled: true,
            ..Settings::default()
        };
        let (sender, mut receiver) = tokio::sync::mpsc::channel(128);
        let wake = Wake::new(models(), &settings, sender);
        let utc_ms = || {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        };
        let start = Instant::now();
        let mut started = 0u64;
        let mut audio = 0u64;
        let mut ended = 0u64;
        let mut cancelled = 0u64;
        let mut last_report = Instant::now();
        let mut announced_ready = false;
        while start.elapsed() < Duration::from_secs(10) {
            wake.ready(true);
            while let Ok(event) = receiver.try_recv() {
                match event {
                    CaptureEvent::Start { .. } => started += 1,
                    CaptureEvent::Audio { .. } => audio += 1,
                    CaptureEvent::End { .. } => ended += 1,
                    CaptureEvent::Cancel { .. } => cancelled += 1,
                }
            }
            let status = wake.status();
            if status.phase == "listening" && !announced_ready {
                eprintln!("wake_worker_ready utc_ms={}", utc_ms());
                announced_ready = true;
            }
            if last_report.elapsed() >= Duration::from_millis(500) {
                eprintln!(
                    "wake_worker_metrics utc_ms={} phase={} frames={} rms={} peak_rms={} candidates={} rejected_energy={} rejected_prefix={} fallback_backend={} starts={} audio_events={} ends={} cancels={} error={}",
                    utc_ms(),
                    status.phase,
                    status.audio_frames,
                    status.rms,
                    status.peak_rms,
                    status.candidates,
                    status.rejected_energy,
                    status.rejected_prefix,
                    status.fallback_decoder,
                    started,
                    audio,
                    ended,
                    cancelled,
                    status.phase == "error"
                );
                last_report = Instant::now();
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        wake.ready(false);
        wake.pause_all();
        let cleanup = Instant::now();
        while cleanup.elapsed() < Duration::from_secs(2) && wake.status().phase != "paused" {
            std::thread::sleep(Duration::from_millis(20));
        }
        eprintln!(
            "wake_worker_finished utc_ms={} phase={} starts={} audio_events={} ends={} cancels={}",
            utc_ms(),
            wake.status().phase,
            started,
            audio,
            ended,
            cancelled
        );
        assert!(announced_ready, "actual worker never reached listening");
        assert_eq!(
            wake.status().phase,
            "paused",
            "actual worker did not acknowledge lease shutdown"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "explicit local microphone lease; aggregate diagnostics only, no audio retention or providers"]
    fn linux_microphone_to_detector_metadata_diagnostic() {
        assert_eq!(
            std::env::var("NOTDOT_LINUX_WAKE_MIC_DIAGNOSTIC").as_deref(),
            Ok("1")
        );
        let _models = model_fixture_guard();
        let phrase =
            std::env::var("NOTDOT_WAKE_DIAGNOSTIC_PHRASE").unwrap_or_else(|_| "Hey ava".into());
        let mut detector = Detector::new(&models(), &phrase).unwrap();
        let microphone = Microphone::open().unwrap();
        let started = Instant::now();
        let utc_ms = || {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        };
        let mut frames = 0u64;
        let mut total_samples = 0usize;
        let mut sum_squares = 0f64;
        let mut peak_rms = 0f32;
        let mut matches = 0u64;
        let mut last_frame = None;
        let mut max_gap_ms = 0u128;
        let mut last_report = Instant::now();
        eprintln!(
            "wake_mic_ready utc_ms={} rate={} duration_ms=10000",
            utc_ms(),
            microphone.rate
        );
        while started.elapsed() < Duration::from_secs(10) {
            if let Ok(samples) = microphone.receiver.recv_timeout(Duration::from_millis(50)) {
                let now = Instant::now();
                if let Some(last) = last_frame {
                    max_gap_ms = max_gap_ms.max(now.duration_since(last).as_millis());
                }
                last_frame = Some(now);
                frames += 1;
                total_samples += samples.len();
                sum_squares += samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>();
                peak_rms = peak_rms.max(energy(&samples));
                if detector.accept(microphone.rate, &samples) {
                    matches += 1;
                }
            }
            if last_report.elapsed() >= Duration::from_millis(500) {
                eprintln!(
                    "wake_mic_metrics utc_ms={} frames={} rms={} peak_frame_rms={} max_gap_ms={} candidates={} rejected_energy={} rejected_prefix={} fallback_busy={} fallback_backend={} fallback_error={} mic_failed={} matches={}",
                    utc_ms(),
                    frames,
                    (sum_squares / total_samples.max(1) as f64).sqrt(),
                    peak_rms,
                    max_gap_ms,
                    detector.candidates,
                    detector.rejected_energy,
                    detector.rejected_prefix,
                    detector.fallback.is_busy(),
                    detector.fallback.backend(),
                    detector.fallback.error.is_some(),
                    microphone.failed.load(Ordering::Relaxed),
                    matches
                );
                last_report = Instant::now();
            }
        }
        assert_eq!(
            microphone.failed.load(Ordering::Relaxed),
            0,
            "native microphone failed"
        );
        assert!(frames > 0, "native microphone supplied no frames");
        eprintln!(
            "wake_mic_finished utc_ms={} frames={} matches={} candidates={} rejected_energy={} rejected_prefix={} fallback_busy={} fallback_error={}",
            utc_ms(),
            frames,
            matches,
            detector.candidates,
            detector.rejected_energy,
            detector.rejected_prefix,
            detector.fallback.is_busy(),
            detector.fallback.error.is_some()
        );
    }

    #[test]
    #[ignore = "private local human fixture; audio must not be committed"]
    fn local_human_wake_fixture() {
        let _models = model_fixture_guard();
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
    fn wake_after_extended_idle_and_rejected_candidates() {
        let _models = model_fixture_guard();
        for idle in [2, 5, 25, 65] {
            let mut d = Detector::new(&models(), "Hey Ava").unwrap();
            for _ in 0..idle * 50 {
                assert!(!d.accept(16000, &[0.; 320]));
            }
            for name in ["hey-ava.wav", "negative.wav"] {
                let mut detected = false;
                for chunk in audio(name).chunks(320) {
                    detected |= d.accept(16000, chunk);
                }
                for _ in 0..60 {
                    detected |= d.accept(16000, &[0.; 320]);
                }
                for _ in 0..400 {
                    if !d.fallback.is_busy() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                    detected |= d.accept(16000, &[0.; 320]);
                }
                assert_eq!(detected, name == "hey-ava.wav", "{name} after {idle}s idle");
                if detected {
                    d.reset();
                }
            }
        }
    }
    #[test]
    fn bundled_detector_hears_custom_phrases_and_rejects_unrelated_audio() {
        let _models = model_fixture_guard();
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
        let _models = model_fixture_guard();
        let wake = isolated();
        wake.inner.lock().unwrap().pending = None;
        let (events, mut stream_events) = tokio::sync::mpsc::channel(64);
        let mut worker = Worker {
            events: Some(events),
            detector: Some(Detector::new(&models(), "Hey Ava").unwrap()),
            speech_gate: Some(speech::SpeechGate::new(&models()).unwrap()),
            ..Worker::default()
        };
        let mut input = vec![0.; 8000];
        input.extend(audio("hey-ava.wav"));
        input.extend(vec![0.; 16000]);
        // Model setup can outlast an idle lease on a cold runner. Drive the
        // frontend readiness boundary while feeding audio, as the live UI does.
        wake.inner.lock().unwrap().lease = Some(Instant::now() - Duration::from_secs(4));
        for frame in input.chunks(320) {
            wake.ready(true);
            worker.frame(&wake.inner, frame, 16000, 1, 0);
            if worker.capture.is_some() {
                break;
            }
        }
        assert_eq!(wake.status().phase, "capturing");
        assert!(matches!(
            stream_events.try_recv().unwrap(),
            CaptureEvent::Start { .. }
        ));
        assert!(
            stream_events.try_recv().is_err(),
            "background and wake audio must not be streamed"
        );
        assert!(wake.inner.lock().unwrap().pending.is_none());
        assert!(
            worker.capture.as_ref().unwrap().samples.is_empty(),
            "no keyword or background pre-roll is retained"
        );
        let request = audio("negative.wav");
        for frame in request.chunks(320) {
            wake.ready(true);
            assert!(!worker.frame(&wake.inner, frame, 16000, 1, 0));
        }
        let mut submitted = false;
        for _ in 0..100 {
            wake.ready(true);
            if worker.frame(&wake.inner, &[0.; 320], 16000, 1, 0) {
                submitted = true;
                break;
            }
        }
        assert!(submitted);
        wake.ready(true);
        let id = wake
            .status()
            .request_id
            .expect("completed request with a live lease");
        let bytes = wake.take(id).unwrap();
        let reader = hound::WavReader::new(Cursor::new(bytes)).unwrap();
        let samples: Vec<i16> = reader.into_samples().map(Result::unwrap).collect();
        assert!(samples.len() >= request.len());
        assert!(
            samples
                .iter()
                .zip(&request)
                .all(|(a, b)| (*a as f32 / 32768. - b).abs() < 0.0001),
            "captured request must retain its original samples without the wake phrase"
        );
        assert!(wake.take(id).is_err());
        assert!(worker.capture.is_none());
        assert!(worker.cooldown.is_some());
        let mut next = 0;
        let mut streamed = 0;
        let mut ended = false;
        while let Ok(event) = stream_events.try_recv() {
            match event {
                CaptureEvent::Audio { sequence, pcm, .. } => {
                    assert_eq!(sequence, next);
                    next += 1;
                    streamed += pcm.len();
                }
                CaptureEvent::End {
                    sequence, valid, ..
                } => {
                    assert_eq!(sequence, next);
                    assert!(valid);
                    ended = true;
                }
                _ => panic!("unexpected stream control event"),
            }
        }
        assert!(ended);
        assert_eq!(streamed, samples.len() * 2);
    }
    #[test]
    fn endpoint_requires_speech_then_silence_and_bounds_recording() {
        let mut capture = Capture::default();
        for _ in 0..100 {
            assert!(matches!(
                capture.accept(&[0.; 320], 16000, false),
                End::Continue
            ));
        }
        for _ in 0..20 {
            assert!(matches!(
                capture.accept(&[0.1; 320], 16000, true),
                End::Continue
            ));
        }
        for _ in 0..59 {
            assert!(matches!(
                capture.accept(&[0.; 320], 16000, false),
                End::Continue
            ));
        }
        assert!(matches!(
            capture.accept(&[0.; 320], 16000, false),
            End::Submit
        ));
        let wav = capture.wav(16000).unwrap();
        let reader = hound::WavReader::new(Cursor::new(wav)).unwrap();
        assert_eq!(reader.spec().sample_rate, 16000);
        assert_eq!(reader.len() as usize, capture.samples.len());
        let mut empty = Capture::default();
        assert!(matches!(
            empty.accept(&vec![0.; 16000 * 8], 16000, false),
            End::Empty
        ));
        let mut cue_only = Capture::default();
        assert!(matches!(
            cue_only.accept(&[0.1; 1600], 16000, false),
            End::Continue
        ));
        assert!(
            matches!(cue_only.accept(&vec![0.; 126400], 16000, false), End::Empty),
            "an activation tone without a spoken request must not reach STT"
        );
        let mut continuous = Capture::default();
        for _ in 0..29 {
            assert!(matches!(
                continuous.accept(&[0.1; 16000], 16000, true),
                End::Continue
            ));
        }
        assert!(matches!(
            continuous.accept(&[0.1; 16000], 16000, true),
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
                suspended: None,
                released: None,
                release_failed: None,
                lease: Some(Instant::now()),
                status: Status::default(),
                manual_token: None,
                manual_limit: 60,
                finish_requested: false,
                cancelled: Default::default(),
                pending: Some((42, vec![1, 2, 3], Instant::now())),
            })),
        }
    }
    #[test]
    fn update_release_waits_for_worker_ack_and_failed_install_requires_fresh_lease() {
        let _models = model_fixture_guard();
        let wake = isolated();
        let guard = wake.begin_decoder_release().unwrap();
        let token = guard.token;
        assert_eq!(wake.inner.lock().unwrap().released, None);
        wake.ready(true);
        assert!(wake.inner.lock().unwrap().lease.is_none());
        assert!(wake.start_manual("during-update", 60).is_err());
        let mut worker = Worker::default();
        // No model/microphone access is allowed on the suspended path.
        worker.tick(Path::new("/deliberately-missing-models"), &wake.inner);
        assert_eq!(wake.inner.lock().unwrap().released, Some(token));
        assert!(worker.detector.is_none());
        assert!(worker.mic.is_none());
        // The installation hook may run only after the acknowledgement above.
        let failed_install = || -> Result<(), ()> {
            assert_eq!(wake.inner.lock().unwrap().released, Some(token));
            Err(())
        };
        assert!(failed_install().is_err());
        drop(guard);
        assert!(wake.inner.lock().unwrap().lease.is_none());
        assert!(wake.inner.lock().unwrap().manual_token.is_none());
        wake.ready(true);
        assert!(wake.inner.lock().unwrap().lease.is_some());
    }
    #[test]
    fn unconfirmed_decoder_release_never_becomes_an_ack_on_a_later_empty_tick() {
        let _models = model_fixture_guard();
        let wake = isolated();
        let guard = wake.begin_decoder_release().unwrap();
        wake.inner.lock().unwrap().release_failed = Some(guard.token);
        let mut worker = Worker::default();
        worker.tick(Path::new("/missing-models"), &wake.inner);
        worker.tick(Path::new("/missing-models"), &wake.inner);
        let i = wake.inner.lock().unwrap();
        assert_eq!(i.release_failed, Some(guard.token));
        assert!(i.released.is_none());
    }
    #[test]
    fn failed_install_preserves_new_mute_and_cancellation_and_never_restarts_recording() {
        let wake = isolated();
        let guard = wake.begin_decoder_release().unwrap();
        wake.pause_all();
        wake.inner.lock().unwrap().config.muted = true;
        drop(guard);
        let i = wake.inner.lock().unwrap();
        assert!(i.config.muted);
        assert!(i.lease.is_none());
        assert!(i.manual_token.is_none());
        drop(i);
        let wake = isolated();
        wake.inner.lock().unwrap().status.phase = "capturing".into();
        assert!(wake.inner.lock().unwrap().manual_token.is_none());
        assert!(wake.begin_decoder_release().is_err());
        assert_eq!(wake.inner.lock().unwrap().status.phase, "capturing");
        let wake = isolated();
        wake.start_manual("still-recording", 60).unwrap();
        assert!(wake.begin_decoder_release().is_err());
        assert_eq!(
            wake.inner.lock().unwrap().manual_token.as_deref(),
            Some("still-recording")
        );
    }
    #[test]
    fn microphone_failure_keeps_its_reason_when_the_frontend_cancels_cleanup() {
        let wake = isolated();
        wake.start_manual("broken", 60).unwrap();
        wake.inner.lock().unwrap().status.capture_token = Some("broken".into());
        let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
        let mut worker = Worker {
            events: Some(sender),
            capture_token: Some("broken".into()),
            capture: Some(Capture::default()),
            ..Default::default()
        };
        worker.error(&wake.inner, "native input failed".into());
        assert!(
            matches!(receiver.try_recv().unwrap(), CaptureEvent::Cancel { token } if token == "broken")
        );
        wake.cancel_capture("broken").unwrap();
        let status = wake.status();
        assert_eq!(status.phase, "error");
        assert_eq!(status.message, "native input failed");
        assert_eq!(status.capture_token.as_deref(), Some("broken"));
        assert!(worker.capture.is_none());
        assert!(worker.failed);
    }
    #[test]
    fn manual_capture_ignores_stale_idle_heartbeats_and_old_cancellation() {
        let wake = isolated();
        wake.configure(&Settings {
            wake_enabled: false,
            ..Default::default()
        });
        wake.start_manual("old", 60).unwrap();
        wake.ready(false);
        assert_eq!(
            wake.inner.lock().unwrap().manual_token.as_deref(),
            Some("old")
        );
        wake.cancel_capture("old").unwrap();
        assert!(wake.start_manual("old", 60).is_err());
        wake.start_manual("new", 60).unwrap();
        wake.cancel_capture("old").unwrap();
        assert_eq!(
            wake.inner.lock().unwrap().manual_token.as_deref(),
            Some("new")
        );
        assert!(wake.finish_manual("old").is_err());
        wake.finish_manual("new").unwrap();
        wake.pause_all();
        assert!(wake.inner.lock().unwrap().manual_token.is_none());
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
