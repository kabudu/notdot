//! Acoustic evaluation with the real CAM++ model. Ignored by default because it
//! needs the model file and a fixture directory; run with:
//!
//! ```text
//! NOTDOT_SPEAKER_MODEL=/path/3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx \
//! NOTDOT_SPEAKER_FIXTURES=/path/fixtures \
//! cargo test -p notdot-speakers --release --test acoustic -- --ignored --nocapture
//! ```
//!
//! Fixture layout: `<fixtures>/<speaker>/enrol-{0..3}.wav` and
//! `test-{0..7}.wav`, 16-bit mono. The speakers listed in `ENROLLED` are
//! enrolled through the real guided-enrolment code; all others are unknown.
//! The evidence set used for the release notes is synthetic (macOS system
//! voices rendered to files), so the figures describe synthetic voices only.
use notdot_speakers::{
    embedding::{CamPlusPlus, Extractor},
    engine::{Engine, Identification},
    enrol::{Enrolment, Progress},
    matcher::{Thresholds, Voiceprint},
};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::Instant,
};

/// Voices rendered by the same synthesiser engine with different parameters.
/// They stand in for very similar real voices (for example close relatives)
/// and are reported separately; they are not evidence about unrelated people.
const FAMILIES: [&[&str]; 3] = [
    &["eddy", "flo", "reed", "grandpa", "grandma"],
    &["aman", "rishi"],
    &["samantha", "karen"],
];
fn same_family(a: &str, b: &str) -> bool {
    FAMILIES.iter().any(|f| f.contains(&a) && f.contains(&b))
}

const ENROLLED: [&str; 8] = [
    "daniel", "karen", "moira", "rishi", "samantha", "tessa", "eddy", "flo",
];

fn read(path: &Path) -> (u32, Vec<f32>) {
    let mut r = hound::WavReader::open(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let spec = r.spec();
    assert_eq!((spec.channels, spec.bits_per_sample), (1, 16));
    let s = r
        .samples::<i16>()
        .map(|s| f32::from(s.unwrap()) / 32768.0)
        .collect();
    (spec.sample_rate, s)
}

/// Deterministic white noise at a target SNR against the voiced signal power.
fn with_noise(s: &[f32], snr_db: f32, seed: u64) -> Vec<f32> {
    let power = s
        .iter()
        .filter(|x| x.abs() > 0.008)
        .map(|x| x * x)
        .sum::<f32>()
        / s.iter().filter(|x| x.abs() > 0.008).count().max(1) as f32;
    let noise_rms = (power / 10f32.powf(snr_db / 10.0)).sqrt();
    let mut x = seed;
    s.iter()
        .map(|v| {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let u = ((x >> 33) as f32 / (1u64 << 31) as f32) * 2.0 - 1.0;
            (v + u * noise_rms * 1.732).clamp(-1.0, 1.0)
        })
        .collect()
}

/// Mix another talker underneath (competing speech).
fn with_talker(s: &[f32], other: &[f32], gain: f32) -> Vec<f32> {
    s.iter()
        .enumerate()
        .map(|(i, v)| {
            (v + gain * other.get(i % other.len()).copied().unwrap_or(0.0)).clamp(-1.0, 1.0)
        })
        .collect()
}

/// Crude loudspeaker-and-room replay channel: gain, 3-tap low-pass, short echo,
/// light noise. Not a substitute for a real playback test.
fn replay_channel(s: &[f32]) -> Vec<f32> {
    let lp: Vec<f32> = (0..s.len())
        .map(|i| {
            0.7 * (0.25 * s[i.saturating_sub(1)] + 0.5 * s[i] + 0.25 * s[(i + 1).min(s.len() - 1)])
        })
        .collect();
    let echoed: Vec<f32> = (0..lp.len())
        .map(|i| lp[i] + if i >= 480 { 0.3 * lp[i - 480] } else { 0.0 })
        .collect();
    with_noise(&echoed, 25.0, 99)
}

#[derive(Default, Debug)]
struct Tally {
    correct: usize,
    wrong: usize,
    ambiguous: usize,
    replay: usize,
    unknown: usize,
    unusable: usize,
}
impl Tally {
    fn add(&mut self, expected: Option<&str>, id: &Identification) {
        match id {
            Identification::Recognised { profile, .. } if Some(profile.as_str()) == expected => {
                self.correct += 1
            }
            Identification::Recognised { .. } => self.wrong += 1,
            Identification::Ambiguous => self.ambiguous += 1,
            Identification::SuspectedReplay => self.replay += 1,
            Identification::Unknown => self.unknown += 1,
            _ => self.unusable += 1,
        }
    }
    fn total(&self) -> usize {
        self.correct + self.wrong + self.ambiguous + self.replay + self.unknown + self.unusable
    }
}

#[test]
#[ignore = "needs NOTDOT_SPEAKER_MODEL and NOTDOT_SPEAKER_FIXTURES"]
fn acoustic_evaluation() {
    let model = PathBuf::from(std::env::var_os("NOTDOT_SPEAKER_MODEL").expect("model"));
    let root = PathBuf::from(std::env::var_os("NOTDOT_SPEAKER_FIXTURES").expect("fixtures"));
    let t0 = Instant::now();
    let ex: Arc<dyn Extractor> = Arc::new(CamPlusPlus::load(&model).expect("model loads"));
    println!(
        "model verified and loaded in {} ms",
        t0.elapsed().as_millis()
    );
    let thresholds = Thresholds::default();
    let mut speakers: Vec<String> = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().into_string().unwrap())
        .collect();
    speakers.sort();

    // Enrolment through the real guided flow.
    let mut prints: Vec<Voiceprint> = Vec::new();
    for name in ENROLLED {
        let (mut e, _) = Enrolment::start(name);
        let mut last = None;
        for i in 0..4 {
            let (rate, audio) = read(&root.join(name).join(format!("enrol-{i}.wav")));
            last = Some(e.submit(ex.as_ref(), rate, &audio));
            if let Some(Progress::Retry { issue, .. }) = &last {
                println!("enrol {name} sample {i}: retry {issue:?}");
            }
        }
        assert_eq!(last, Some(Progress::Ready { of: 4 }), "{name}");
        match e.finish(&prints, &thresholds) {
            Ok(p) => prints.push(p),
            Err(err) => println!("enrol {name}: refused {err:?}"),
        }
    }
    let enrolled: Vec<String> = prints.iter().map(|p| p.profile.clone()).collect();
    println!(
        "enrolled {}/{}: {enrolled:?}",
        enrolled.len(),
        ENROLLED.len()
    );
    let engine = Engine::new(ex.clone(), prints.clone(), thresholds);
    let go = AtomicBool::new(false);

    // Raw score distributions for calibration.
    let mut genuine = Vec::new();
    let mut impostor = Vec::new();
    let mut timings = Vec::new();
    let mut clips: Vec<(String, u32, Vec<f32>)> = Vec::new();
    for name in &speakers {
        for i in 0..8 {
            let (rate, audio) = read(&root.join(name).join(format!("test-{i}.wav")));
            clips.push((name.clone(), rate, audio));
        }
    }
    for (name, rate, audio) in &clips {
        let s = Instant::now();
        let e = engine.embed(*rate, audio, &go).expect("embedding");
        timings.push(s.elapsed().as_secs_f64() * 1000.0);
        for p in &prints {
            let score = p.score(&e);
            if &p.profile == name {
                genuine.push(score);
            } else {
                impostor.push(score);
            }
        }
    }
    let stats = |v: &mut Vec<f32>| {
        v.sort_by(f32::total_cmp);
        (v[0], v[v.len() / 2], v[v.len() - 1])
    };
    let (gmin, gmed, gmax) = stats(&mut genuine);
    let (imin, imed, imax) = stats(&mut impostor);
    println!(
        "genuine centroid cosine: n={} min={gmin:.3} median={gmed:.3} max={gmax:.3}",
        genuine.len()
    );
    println!(
        "impostor centroid cosine: n={} min={imin:.3} median={imed:.3} max={imax:.3}",
        impostor.len()
    );
    for accept in [0.40f32, 0.45, 0.50, 0.55, 0.60, 0.65, 0.70, 0.75] {
        let frr = genuine.iter().filter(|&&s| s < accept).count() as f32 / genuine.len() as f32;
        let far = impostor.iter().filter(|&&s| s >= accept).count() as f32 / impostor.len() as f32;
        println!(
            "  threshold {accept:.2}: score-level FRR {:.1}% FAR {:.2}%",
            frr * 100.0,
            far * 100.0
        );
    }
    timings.sort_by(f64::total_cmp);
    println!(
        "embedding time per test clip (1 thread): median {:.1} ms, p95 {:.1} ms, max {:.1} ms",
        timings[timings.len() / 2],
        timings[timings.len() * 95 / 100],
        timings[timings.len() - 1]
    );

    // Decisions under conditions.
    let run = |label: &str, f: &dyn Fn(&str, &[f32]) -> Vec<f32>| {
        let mut known = Tally::default();
        let mut unknown = Tally::default();
        let mut accepted: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        for (i, (name, rate, audio)) in clips.iter().enumerate() {
            let a = f(name, audio);
            let id = engine.identify(*rate, &a, &go);
            if enrolled.contains(name) {
                known.add(Some(name), &id);
            } else {
                unknown.add(None, &id);
                if let Identification::Recognised { profile, .. } = &id {
                    accepted
                        .entry(name.clone())
                        .or_default()
                        .push(profile.clone());
                }
            }
            let _ = i;
        }
        println!(
            "{label}:\n  enrolled speakers {known:?} (n={})\n  unknown speakers  {unknown:?} (n={})",
            known.total(),
            unknown.total()
        );
        if !accepted.is_empty() {
            println!("  unknown voices accepted as: {accepted:?}");
        }
        (known, unknown, accepted)
    };
    let (clean_known, clean_unknown, clean_accepts) = run("clean", &|_, a| a.to_vec());
    run("white noise 20 dB SNR", &|_, a| with_noise(a, 20.0, 7));
    run("white noise 10 dB SNR", &|_, a| with_noise(a, 10.0, 7));
    run("white noise 0 dB SNR", &|_, a| with_noise(a, 0.0, 7));
    let other = read(&root.join("fred").join("test-3.wav")).1;
    run("competing talker at -6 dB", &|_, a| {
        with_talker(a, &other, 0.5)
    });

    // Replay: verbatim enrolment clips, and enrolment clips through a channel.
    let mut verbatim = Tally::default();
    let mut channel = Tally::default();
    let mut new_phrase_replay = Tally::default();
    for name in &enrolled {
        for i in 0..4 {
            let (rate, audio) = read(&root.join(name).join(format!("enrol-{i}.wav")));
            verbatim.add(Some(name), &engine.identify(rate, &audio, &go));
            channel.add(
                Some(name),
                &engine.identify(rate, &replay_channel(&audio), &go),
            );
        }
        let (rate, audio) = read(&root.join(name).join("test-0.wav"));
        new_phrase_replay.add(
            Some(name),
            &engine.identify(rate, &replay_channel(&audio), &go),
        );
    }
    println!("replay of verbatim enrolment clips: {verbatim:?} (correct = NOT caught)");
    println!(
        "replay of enrolment clips through simulated speaker/room: {channel:?} (correct = NOT caught)"
    );
    println!(
        "replay of a different recorded phrase through simulated channel: {new_phrase_replay:?} (correct = NOT caught)"
    );

    // Minimum release bar on the synthetic set: no unrelated unknown voice is
    // accepted and no enrolled voice is confused with another on clean audio.
    // Same-engine "relatives" are reported above, not excused silently.
    let unrelated_accepts: usize = clean_accepts
        .iter()
        .map(|(voice, targets)| targets.iter().filter(|t| !same_family(voice, t)).count())
        .sum();
    println!(
        "clean unknown accepts: unrelated {unrelated_accepts}, same-engine {}",
        clean_unknown.wrong - unrelated_accepts
    );
    assert_eq!(
        unrelated_accepts, 0,
        "false accept of an unrelated unknown voice"
    );
    assert_eq!(clean_known.wrong, 0, "enrolled voice confused with another");
    assert!(
        verbatim.replay >= verbatim.total() * 9 / 10,
        "verbatim replay flag regressed"
    );
}

/// Pairwise cosine between single clips, for calibration diagnostics.
#[test]
#[ignore = "needs NOTDOT_SPEAKER_MODEL and NOTDOT_SPEAKER_FIXTURES"]
fn pairwise_diagnostic() {
    let model = PathBuf::from(std::env::var_os("NOTDOT_SPEAKER_MODEL").expect("model"));
    let root = PathBuf::from(std::env::var_os("NOTDOT_SPEAKER_FIXTURES").expect("fixtures"));
    let ex = CamPlusPlus::load(&model).expect("model");
    let embed = |name: &str, clip: &str, trim: bool| {
        let (rate, audio) = read(&root.join(name).join(format!("{clip}.wav")));
        let a = if trim {
            notdot_speakers::audio::voiced(rate, &audio).unwrap().0
        } else {
            audio
        };
        ex.embed(rate, &a).unwrap()
    };
    let cos = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
    for trim in [false, true] {
        for (a, b) in [
            ("daniel", "daniel"),
            ("karen", "karen"),
            ("daniel", "karen"),
            ("eddy", "flo"),
            ("daniel", "fred"),
        ] {
            let row: Vec<String> = (0..4)
                .map(|i| {
                    format!(
                        "{:.3}",
                        cos(
                            &embed(a, "enrol-0", trim),
                            &embed(b, &format!("enrol-{i}"), trim)
                        )
                    )
                })
                .collect();
            println!("trim={trim} {a} enrol-0 vs {b} enrol-0..3: {row:?}");
        }
    }
}
