//! Matching decisions for short spoken requests at a quiet desk-microphone
//! level. Ignored by default (needs the model and fixtures, as for
//! `acoustic.rs`); run with the same environment variables:
//!
//! ```text
//! cargo test -p notdot-speakers --release --test short_requests -- --ignored --nocapture
//! ```
//!
//! Each test clip is cut to N seconds from speech onset and scaled to a
//! voiced RMS of 0.019, as measured from a real request at about 50 cm.
use notdot_speakers::{
    embedding::{CamPlusPlus, Extractor},
    enrol::{Enrolment, Progress},
    matcher::{self, Match, Thresholds, Voiceprint},
};
use std::path::{Path, PathBuf};

const ENROLLED: [&str; 8] = [
    "daniel", "karen", "moira", "rishi", "samantha", "tessa", "eddy", "flo",
];

fn read(path: &Path) -> (u32, Vec<f32>) {
    let mut r = hound::WavReader::open(path).unwrap();
    let rate = r.spec().sample_rate;
    (
        rate,
        r.samples::<i16>()
            .map(|s| f32::from(s.unwrap()) / 32768.0)
            .collect(),
    )
}
fn frame_rms(f: &[f32]) -> f32 {
    (f.iter().map(|s| s * s).sum::<f32>() / f.len().max(1) as f32).sqrt()
}
/// Voiced seconds at a frame threshold (20 ms frames).
fn voiced_s(rate: u32, s: &[f32], thr: f32) -> f32 {
    let n = (rate / 50) as usize;
    s.chunks(n).filter(|f| frame_rms(f) >= thr).count() as f32 * 0.02
}

#[test]
#[ignore = "needs NOTDOT_SPEAKER_MODEL and NOTDOT_SPEAKER_FIXTURES"]
fn short_quiet_requests() {
    let model = PathBuf::from(std::env::var_os("NOTDOT_SPEAKER_MODEL").unwrap());
    let root = PathBuf::from(std::env::var_os("NOTDOT_SPEAKER_FIXTURES").unwrap());
    let ex = CamPlusPlus::load(&model).unwrap();
    let t = Thresholds::default();
    let mut prints: Vec<Voiceprint> = Vec::new();
    for name in ENROLLED {
        let (mut e, _) = Enrolment::start(name);
        let mut last = None;
        for i in 0..4 {
            let (rate, a) = read(&root.join(name).join(format!("enrol-{i}.wav")));
            last = Some(e.submit(&ex, rate, &a));
        }
        assert_eq!(last, Some(Progress::Ready { of: 4 }));
        if let Ok(p) = e.finish(&prints, &t) {
            prints.push(p);
        }
    }
    let mut speakers: Vec<String> = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().into_string().unwrap())
        .collect();
    speakers.sort();
    // Request audio: speech from onset, cut to `secs`, scaled so the voiced
    // RMS (at 0.008) matches the user's measured level (~0.019).
    for secs in [0.6f32, 0.8, 1.0, 1.3, 1.6, 2.5] {
        let (
            mut ok,
            mut wrong,
            mut amb,
            mut unk,
            mut fa,
            mut imp_unk,
            mut imp_amb,
            mut n_k,
            mut n_i,
        ) = (0, 0, 0, 0, 0, 0, 0, 0, 0);
        let mut v8 = Vec::new();
        let mut v4 = Vec::new();
        for name in &speakers {
            let known = prints.iter().any(|p| &p.profile == name);
            for i in 0..8 {
                let (rate, a) = read(&root.join(name).join(format!("test-{i}.wav")));
                let n = (rate / 50) as usize;
                let onset = a.chunks(n).position(|f| frame_rms(f) >= 0.008).unwrap_or(0) * n;
                let end = (onset + (secs * rate as f32) as usize).min(a.len());
                let clip = &a[onset..end];
                let loud: Vec<&[f32]> = clip.chunks(n).filter(|f| frame_rms(f) >= 0.008).collect();
                let lr = (loud
                    .iter()
                    .map(|f| f.iter().map(|s| s * s).sum::<f32>())
                    .sum::<f32>()
                    / (loud.len().max(1) * n) as f32)
                    .sqrt();
                let g = 0.019 / lr.max(1e-6);
                let clip: Vec<f32> = clip.iter().map(|s| s * g).collect();
                v8.push(voiced_s(rate, &clip, 0.008));
                v4.push(voiced_s(rate, &clip, 0.004));
                let e = ex.embed(rate, &clip).unwrap();
                let m = matcher::identify(&prints, &e, &t);
                if known {
                    n_k += 1;
                    match m {
                        Match::Recognised { ref profile, .. } if profile == name => ok += 1,
                        Match::Recognised { .. } => wrong += 1,
                        Match::Ambiguous { .. } => amb += 1,
                        _ => unk += 1,
                    }
                } else {
                    n_i += 1;
                    match m {
                        Match::Recognised { ref profile, .. } => {
                            fa += 1;
                            println!("  {secs:.1}s stranger {name} accepted as {profile}");
                        }
                        Match::Ambiguous { .. } => imp_amb += 1,
                        _ => imp_unk += 1,
                    }
                }
            }
        }
        v8.sort_by(f32::total_cmp);
        v4.sort_by(f32::total_cmp);
        println!(
            "{secs:.1}s: enrolled n={n_k} correct={ok} WRONG={wrong} ambiguous={amb} unknown={unk} | strangers n={n_i} ACCEPTED={fa} ambiguous={imp_amb} unknown={imp_unk} | voiced@0.008 median {:.2}s, @0.004 median {:.2}s",
            v8[v8.len() / 2],
            v4[v4.len() / 2]
        );
    }
}
