//! Wake test tool: run the shared spotter over a stream of clips, either
//! decoded directly (`offline`) or played through the default speakers and
//! captured by the default microphone (`acoustic`, macOS `afplay`).
//!
//! `cargo run -p notdot-wake-kws --example acoustic_wake -- <offline|acoustic> <models> <phrase> <clip.wav>...`
//!
//! Clips are 16-bit PCM WAV. Clips whose file name starts with `pos` should
//! wake; all others should not. Captured audio is processed in memory only
//! and never written. Plays one clip at a time with gaps.
use notdot_wake_kws::{Spotter, energy};
use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant},
};

const GAP: Duration = Duration::from_millis(2200);
const LEAD_IN: Duration = Duration::from_millis(1500);
/// A detection counts for a clip if it lands before the clip ends + this.
const TAIL: Duration = Duration::from_millis(1400);

struct Window {
    clip: PathBuf,
    start: Duration,
    end: Duration,
}

fn read_wav(path: &Path) -> (Vec<f32>, u32) {
    let mut reader = hound::WavReader::open(path).expect("wav");
    let spec = reader.spec();
    assert_eq!(spec.bits_per_sample, 16, "16-bit PCM clips only");
    let channels = usize::from(spec.channels);
    let samples: Vec<f32> = reader
        .samples::<i16>()
        .map(|s| f32::from(s.expect("sample")) / 32768.)
        .collect::<Vec<_>>()
        .chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect();
    (samples, spec.sample_rate)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [mode, models, phrase, clips @ ..] = args.as_slice() else {
        eprintln!("usage: acoustic_wake <offline|acoustic> <models> <phrase> <clip.wav>...");
        std::process::exit(2);
    };
    let mut spotter = Spotter::new(Path::new(models), phrase).expect("spotter");
    let clips: Vec<PathBuf> = clips.iter().map(PathBuf::from).collect();
    // (seconds since start, rms) per packet, and detection times.
    let mut detections = Vec::new();
    let mut windows = Vec::new();
    let mut levels: Vec<(Duration, f32)> = Vec::new();
    match mode.as_str() {
        "offline" => {
            let rate = 16_000;
            let mut stream: Vec<f32> =
                vec![0.; rate as usize * LEAD_IN.as_millis() as usize / 1000];
            for clip in &clips {
                let (samples, clip_rate) = read_wav(clip);
                assert_eq!(clip_rate, rate);
                let start = Duration::from_secs_f64(stream.len() as f64 / f64::from(rate));
                stream.extend(&samples);
                let end = Duration::from_secs_f64(stream.len() as f64 / f64::from(rate));
                windows.push(Window {
                    clip: clip.clone(),
                    start,
                    end,
                });
                stream.extend(std::iter::repeat_n(
                    0.,
                    rate as usize * GAP.as_millis() as usize / 1000,
                ));
            }
            for (index, packet) in stream.chunks(rate as usize / 50).enumerate() {
                let at = Duration::from_millis(index as u64 * 20);
                levels.push((at, energy(packet)));
                if spotter.accept(rate, packet) {
                    detections.push(at);
                    spotter.reset();
                }
            }
        }
        "acoustic" => acoustic(
            &mut spotter,
            &clips,
            &mut detections,
            &mut windows,
            &mut levels,
        ),
        _ => panic!("mode must be offline or acoustic"),
    }
    let mut failures = 0;
    for window in &windows {
        let hit = detections
            .iter()
            .any(|at| *at >= window.start && *at <= window.end + TAIL);
        let peak = levels
            .iter()
            .filter(|(at, _)| *at >= window.start && *at <= window.end)
            .map(|(_, rms)| *rms)
            .fold(0., f32::max);
        let name = window.clip.file_name().unwrap().to_string_lossy();
        let expect = name.starts_with("pos");
        if hit != expect {
            failures += 1;
        }
        println!(
            "{mode} {name}: woke={hit} expected={expect} peak_rms={peak:.4} {}",
            if hit == expect { "ok" } else { "MISMATCH" }
        );
    }
    let stray = detections
        .iter()
        .filter(|at| {
            !windows
                .iter()
                .any(|w| **at >= w.start && **at <= w.end + TAIL)
        })
        .count();
    println!(
        "{mode} summary: {} clips, {failures} mismatches, {stray} detections outside clip windows, {} candidates, {} rejected (energy), {} rejected (prefix)",
        windows.len(),
        spotter.candidates,
        spotter.rejected_energy,
        spotter.rejected_prefix
    );
}

fn acoustic(
    spotter: &mut Spotter,
    clips: &[PathBuf],
    detections: &mut Vec<Duration>,
    windows: &mut Vec<Window>,
    levels: &mut Vec<(Duration, f32)>,
) {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let device = cpal::default_host()
        .default_input_device()
        .expect("no default input device");
    let config = device.default_input_config().expect("input config");
    let rate = config.sample_rate();
    let channels = usize::from(config.channels());
    let (send, receive) = mpsc::sync_channel::<Vec<f32>>(512);
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(
            config.into(),
            move |data: &[f32], _| {
                let mono = data
                    .chunks(channels)
                    .map(|f| f.iter().sum::<f32>() / channels as f32)
                    .collect();
                let _ = send.try_send(mono);
            },
            |error| eprintln!("input error: {error}"),
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            config.into(),
            move |data: &[i16], _| {
                let mono = data
                    .chunks(channels)
                    .map(|f| {
                        f.iter().map(|s| f32::from(*s) / 32768.).sum::<f32>() / channels as f32
                    })
                    .collect();
                let _ = send.try_send(mono);
            },
            |error| eprintln!("input error: {error}"),
            None,
        ),
        other => panic!("unsupported input format {other:?}"),
    }
    .expect("input stream");
    stream.play().expect("start capture");
    let started = Instant::now();
    let player_clips = clips.to_vec();
    let (window_send, window_receive) = mpsc::channel();
    let player = std::thread::spawn(move || {
        std::thread::sleep(LEAD_IN);
        for clip in player_clips {
            let start = started.elapsed();
            let status = std::process::Command::new("/usr/bin/afplay")
                .arg(&clip)
                .status()
                .expect("afplay");
            assert!(status.success(), "afplay failed for {}", clip.display());
            let end = started.elapsed();
            window_send.send(Window { clip, start, end }).unwrap();
            std::thread::sleep(GAP);
        }
    });
    let mut pending: Vec<f32> = Vec::new();
    let packet = rate as usize / 50;
    let mut consumed = 0usize;
    while !player.is_finished() || !pending.is_empty() {
        match receive.recv_timeout(Duration::from_millis(50)) {
            Ok(chunk) => pending.extend(chunk),
            Err(_) if player.is_finished() => break,
            Err(_) => continue,
        }
        while pending.len() >= packet {
            let chunk: Vec<f32> = pending.drain(..packet).collect();
            let at = Duration::from_secs_f64(consumed as f64 / f64::from(rate));
            consumed += packet;
            levels.push((at, energy(&chunk)));
            if spotter.accept(rate, &chunk) {
                detections.push(at);
                spotter.reset();
            }
        }
    }
    drop(stream);
    windows.extend(window_receive.try_iter());
    eprintln!(
        "acoustic: device rate {rate} Hz, {channels} channel(s), {:.1} s captured",
        consumed as f64 / f64::from(rate)
    );
}
