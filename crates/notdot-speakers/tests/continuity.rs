//! Mixed-speaker evaluation of admission plus continuity on virtual audio.
//! Ignored by default; needs the bundled model and the synthetic fixtures:
//!
//! ```text
//! NOTDOT_SPEAKER_MODEL=src-tauri/resources/speaker/<MODEL_FILE> \
//! NOTDOT_SPEAKER_FIXTURES=<dir from scripts/render-synthetic-voices.sh> \
//! cargo test -p notdot-speakers --release --test continuity -- --ignored --nocapture
//! ```
//! Scenarios splice or mix synthetic macOS voices. They show how the logic
//! behaves; they are not evidence about real people, rooms or interruptions.
use notdot_speakers::{
    embedding::{CamPlusPlus, Extractor},
    engine::{Continuity, Engine, Identification},
    enrol::Enrolment,
    matcher::{Thresholds, Voiceprint},
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

const ENROLLED: [&str; 8] = [
    "daniel", "karen", "moira", "rishi", "tessa", "eddy", "samantha", "flo",
];
const GUESTS: [&str; 4] = ["fred", "albert", "kathy", "reed"];
pub const FLOOR: f32 = notdot_speakers::engine::CONTINUITY_FLOOR;

fn read(p: PathBuf) -> Vec<f32> {
    let mut r = hound::WavReader::open(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    r.samples::<i16>()
        .map(|s| f32::from(s.unwrap()) / 32768.0)
        .collect()
}
fn trim(v: Vec<f32>) -> Vec<f32> {
    let a = v.iter().position(|x| x.abs() > 0.01).unwrap_or(0);
    let b = v.iter().rposition(|x| x.abs() > 0.01).unwrap_or(v.len());
    v[a..b].to_vec()
}
fn mix(a: &[f32], b: &[f32], gain: f32) -> Vec<f32> {
    (0..a.len().max(b.len()))
        .map(|i| {
            (a.get(i).copied().unwrap_or(0.0) + gain * b.get(i).copied().unwrap_or(0.0))
                .clamp(-1.0, 1.0)
        })
        .collect()
}
fn noise(s: &[f32], snr_db: f32) -> Vec<f32> {
    let p = s.iter().map(|x| x * x).sum::<f32>() / s.len().max(1) as f32;
    let rms = (p / 10f32.powf(snr_db / 10.0)).sqrt() * 1.732;
    let mut x = 7u64;
    s.iter()
        .map(|v| {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (v + rms * (((x >> 33) as f32 / (1u64 << 31) as f32) * 2.0 - 1.0)).clamp(-1.0, 1.0)
        })
        .collect()
}

/// Root's admission policy: 2 s, then 5 s, then 6 s on refusals.
fn admit(e: &Engine, a: &[f32]) -> Option<String> {
    let go = AtomicBool::new(false);
    for secs in [2usize, 5, 6] {
        let n = (secs * 16_000).min(a.len());
        match e.identify(16_000, &a[..n], &go) {
            Identification::Recognised { profile, .. } => return Some(profile),
            _ if n == a.len() => return None,
            _ => {}
        }
    }
    None
}

#[derive(Default)]
struct Row {
    n: usize,
    refused_at_admission: usize,
    sent_as: BTreeMap<String, usize>,
    stopped_by_continuity: usize,
}

#[test]
#[ignore = "needs NOTDOT_SPEAKER_MODEL and NOTDOT_SPEAKER_FIXTURES"]
fn mixed_and_overlapping_speech() {
    let fx = PathBuf::from(std::env::var("NOTDOT_SPEAKER_FIXTURES").unwrap());
    let ex: Arc<dyn Extractor> = Arc::new(
        CamPlusPlus::load(&PathBuf::from(
            std::env::var("NOTDOT_SPEAKER_MODEL").unwrap(),
        ))
        .unwrap(),
    );
    let mut prints: Vec<Voiceprint> = vec![];
    for who in ENROLLED {
        let (mut en, _) = Enrolment::start(who);
        for i in 0..4 {
            en.submit(
                ex.as_ref(),
                16_000,
                &read(fx.join(who).join(format!("enrol-{i}.wav"))),
            );
        }
        prints.push(en.finish(&prints, &Thresholds::default()).unwrap());
    }
    let e = Engine::new(ex, prints, Thresholds::default());
    let clip = |who: &str, i: usize| trim(read(fx.join(who).join(format!("test-{}.wav", i % 8))));
    let two = |who: &str, i: usize| {
        let mut a = clip(who, i);
        a.extend(vec![0.0; 8000]);
        a.extend(clip(who, i + 1));
        a
    };
    let head = |v: Vec<f32>, s: f32| v[..((s * 16_000.0) as usize).min(v.len())].to_vec();
    let then = |a: Vec<f32>, b: Vec<f32>| {
        let mut a = a;
        a.extend(vec![0.0; 4000]);
        a.extend(b);
        a
    };
    type Gen<'a> = Box<dyn Fn(&str, &str, usize) -> (Vec<f32>, &'static str) + 'a>;
    let scenarios: Vec<(&str, Gen)> = vec![
        (
            "one enrolled person, two phrases",
            Box::new(|a, _, i| (two(a, i), "same")),
        ),
        (
            "one enrolled person, 20 dB noise",
            Box::new(|a, _, i| (noise(&two(a, i), 20.0), "same")),
        ),
        (
            "one enrolled person, 10 dB noise",
            Box::new(|a, _, i| (noise(&two(a, i), 10.0), "same")),
        ),
        (
            "A 2.5 s then enrolled B",
            Box::new(|a, b, i| (then(head(clip(a, i), 2.5), two(b, i + 3)), "switch")),
        ),
        (
            "A full phrase then enrolled B",
            Box::new(|a, b, i| (then(clip(a, i), clip(b, i + 3)), "switch")),
        ),
        (
            "A 2.5 s then unenrolled guest",
            Box::new(|a, _, i| {
                (
                    then(head(clip(a, i), 2.5), two(GUESTS[i % 4], i + 2)),
                    "switch",
                )
            }),
        ),
        (
            "A, B overlap at equal level",
            Box::new(|a, b, i| (mix(&two(a, i), &two(b, i + 3), 1.0), "switch")),
        ),
        (
            "A with B 6 dB quieter underneath",
            Box::new(|a, b, i| (mix(&two(a, i), &two(b, i + 3), 0.5), "switch")),
        ),
        (
            "A with guest 6 dB quieter underneath",
            Box::new(|a, _, i| (mix(&two(a, i), &two(GUESTS[i % 4], i + 2), 0.5), "switch")),
        ),
    ];
    let go = AtomicBool::new(false);
    let mut results = BTreeMap::new();
    println!("continuity floor {FLOOR:.2}; A and B rotate through the enrolled voices");
    for (label, make) in &scenarios {
        let mut row = Row::default();
        let mut wrong_person_sent = 0;
        for (k, a) in ENROLLED.iter().enumerate() {
            let b = ENROLLED[(k + 1) % ENROLLED.len()];
            for i in 0..4 {
                let (audio, kind) = make(a, b, i);
                row.n += 1;
                let Some(admitted) = admit(&e, &audio) else {
                    row.refused_at_admission += 1;
                    continue;
                };
                match e.continuity(16_000, &audio, &admitted, FLOOR, &go) {
                    Continuity::Consistent { .. } => {
                        *row.sent_as
                            .entry(if admitted == *a {
                                "A".into()
                            } else if admitted == b {
                                "B".into()
                            } else {
                                admitted.clone()
                            })
                            .or_default() += 1;
                        if kind == "switch" {
                            wrong_person_sent += 1;
                        }
                    }
                    Continuity::Switched { .. } => row.stopped_by_continuity += 1,
                    other => panic!("{other:?}"),
                }
            }
        }
        println!(
            "{label:<40} n={:<3} refused at admission {:<3} stopped by continuity {:<3} sent {:?}{}",
            row.n,
            row.refused_at_admission,
            row.stopped_by_continuity,
            row.sent_as,
            if wrong_person_sent > 0 {
                format!("  <- {wrong_person_sent} mixed captures sent")
            } else {
                String::new()
            }
        );
        results.insert(
            *label,
            (row.n, row.stopped_by_continuity, wrong_person_sent),
        );
    }
    // Regression bars on this synthetic set. Overlap rows are reported, not
    // asserted: simultaneous speech is a documented limit.
    for single in [
        "one enrolled person, two phrases",
        "one enrolled person, 20 dB noise",
    ] {
        assert_eq!(
            results[single].1, 0,
            "{single}: a single speaker was stopped"
        );
    }
    for handover in [
        "A 2.5 s then enrolled B",
        "A full phrase then enrolled B",
        "A 2.5 s then unenrolled guest",
    ] {
        let (n, _, sent) = results[handover];
        assert!(
            sent * 10 <= n,
            "{handover}: {sent}/{n} mixed captures were sent"
        );
    }
}
