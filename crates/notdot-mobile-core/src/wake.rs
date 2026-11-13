//! Wake detection with the desktop's detector: the keyword spotter plus,
//! when built with the `whisper` feature, the desktop's Whisper backup with
//! the same model and decode rules. Audio is processed in memory and never
//! stored or sent.
use crate::MobileError;
#[cfg(feature = "whisper")]
use crate::backup::Backup;
pub use crate::backup_types::{BackupState, BackupStats};
use notdot_wake_kws::Spotter;
use std::{path::Path, sync::Mutex};

/// Validate a name and build the wake phrase, exactly as the desktop does.
#[uniffi::export]
pub fn wake_phrase(name: String, name_only: bool) -> Result<String, MobileError> {
    notdot_wake_kws::phrase(&name, name_only)
        .map_err(|message| MobileError::Invalid { detail: message })
}

struct Inner {
    spotter: Spotter,
    #[cfg(feature = "whisper")]
    backup: Option<Backup>,
    rate: u32,
}

impl Inner {
    fn reset(&mut self) {
        self.spotter.reset();
        #[cfg(feature = "whisper")]
        if let Some(backup) = &mut self.backup {
            backup.reset();
        }
    }
}

#[derive(uniffi::Object)]
pub struct WakeEngine(Mutex<Inner>);

impl WakeEngine {
    fn build(
        models: &str,
        name: String,
        name_only: bool,
        backup: bool,
    ) -> Result<Self, MobileError> {
        let phrase = wake_phrase(name, name_only)?;
        let models = Path::new(models);
        let spotter = Spotter::new(models, &phrase)
            .map_err(|message| MobileError::Wake { detail: message })?;
        #[cfg(feature = "whisper")]
        let backup = if backup {
            Some(
                Backup::start(models, &phrase)
                    .map_err(|message| MobileError::Wake { detail: message })?,
            )
        } else {
            None
        };
        #[cfg(not(feature = "whisper"))]
        let _ = backup;
        Ok(Self(Mutex::new(Inner {
            spotter,
            #[cfg(feature = "whisper")]
            backup,
            rate: 0,
        })))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[uniffi::export]
impl WakeEngine {
    /// The full detector. `models` is the directory holding the bundled wake
    /// models (the same files as the desktop's `resources/wake`). The backup
    /// model loads in the background; until it is ready, or if it cannot
    /// load, the keyword spotter works alone (see `backup_state`).
    #[uniffi::constructor]
    pub fn new(models: String, name: String, name_only: bool) -> Result<Self, MobileError> {
        Self::build(&models, name, name_only, true)
    }

    /// The keyword spotter alone, for comparison and for low-power use.
    #[uniffi::constructor]
    pub fn spotter_only(
        models: String,
        name: String,
        name_only: bool,
    ) -> Result<Self, MobileError> {
        Self::build(&models, name, name_only, false)
    }

    /// Feed mono float samples at `rate` Hz; true once per detected wake.
    pub fn accept(&self, samples: Vec<f32>, rate: u32) -> bool {
        if !(8_000..=192_000).contains(&rate) || samples.len() > rate as usize * 2 {
            return false;
        }
        let mut inner = self.lock();
        // A route change (e.g. Bluetooth) can change the rate; the stream's
        // timing assumes one rate, so start afresh.
        if inner.rate != rate {
            inner.reset();
            inner.rate = rate;
        }
        // As on the desktop: the backup's verdict first, then the spotter.
        #[cfg(feature = "whisper")]
        let backup_hit = inner
            .backup
            .as_mut()
            .is_some_and(|backup| backup.accept(rate, &samples));
        #[cfg(not(feature = "whisper"))]
        let backup_hit = false;
        let hit = backup_hit || inner.spotter.accept(rate, &samples);
        if hit {
            inner.reset();
        }
        hit
    }

    pub fn reset(&self) {
        self.lock().reset();
    }

    pub fn backup_state(&self) -> BackupState {
        #[cfg(feature = "whisper")]
        if let Some(backup) = &self.lock().backup {
            return backup.state;
        }
        BackupState::Off
    }

    /// The backup's last error, in plain language, if any.
    pub fn backup_error(&self) -> Option<String> {
        #[cfg(feature = "whisper")]
        if let Some(backup) = &self.lock().backup {
            return backup.error.clone();
        }
        None
    }

    /// Whether a backup decode is running. Tests that feed recorded audio
    /// faster than real time pace it in real time while this is true, so
    /// the decoder sees what it would live.
    pub fn backup_busy(&self) -> bool {
        #[cfg(feature = "whisper")]
        if let Some(backup) = &self.lock().backup {
            return backup.is_busy();
        }
        false
    }

    pub fn backup_stats(&self) -> BackupStats {
        #[cfg(feature = "whisper")]
        if let Some(backup) = &self.lock().backup {
            return backup.stats.clone();
        }
        BackupStats::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn repo() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn clip(name: &str) -> (Vec<f32>, u32) {
        let mut reader =
            hound::WavReader::open(repo().join("src-tauri/tests/fixtures").join(name)).unwrap();
        let spec = reader.spec();
        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Int => reader
                .samples::<i32>()
                .map(|s| s.unwrap() as f32 / (1 << (spec.bits_per_sample - 1)) as f32)
                .collect(),
            hound::SampleFormat::Float => reader.samples::<f32>().map(Result::unwrap).collect(),
        };
        let channels = usize::from(spec.channels);
        (
            samples
                .chunks(channels)
                .map(|frame| frame.iter().sum::<f32>() / channels as f32)
                .collect(),
            spec.sample_rate,
        )
    }

    fn models() -> String {
        repo()
            .join("src-tauri/resources/wake")
            .to_string_lossy()
            .into_owned()
    }

    /// Wait (bounded) for the backup to finish loading.
    #[cfg(feature = "whisper")]
    fn ready(engine: &WakeEngine) -> BackupState {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while engine.backup_state() == BackupState::Loading && std::time::Instant::now() < until {
            engine.accept(Vec::new(), 16_000);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        engine.backup_state()
    }

    /// Feed `wake` (at `gain`) after other speech and a short pause, in
    /// 20 ms packets as a microphone callback would, then 0.8 s of silence.
    /// Packets go faster than real time except while a backup decode runs,
    /// when they are paced in real time, as they would arrive live; then
    /// silence continues while a decode is still running (at most 4 s). Returns whether it woke and, if so, the host
    /// time from the end of the clip to the wake.
    fn detects(engine: &WakeEngine, wake: &str, gain: f32) -> (bool, Option<std::time::Duration>) {
        let (before, rate) = clip("negative.wav");
        let (samples, wake_rate) = clip(wake);
        assert_eq!((rate, wake_rate), (16_000, 16_000));
        let packet = rate as usize / 50;
        engine.reset();
        let mut audio: Vec<f32> = before.iter().map(|s| s * 0.3).collect();
        audio.extend(std::iter::repeat_n(0., rate as usize / 5));
        audio.extend(samples.iter().map(|s| s * gain));
        let clip_end = audio.len();
        audio.extend(std::iter::repeat_n(0., packet * 40));
        let mut ended = None;
        for (index, chunk) in audio.chunks(packet).enumerate() {
            if index * packet >= clip_end && ended.is_none() {
                ended = Some(std::time::Instant::now());
            }
            if engine.backup_busy() {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            if engine.accept(chunk.to_vec(), rate) {
                return (true, ended.map(|t| t.elapsed()));
            }
        }
        let until = std::time::Instant::now() + std::time::Duration::from_secs(4);
        while engine.backup_busy() && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(20));
            if engine.accept(vec![0.; packet], rate) {
                return (true, ended.map(|t| t.elapsed()));
            }
        }
        (false, None)
    }

    const POSITIVES: [&str; 3] = ["hey-ava.wav", "hey-ava-uk.wav", "hey-ava-us.wav"];

    /// Spotter alone. Hard checks: the name alone and other speech never
    /// wake at any level, and the spotter path does wake on the corpus.
    /// Recall per clip is reported, not asserted: it differs by CPU (arm64
    /// detects all three clips at conversational level; x86_64 CI missed
    /// the UK clip) and quiet speech is missed without the backup (W1).
    #[test]
    fn the_spotter_alone_wakes_and_rejects_offline() {
        let engine = WakeEngine::spotter_only(models(), "Ava".into(), false).unwrap();
        assert_eq!(engine.backup_state(), BackupState::Off);
        let mut woke = Vec::new();
        for clip in POSITIVES {
            let (hit, _) = detects(&engine, clip, 0.35);
            eprintln!("spotter-only recall: {clip} at 0.35 -> {hit}");
            if hit {
                woke.push(clip);
            }
        }
        assert!(!woke.is_empty(), "the spotter path must wake on the corpus");
        for clip in ["ava.wav", "negative.wav"] {
            for gain in [0.05, 0.35, 0.8] {
                assert!(
                    !detects(&engine, clip, gain).0,
                    "{clip}@{gain} must not wake"
                );
            }
        }
    }

    /// The full detector (spotter plus the desktop's Whisper backup) on the
    /// desktop corpus at conversational and quiet levels. Host timings are
    /// printed for the evidence table; they are not phone performance.
    #[cfg(feature = "whisper")]
    #[test]
    fn the_full_detector_wakes_on_the_corpus_and_rejects_offline() {
        let engine = WakeEngine::new(models(), "Ava".into(), false).unwrap();
        assert_eq!(
            ready(&engine),
            BackupState::Ready,
            "{:?}",
            engine.backup_error()
        );
        for gain in [0.35, 0.05] {
            for clip in POSITIVES {
                let (hit, latency) = detects(&engine, clip, gain);
                // `None` latency: woke before the clip ended (the spotter).
                eprintln!("full detector: {clip} at {gain} -> {hit}, after clip end: {latency:?}");
                assert!(hit, "{clip}@{gain} must wake");
            }
        }
        for clip in ["ava.wav", "negative.wav"] {
            for gain in [0.05, 0.35, 0.8] {
                assert!(
                    !detects(&engine, clip, gain).0,
                    "{clip}@{gain} must not wake"
                );
            }
        }
        let stats = engine.backup_stats();
        eprintln!("backup stats: {stats:?}");
        assert!(stats.decodes > 0 && stats.failures == 0, "{stats:?}");
        assert!(stats.decode_ms_max < 3_000, "{stats:?}");
    }

    /// A missing backup model leaves the spotter working and says why.
    #[cfg(all(feature = "whisper", unix))]
    #[test]
    fn a_missing_backup_model_degrades_to_the_spotter() {
        let dir = std::env::temp_dir().join(format!("notdot-wake-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Every bundled file except the backup model (linked, not copied).
        for file in std::fs::read_dir(repo().join("src-tauri/resources/wake")).unwrap() {
            let file = file.unwrap();
            if file.file_name() != crate::backup::MODEL {
                std::os::unix::fs::symlink(file.path(), dir.join(file.file_name())).unwrap();
            }
        }
        let engine =
            WakeEngine::new(dir.to_string_lossy().into_owned(), "Ava".into(), false).unwrap();
        assert_eq!(ready(&engine), BackupState::Unavailable);
        assert!(engine.backup_error().is_some());
        assert!(!detects(&engine, "negative.wav", 0.35).0);
        drop(engine);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Reset and drop cancel a decode in progress rather than waiting for it.
    #[cfg(feature = "whisper")]
    #[test]
    fn reset_and_drop_cancel_a_running_decode() {
        let engine = WakeEngine::new(models(), "Ava".into(), false).unwrap();
        assert_eq!(ready(&engine), BackupState::Ready);
        let (speech, rate) = clip("negative.wav");
        let packet = rate as usize / 50;
        let mut audio: Vec<f32> = vec![0.; rate as usize];
        audio.extend(speech.iter().map(|s| s * 0.35));
        audio.extend(std::iter::repeat_n(0., rate as usize / 2));
        for chunk in audio.chunks(packet) {
            engine.accept(chunk.to_vec(), rate);
            if engine.backup_busy() {
                break;
            }
        }
        assert!(engine.backup_busy(), "a decode should be running");
        let started = std::time::Instant::now();
        engine.reset();
        drop(engine);
        let took = started.elapsed();
        eprintln!("reset + drop during a decode took {took:?}");
        assert!(took < std::time::Duration::from_secs(1), "{took:?}");
    }

    #[cfg(feature = "whisper")]
    fn read_clip(path: &std::path::Path) -> Vec<f32> {
        let mut reader = hound::WavReader::open(path).unwrap();
        assert_eq!(reader.spec().sample_rate, 16_000);
        reader
            .samples::<i16>()
            .map(|v| f32::from(v.unwrap()) / 32768.)
            .collect()
    }

    #[cfg(feature = "whisper")]
    /// Linear 16 to 48 kHz upsampling: phones usually capture at 48 kHz.
    fn to_48k(samples: &[f32]) -> Vec<f32> {
        let mut out = Vec::with_capacity(samples.len() * 3);
        for pair in samples.windows(2) {
            for step in 0..3 {
                out.push(pair[0] + (pair[1] - pair[0]) * step as f32 / 3.);
            }
        }
        out
    }

    #[cfg(feature = "whisper")]
    fn rss_kib() -> u64 {
        std::process::Command::new("/bin/ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output()
            .ok()
            .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
            .unwrap_or(0)
    }

    #[cfg(feature = "whisper")]
    /// Feed audio in 20 ms packets. While a backup decode runs, packets are
    /// paced in real time, so the decoder sees what it would live; otherwise
    /// faster. Returns the number of wakes.
    fn stream(engine: &WakeEngine, audio: &[f32], rate: u32) -> usize {
        let packet = rate as usize / 50;
        let mut wakes = 0;
        for chunk in audio.chunks(packet) {
            if engine.backup_busy() {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            wakes += usize::from(engine.accept(chunk.to_vec(), rate));
        }
        let until = std::time::Instant::now() + std::time::Duration::from_secs(4);
        while engine.backup_busy() && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(20));
            wakes += usize::from(engine.accept(vec![0.; packet], rate));
        }
        wakes
    }

    /// Gate W1 diagnostic on the actual mobile engine: the full detector
    /// versus the spotter alone on a folder of 16 kHz 16-bit WAV clips
    /// (names starting `pos` should wake), at 16 and 48 kHz input; then a
    /// continuous stream of the negative clips for false wakes and backup
    /// decode time per minute of speech. Host numbers, not phone ones.
    /// Clips are generated locally; nothing is played, recorded or kept.
    #[cfg(feature = "whisper")]
    #[test]
    #[ignore = "set NOTDOT_WAKE_CLIP_DIR to a folder of synthetic clips"]
    fn synthetic_clip_folder_diagnostic() {
        let Ok(dir) = std::env::var("NOTDOT_WAKE_CLIP_DIR") else {
            return;
        };
        let mut clips: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "wav"))
            .collect();
        clips.sort();
        let before = rss_kib();
        let full = WakeEngine::new(models(), "Ava".into(), false).unwrap();
        let loading = std::time::Instant::now();
        assert_eq!(ready(&full), BackupState::Ready);
        eprintln!(
            "backup ready after {:?}; process RSS {} -> {} KiB",
            loading.elapsed(),
            before,
            rss_kib()
        );
        let spotter = WakeEngine::spotter_only(models(), "Ava".into(), false).unwrap();
        for rate in [16_000u32, 48_000] {
            let (mut full_ok, mut spotter_ok, mut latencies) = (0, 0, Vec::new());
            for clip in &clips {
                let name = clip.file_name().unwrap().to_string_lossy().into_owned();
                let expect = name.starts_with("pos");
                let mut audio = vec![0.; 8_000];
                audio.extend(read_clip(clip));
                audio.extend(vec![0.; 16_000]);
                if rate == 48_000 {
                    audio = to_48k(&audio);
                }
                full.reset();
                spotter.reset();
                let started = std::time::Instant::now();
                let both = stream(&full, &audio, rate) > 0;
                let took = started.elapsed();
                let alone = stream(&spotter, &audio, rate) > 0;
                full_ok += usize::from(both == expect);
                spotter_ok += usize::from(alone == expect);
                if both && expect {
                    latencies.push(took);
                }
                eprintln!("{rate} Hz {name}: expected={expect} full={both} spotter_only={alone}");
            }
            eprintln!(
                "{rate} Hz: {} clips; full correct {full_ok}; spotter only correct {spotter_ok}; wall time per woken clip {latencies:?}",
                clips.len()
            );
        }
        // Continuous non-wake speech: every negative clip, repeated to about
        // ten minutes, with short gaps, as one stream at 48 kHz.
        let negatives: Vec<Vec<f32>> = clips
            .iter()
            .filter(|c| !c.file_name().unwrap().to_string_lossy().starts_with("pos"))
            .map(|c| read_clip(c))
            .collect();
        if negatives.is_empty() {
            return;
        }
        let mut speech = Vec::new();
        while speech.len() < 16_000 * 600 {
            for clip in &negatives {
                speech.extend_from_slice(clip);
                speech.extend(std::iter::repeat_n(0., 4_000));
            }
        }
        let speech = to_48k(&speech);
        let minutes = speech.len() as f64 / 48_000. / 60.;
        full.reset();
        let base = full.backup_stats();
        let wakes = stream(&full, &speech, 48_000);
        let stats = full.backup_stats();
        let decodes = stats.decodes - base.decodes;
        let ms = stats.decode_ms_total - base.decode_ms_total;
        eprintln!(
            "continuous negatives: {minutes:.1} min; false wakes {wakes}; backup decodes {decodes}; decode time {ms} ms ({:.1} s per minute of speech; max {} ms); RSS {} KiB",
            ms as f64 / 1000. / minutes,
            stats.decode_ms_max,
            rss_kib()
        );
    }

    #[test]
    fn invalid_names_and_audio_are_refused() {
        assert!(wake_phrase("Ava #0.01".into(), false).is_err());
        assert_eq!(wake_phrase("Ava".into(), false).unwrap(), "Hey Ava");
        assert!(WakeEngine::new("/nonexistent".into(), "Ava".into(), false).is_err());
    }
}
