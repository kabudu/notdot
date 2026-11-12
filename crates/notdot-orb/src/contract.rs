//! The orb's platform contract: state in, shader uniforms and overlay
//! primitives out. Desktop WebGL, Android and iOS all draw from this.
//!
//! Adapters only bind [`Uniforms`] to the canonical shader and draw the
//! [`Overlays`] it returns. Easing, timing, colour maths and state rules
//! live here and in `visual_state`/`theme`, never in native code.
use crate::{
    theme,
    visual_state::{ListeningCue, Mode, OrbMotion, TaskKind, TaskMotion, smooth_speech},
};

/// Bump on any change to uniform meaning, overlay geometry or golden output.
pub const ORB_CONTRACT_VERSION: u32 = 1;
/// The canonical fragment shader (GLSL ES 1.00). Generated adapters embed it
/// or a translation of it, stamped with [`shader_sha256`].
pub const FRAGMENT_SHADER: &str = include_str!("orb.frag");
/// Full-screen triangle pair; `position` is the only attribute.
pub const VERTEX_SHADER: &str =
    "attribute vec2 position;void main(){gl_Position=vec4(position,0.,1.);}";
/// Two triangles covering clip space, as `position` pairs.
pub const QUAD: [f32; 12] = [-1., -1., 1., -1., -1., 1., -1., 1., 1., -1., 1., 1.];
/// Uniform names in [`Uniforms::packed`] order. `resolution` is a vec2,
/// `tint` a vec3, the rest are floats.
pub const UNIFORMS: [&str; 11] = [
    "resolution",
    "tint",
    "phase",
    "energy",
    "listening",
    "thinking",
    "speaking",
    "still",
    "task_kind",
    "task_time",
    "task_strength",
];
/// Packed float count: vec2 + vec3 + 9 floats.
pub const PACKED_LEN: usize = 14;
/// Uniform-buffer layout for APIs without loose uniforms (Metal, Vulkan):
/// four `vec4`s, `u0`..`u3`. Each canonical uniform maps to a swizzle.
/// [`Uniforms::block`] fills it; `notdot-orb-gen` emits the same mapping.
pub const BLOCK_VEC4S: usize = 4;
pub const BLOCK_MAPPING: [(&str, &str); 11] = [
    ("resolution", "u0.xy"),
    ("phase", "u0.z"),
    ("energy", "u0.w"),
    ("tint", "u1.xyz"),
    ("listening", "u1.w"),
    ("thinking", "u2.x"),
    ("speaking", "u2.y"),
    ("still", "u2.z"),
    ("task_kind", "u2.w"),
    ("task_time", "u3.x"),
    ("task_strength", "u3.y"),
];
/// Orb radius as a fraction of the drawing area's shorter side. Overlay
/// geometry below is in multiples of this radius unless marked as pixels.
pub const RADIUS_FRACTION: f64 = 0.17;

/// Voice-allowance warning, as reported by the linked computer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Quota {
    #[default]
    Normal,
    Warning,
    Exhausted,
}

/// Everything a platform knows about the orb for one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrbInput {
    pub mode: Mode,
    /// Raw task cue. Shown only while idle; see `visual_state::visible_task`.
    pub task: TaskKind,
    /// Theme hue in degrees.
    pub hue: f64,
    /// Metered audio RMS (0-1), or `None` when the audio cannot be metered.
    pub level: Option<f64>,
    /// Reduce Motion: freeze phase and task light, steady acknowledgement.
    pub reduced: bool,
    pub quota: Quota,
    /// A task or follow-up is waiting to be heard.
    pub task_available: bool,
    pub microphone_muted: bool,
    /// The wake word is armed (shown only when not muted).
    pub wake_ready: bool,
    /// Mobile only: the linked computer is unreachable.
    pub offline: bool,
}
impl Default for OrbInput {
    fn default() -> Self {
        Self {
            mode: Mode::Idle,
            task: TaskKind::None,
            hue: f64::from(theme::DEFAULT_HUE),
            level: None,
            reduced: false,
            quota: Quota::Normal,
            task_available: false,
            microphone_muted: false,
            wake_ready: false,
            offline: false,
        }
    }
}

/// Values for the canonical shader's uniforms, excluding `resolution`,
/// which is the drawable size in pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Uniforms {
    pub tint: [f32; 3],
    pub phase: f32,
    pub energy: f32,
    pub listening: f32,
    pub thinking: f32,
    pub speaking: f32,
    pub still: f32,
    pub task_kind: f32,
    pub task_time: f32,
    pub task_strength: f32,
}
impl Uniforms {
    /// The single mapping from animation state to shader inputs.
    pub fn compute(
        hue: f64,
        energy: f64,
        reduced: bool,
        motion: &OrbMotion,
        task: &TaskMotion,
        offline: bool,
    ) -> Self {
        let tint = theme::error_blend(theme::tint(hue), motion.error);
        Self {
            tint: if offline {
                theme::offline_blend(tint)
            } else {
                tint
            },
            phase: motion.phase as f32,
            energy: energy as f32,
            listening: (motion.listening + motion.followup) as f32,
            thinking: motion.thinking as f32,
            speaking: motion.speaking as f32,
            still: f32::from(u8::from(reduced)),
            task_kind: f32::from(task.kind as u8),
            task_time: task.elapsed as f32,
            task_strength: task.strength as f32,
        }
    }
    /// Values in [`BLOCK_MAPPING`] layout: 16 floats, `u0`..`u3`.
    pub fn block(&self, width: f32, height: f32) -> [f32; BLOCK_VEC4S * 4] {
        let [r, g, b] = self.tint;
        [
            width,
            height,
            self.phase,
            self.energy,
            r,
            g,
            b,
            self.listening,
            self.thinking,
            self.speaking,
            self.still,
            self.task_kind,
            self.task_time,
            self.task_strength,
            0.,
            0.,
        ]
    }
    /// Values in [`UNIFORMS`] order for a drawable of `width` x `height` px.
    pub fn packed(&self, width: f32, height: f32) -> [f32; PACKED_LEN] {
        let [r, g, b] = self.tint;
        [
            width,
            height,
            r,
            g,
            b,
            self.phase,
            self.energy,
            self.listening,
            self.thinking,
            self.speaking,
            self.still,
            self.task_kind,
            self.task_time,
            self.task_strength,
        ]
    }
}

/// A colour exactly as the desktop canvas styles it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Paint {
    /// `hsla(h, s%, l%, a)` with saturation and lightness in percent.
    Hsla { h: f64, s: u8, l: u8, a: f64 },
    /// Opaque `#rrggbb`.
    Hex(u32),
    /// Fully transparent black.
    Clear,
}
impl Paint {
    /// CSS string, byte-for-byte what the desktop drew before the contract.
    pub fn css(self) -> String {
        match self {
            Self::Hsla { h, s, l, a } if a >= 1. => format!("hsl({h},{s}%,{l}%)"),
            Self::Hsla { h, s, l, a } => format!("hsla({h},{s}%,{l}%,{a})"),
            Self::Hex(rgb) => format!("#{rgb:06x}"),
            Self::Clear => "rgba(0,0,0,0)".into(),
        }
    }
    /// Straight (not premultiplied) sRGB components, 0-1, for native canvases.
    pub fn rgba(self) -> [f32; 4] {
        match self {
            Self::Hsla { h, s, l, a } => {
                let (s, l) = (f64::from(s) / 100., f64::from(l) / 100.);
                let chroma = (1. - (2. * l - 1.).abs()) * s;
                let [r, g, b] = theme_rgb(h, chroma, l);
                [r, g, b, a.clamp(0., 1.) as f32]
            }
            Self::Hex(rgb) => [
                ((rgb >> 16) & 0xff) as f32 / 255.,
                ((rgb >> 8) & 0xff) as f32 / 255.,
                (rgb & 0xff) as f32 / 255.,
                1.,
            ],
            Self::Clear => [0.; 4],
        }
    }
}
fn theme_rgb(hue: f64, chroma: f64, lightness: f64) -> [f32; 3] {
    let h = hue.rem_euclid(360.) / 60.;
    let x = chroma * (1. - (h.rem_euclid(2.) - 1.).abs());
    let (r, g, b) = match h as u32 {
        0 => (chroma, x, 0.),
        1 => (x, chroma, 0.),
        2 => (0., chroma, x),
        3 => (0., x, chroma),
        4 => (x, 0., chroma),
        _ => (chroma, 0., x),
    };
    let m = lightness - chroma / 2.;
    [(r + m) as f32, (g + m) as f32, (b + m) as f32]
}

/// Radial glow inside the opaque core acknowledging capture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glow {
    /// Outer radius in orb radii.
    pub radius: f64,
    /// (offset 0-1, paint) gradient stops from the centre outwards.
    pub stops: [(f64, Paint); 3],
}
/// Stroked circle centred on the orb.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ring {
    pub radius: f64,
    pub width_px: f64,
    pub paint: Paint,
}
/// Filled dot, offset from the orb centre in orb radii (y grows downwards).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dot {
    pub dx: f64,
    pub dy: f64,
    pub radius_px: f64,
    pub paint: Paint,
}
/// Short horizontal stroke below the orb.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bar {
    pub dy: f64,
    pub half_width_px: f64,
    pub width_px: f64,
    pub paint: Paint,
}
/// Microphone indicator below the orb; muted takes precedence over armed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Microphone {
    Muted(Bar),
    WakeReady(Dot),
}
/// Overlays in draw order. Platforms may add focus rings between `glow` and
/// `quota`, but must not restyle these.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Overlays {
    pub glow: Option<Glow>,
    pub quota: Option<Ring>,
    pub task_available: Option<Dot>,
    pub microphone: Option<Microphone>,
}
impl Overlays {
    pub fn compute(input: &OrbInput, acknowledgement: f64) -> Self {
        let hue = input.hue;
        Self {
            glow: (acknowledgement > 0.).then_some(Glow {
                radius: 0.94,
                stops: [
                    (
                        0.,
                        Paint::Hsla {
                            h: hue,
                            s: 95,
                            l: 86,
                            a: acknowledgement,
                        },
                    ),
                    (
                        0.55,
                        Paint::Hsla {
                            h: hue,
                            s: 90,
                            l: 74,
                            a: acknowledgement * 0.65,
                        },
                    ),
                    (1., Paint::Clear),
                ],
            }),
            quota: match input.quota {
                Quota::Normal => None,
                Quota::Warning => Some(0xedb347),
                Quota::Exhausted => Some(0xf08c7a),
            }
            .map(|rgb| Ring {
                radius: 1.8,
                width_px: 2.5,
                paint: Paint::Hex(rgb),
            }),
            task_available: input.task_available.then_some(Dot {
                dx: 1.55,
                dy: -1.55,
                radius_px: 5.,
                paint: Paint::Hsla {
                    h: hue,
                    s: 90,
                    l: 75,
                    a: 1.,
                },
            }),
            microphone: if input.microphone_muted {
                Some(Microphone::Muted(Bar {
                    dy: 1.8,
                    half_width_px: 7.,
                    width_px: 2.5,
                    paint: Paint::Hex(0xa7aeb8),
                }))
            } else if input.wake_ready {
                Some(Microphone::WakeReady(Dot {
                    dx: 0.,
                    dy: 1.8,
                    radius_px: 3.,
                    paint: Paint::Hex(0xc7f9e1),
                }))
            } else {
                None
            },
        }
    }
}

/// One frame for a platform to draw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrbFrame {
    pub uniforms: Uniforms,
    pub overlays: Overlays,
    /// The task cue actually shown, after the idle-only rule.
    pub task: TaskKind,
}

/// All animation state for one orb. Feed it [`OrbInput`] each frame.
#[derive(Default)]
pub struct OrbAnimator {
    motion: OrbMotion,
    task: TaskMotion,
    cue: ListeningCue,
    speech: f64,
}
impl OrbAnimator {
    /// Advance by `seconds` (clamped like the desktop clock) and describe the
    /// frame. Pass seconds from `visual_state::FrameClock`.
    pub fn advance(&mut self, input: &OrbInput, seconds: f64) -> OrbFrame {
        let mode = input.mode;
        let task = crate::visual_state::visible_task(mode, input.task);
        self.speech = smooth_speech(
            self.speech,
            crate::visual_state::speech_target(mode, input.level),
            seconds,
        );
        self.task.advance_kind(task, seconds, input.reduced);
        self.motion.advance(mode, seconds, input.reduced);
        let acknowledgement = self.cue.advance(mode, seconds, input.reduced);
        OrbFrame {
            uniforms: Uniforms::compute(
                input.hue,
                self.speech,
                input.reduced,
                &self.motion,
                &self.task,
                input.offline,
            ),
            overlays: Overlays::compute(input, acknowledgement),
            task,
        }
    }
    /// Replay the task light's entry, e.g. when a different task finishes.
    pub fn restart_task(&mut self) {
        self.task = TaskMotion::default();
    }
    pub fn motion(&self) -> &OrbMotion {
        &self.motion
    }
    pub fn task(&self) -> &TaskMotion {
        &self.task
    }
    pub fn speech_energy(&self) -> f64 {
        self.speech
    }
}

/// SHA-256 of [`FRAGMENT_SHADER`], lower-case hex. Generated shader copies
/// carry this so a build can reject a stale or hand-edited copy.
pub fn shader_sha256() -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(FRAGMENT_SHADER.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shader_fingerprint_is_the_canonical_file_hash() {
        let hash = shader_sha256();
        assert_eq!(hash.len(), 64);
        assert!(
            hash.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
        // Recorded for v0.8.0 in the private design notes; a shader change
        // must update this value together with ORB_CONTRACT_VERSION.
        assert_eq!(
            hash,
            "1dd9c26cac3a2523214c4f707fd40f760635e55fa2248b8a8e07ace3cc9a48b0"
        );
    }

    #[test]
    fn shader_declares_exactly_the_contract_uniforms() {
        let declared: Vec<&str> = FRAGMENT_SHADER
            .lines()
            .filter_map(|line| line.trim().strip_prefix("uniform "))
            .filter_map(|rest| rest.trim_end_matches(';').split_whitespace().nth(1))
            .collect();
        assert_eq!(declared, UNIFORMS);
    }

    #[test]
    fn overlay_paints_render_the_desktop_css_strings() {
        let input = OrbInput {
            hue: 192.,
            quota: Quota::Exhausted,
            task_available: true,
            wake_ready: true,
            ..OrbInput::default()
        };
        let overlays = Overlays::compute(&input, 0.5);
        let glow = overlays.glow.unwrap();
        assert_eq!(glow.stops[0].1.css(), "hsla(192,95%,86%,0.5)");
        assert_eq!(glow.stops[1].1.css(), "hsla(192,90%,74%,0.325)");
        assert_eq!(glow.stops[2].1.css(), "rgba(0,0,0,0)");
        assert_eq!(overlays.quota.unwrap().paint.css(), "#f08c7a");
        assert_eq!(
            overlays.task_available.unwrap().paint.css(),
            "hsl(192,90%,75%)"
        );
        assert!(matches!(
            overlays.microphone,
            Some(Microphone::WakeReady(Dot { paint, .. })) if paint.css() == "#c7f9e1"
        ));
        let muted = Overlays::compute(
            &OrbInput {
                microphone_muted: true,
                ..input
            },
            0.,
        );
        assert!(muted.glow.is_none());
        assert!(
            matches!(muted.microphone, Some(Microphone::Muted(Bar { paint, .. })) if paint.css() == "#a7aeb8")
        );
        assert_eq!(
            Paint::Hex(0xedb347).rgba(),
            [237. / 255., 179. / 255., 71. / 255., 1.]
        );
        let [r, g, b, a] = Paint::Hsla {
            h: 0.,
            s: 100,
            l: 50,
            a: 0.5,
        }
        .rgba();
        assert_eq!([r, g, b, a], [1., 0., 0., 0.5]);
    }

    #[test]
    fn animator_matches_desktop_composition_step_by_step() {
        // The desktop loop composed these primitives directly before the
        // contract existed; the animator must produce identical values.
        let mut animator = OrbAnimator::default();
        let mut motion = OrbMotion::default();
        let mut task = TaskMotion::default();
        let mut cue = ListeningCue::default();
        let mut speech = 0.;
        let script = [
            (Mode::Idle, TaskKind::Working, Some(0.)),
            (Mode::Listening, TaskKind::Working, Some(0.12)),
            (Mode::Followup, TaskKind::None, None),
            (Mode::Thinking, TaskKind::Attention, None),
            (Mode::Speaking, TaskKind::Finished, Some(0.3)),
            (Mode::Error, TaskKind::None, None),
            (Mode::Idle, TaskKind::Review, None),
        ];
        for (step, (mode, raw, level)) in script.iter().cycle().take(140).enumerate() {
            let reduced = step % 37 > 30;
            let seconds = [1. / 30., 0.1, 1. / 60., 3., f64::NAN][step % 5];
            let input = OrbInput {
                mode: *mode,
                task: *raw,
                level: *level,
                reduced,
                hue: 40. + step as f64,
                ..OrbInput::default()
            };
            let frame = animator.advance(&input, seconds);
            let visible = crate::visual_state::task_cue(*mode, raw.name());
            let energy = level.map_or(0.25, |rms| (rms * 4.5).min(1.));
            let target = if matches!(mode, Mode::Speaking | Mode::Listening | Mode::Followup) {
                energy
            } else {
                0.
            };
            speech = smooth_speech(speech, target, seconds);
            task.advance(visible, seconds, reduced);
            motion.advance(*mode, seconds, reduced);
            let ack = cue.advance(*mode, seconds, reduced);
            let expected = Uniforms::compute(input.hue, speech, reduced, &motion, &task, false);
            assert_eq!(frame.uniforms, expected, "step {step}");
            assert_eq!(
                frame.overlays,
                Overlays::compute(&input, ack),
                "step {step}"
            );
            assert_eq!(frame.task.name(), visible);
        }
    }

    #[test]
    fn block_layout_agrees_with_its_mapping_and_the_packed_order() {
        let uniforms = Uniforms {
            tint: [0.5, 0.6, 0.7],
            phase: 1.,
            energy: 2.,
            listening: 3.,
            thinking: 4.,
            speaking: 5.,
            still: 6.,
            task_kind: 7.,
            task_time: 8.,
            task_strength: 9.,
        };
        let packed = uniforms.packed(320., 240.);
        let block = uniforms.block(320., 240.);
        let mut offset = 0;
        for (name, width) in UNIFORMS.iter().zip([2, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1]) {
            let (_, swizzle) = BLOCK_MAPPING.iter().find(|(n, _)| n == name).unwrap();
            let (vector, lanes) = swizzle.split_once('.').unwrap();
            let base = 4 * vector[1..].parse::<usize>().unwrap();
            assert_eq!(lanes.len(), width, "{name}");
            for (lane, component) in lanes.chars().enumerate() {
                let index = base + "xyzw".find(component).unwrap();
                assert_eq!(block[index], packed[offset + lane], "{name}.{component}");
            }
            offset += width;
        }
        assert_eq!(offset, PACKED_LEN);
    }

    #[test]
    fn uniforms_pack_in_declared_order_and_offline_only_changes_tint() {
        let motion = OrbMotion {
            phase: 1.5,
            listening: 0.25,
            followup: 0.5,
            thinking: 0.125,
            speaking: 0.0625,
            error: 0.,
        };
        let task = TaskMotion {
            kind: TaskKind::Review,
            elapsed: 2.,
            strength: 0.75,
        };
        let online = Uniforms::compute(192., 0.5, true, &motion, &task, false);
        let packed = online.packed(320., 240.);
        assert_eq!(&packed[..2], &[320., 240.]);
        assert_eq!(&packed[2..5], &online.tint);
        assert_eq!(
            &packed[5..],
            &[1.5, 0.5, 0.75, 0.125, 0.0625, 1., 4., 2., 0.75]
        );
        let offline = Uniforms::compute(192., 0.5, true, &motion, &task, true);
        assert_ne!(offline.tint, online.tint);
        assert_eq!(
            Uniforms {
                tint: online.tint,
                ..offline
            },
            online
        );
    }
}
