//! Golden vectors for the orb contract. Desktop, Android and iOS adapter
//! tests all read `fixtures/orb-contract.json`, so the platforms cannot
//! drift apart without this file (and `ORB_CONTRACT_VERSION`) changing.
//!
//! Regenerate deliberately with:
//! `UPDATE_ORB_CONTRACT=1 cargo test -p notdot-orb --no-default-features --test contract`
use notdot_orb::{
    contract::{
        self, Microphone, ORB_CONTRACT_VERSION, OrbAnimator, OrbInput, Overlays, Paint, Quota,
    },
    visual_state::{Mode, TaskKind},
};
use serde_json::{Value, json};
use std::path::PathBuf;

const SIZE: f32 = 400.;
const TOLERANCE: f64 = 1e-6;

fn paint(paint: Paint) -> Value {
    json!({ "css": paint.css(), "rgba": paint.rgba() })
}

fn overlays(overlays: &Overlays) -> Value {
    json!({
        "glow": overlays.glow.map(|glow| json!({
            "radius": glow.radius,
            "stops": glow.stops.map(|(offset, colour)| json!({"offset": offset, "paint": paint(colour)})),
        })),
        "quota": overlays.quota.map(|ring| json!({
            "radius": ring.radius, "width_px": ring.width_px, "paint": paint(ring.paint),
        })),
        "task_available": overlays.task_available.map(|dot| json!({
            "dx": dot.dx, "dy": dot.dy, "radius_px": dot.radius_px, "paint": paint(dot.paint),
        })),
        "microphone": match overlays.microphone {
            Some(Microphone::Muted(bar)) => json!({
                "kind": "muted", "dy": bar.dy, "half_width_px": bar.half_width_px,
                "width_px": bar.width_px, "paint": paint(bar.paint),
            }),
            Some(Microphone::WakeReady(dot)) => json!({
                "kind": "wake_ready", "dx": dot.dx, "dy": dot.dy,
                "radius_px": dot.radius_px, "paint": paint(dot.paint),
            }),
            None => Value::Null,
        },
    })
}

fn quota_name(quota: Quota) -> &'static str {
    match quota {
        Quota::Normal => "normal",
        Quota::Warning => "warning",
        Quota::Exhausted => "exhausted",
    }
}

/// Named scripted sequences. Each step is (input, seconds).
fn sequences() -> Vec<(&'static str, Vec<(OrbInput, f64)>)> {
    let frame = 1. / 30.;
    let base = OrbInput::default();
    let hold = |input: OrbInput, frames: usize| vec![(input, frame); frames];
    let mut all = Vec::new();
    for mode in Mode::ALL {
        let mut steps = hold(OrbInput { mode, ..base }, 12);
        steps.extend(hold(
            OrbInput {
                mode,
                reduced: true,
                ..base
            },
            3,
        ));
        all.push((mode.name(), steps));
    }
    for task in &TaskKind::ALL[1..] {
        let mut steps = hold(
            OrbInput {
                task: *task,
                ..base
            },
            10,
        );
        // A foreground state hides the task light at once.
        steps.extend(hold(
            OrbInput {
                task: *task,
                mode: Mode::Speaking,
                ..base
            },
            2,
        ));
        all.push((task.name(), steps));
    }
    all.push((
        "energy_ramp",
        (0..=10)
            .map(|step| {
                (
                    OrbInput {
                        mode: Mode::Speaking,
                        level: Some(f64::from(step) / 40.),
                        ..base
                    },
                    frame,
                )
            })
            .chain([(
                OrbInput {
                    mode: Mode::Listening,
                    level: None,
                    ..base
                },
                frame,
            )])
            .collect(),
    ));
    all.push((
        "hue_sweep",
        [0., 42., 120., 192., 240., 280., 330., 360., -60.]
            .map(|hue| (OrbInput { hue, ..base }, frame))
            .to_vec(),
    ));
    all.push((
        "error_blend",
        hold(
            OrbInput {
                mode: Mode::Error,
                ..base
            },
            20,
        ),
    ));
    all.push((
        "bad_frame_times",
        [0., f64::NAN, f64::INFINITY, -1., 30., 1. / 30.]
            .map(|seconds| {
                (
                    OrbInput {
                        mode: Mode::Thinking,
                        ..base
                    },
                    seconds,
                )
            })
            .to_vec(),
    ));
    all.push((
        "overlays",
        vec![
            (
                OrbInput {
                    quota: Quota::Warning,
                    wake_ready: true,
                    ..base
                },
                frame,
            ),
            (
                OrbInput {
                    quota: Quota::Exhausted,
                    task_available: true,
                    microphone_muted: true,
                    wake_ready: true,
                    ..base
                },
                frame,
            ),
            (
                OrbInput {
                    mode: Mode::Listening,
                    hue: 280.,
                    ..base
                },
                frame,
            ),
        ],
    ));
    all.push((
        "offline",
        hold(
            OrbInput {
                offline: true,
                ..base
            },
            2,
        ),
    ));
    all
}

fn generate() -> Value {
    let sequences: Vec<Value> = sequences()
        .into_iter()
        .map(|(name, steps)| {
            let mut animator = OrbAnimator::default();
            let frames: Vec<Value> = steps
                .iter()
                .map(|(input, seconds)| {
                    let frame = animator.advance(input, *seconds);
                    json!({
                        "input": {
                            "mode": input.mode.name(),
                            "task": input.task.name(),
                            "hue": input.hue,
                            "level": input.level,
                            "reduced": input.reduced,
                            "quota": quota_name(input.quota),
                            "task_available": input.task_available,
                            "microphone_muted": input.microphone_muted,
                            "wake_ready": input.wake_ready,
                            "offline": input.offline,
                        },
                        // JSON has no NaN/Infinity; adapters map these names back.
                        "seconds": if seconds.is_finite() { json!(seconds) } else { json!(format!("{seconds}")) },
                        "packed": frame.uniforms.packed(SIZE, SIZE),
                        "visible_task": frame.task.name(),
                        "overlays": overlays(&frame.overlays),
                    })
                })
                .collect();
            json!({ "name": name, "frames": frames })
        })
        .collect();
    json!({
        "contract_version": ORB_CONTRACT_VERSION,
        "shader_sha256": contract::shader_sha256(),
        "uniforms": contract::UNIFORMS,
        "radius_fraction": contract::RADIUS_FRACTION,
        "size": SIZE,
        "sequences": sequences,
    })
}

fn close(expected: &Value, actual: &Value, path: &str) {
    match (expected, actual) {
        (Value::Number(e), Value::Number(a)) => {
            let (e, a) = (e.as_f64().unwrap(), a.as_f64().unwrap());
            assert!((e - a).abs() <= TOLERANCE, "{path}: expected {e}, got {a}");
        }
        (Value::Array(e), Value::Array(a)) => {
            assert_eq!(e.len(), a.len(), "{path}: length");
            for (index, (e, a)) in e.iter().zip(a).enumerate() {
                close(e, a, &format!("{path}[{index}]"));
            }
        }
        (Value::Object(e), Value::Object(a)) => {
            assert_eq!(
                e.keys().collect::<Vec<_>>(),
                a.keys().collect::<Vec<_>>(),
                "{path}: keys"
            );
            for (key, e) in e {
                close(e, &a[key], &format!("{path}.{key}"));
            }
        }
        _ => assert_eq!(expected, actual, "{path}"),
    }
}

#[test]
fn contract_matches_golden_vectors() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/orb-contract.json");
    let actual = generate();
    if std::env::var_os("UPDATE_ORB_CONTRACT").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(&actual).unwrap() + "\n").unwrap();
    }
    let expected: Value = serde_json::from_str(
        &std::fs::read_to_string(&path).expect("fixtures/orb-contract.json is checked in"),
    )
    .unwrap();
    assert_eq!(
        expected["contract_version"], ORB_CONTRACT_VERSION,
        "golden vectors changed: bump ORB_CONTRACT_VERSION and regenerate deliberately"
    );
    close(&expected, &actual, "$");
}

#[test]
fn golden_vectors_cover_every_mode_task_and_overlay() {
    let fixture = generate();
    let frames: Vec<&Value> = fixture["sequences"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|sequence| sequence["frames"].as_array().unwrap())
        .collect();
    for mode in Mode::ALL {
        assert!(frames.iter().any(|f| f["input"]["mode"] == mode.name()));
    }
    for task in &TaskKind::ALL[1..] {
        assert!(frames.iter().any(|f| f["visible_task"] == task.name()));
    }
    for key in ["glow", "quota", "task_available"] {
        assert!(
            frames.iter().any(|f| !f["overlays"][key].is_null()),
            "{key}"
        );
    }
    for kind in ["muted", "wake_ready"] {
        assert!(
            frames
                .iter()
                .any(|f| f["overlays"]["microphone"]["kind"] == kind)
        );
    }
    assert!(frames.iter().all(|f| {
        f["packed"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v.as_f64().is_some_and(f64::is_finite))
    }));
}
