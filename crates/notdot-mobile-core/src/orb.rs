//! The canonical orb for native shells: state in, uniforms and overlay
//! primitives out, plus the generated platform shaders. Native code binds
//! and draws only; all motion, timing and colour come from `notdot-orb`.
use notdot_orb::{
    contract::{self, Microphone, OrbAnimator, OrbInput, Paint, Quota},
    visual_state::{FrameClock, Mode, TaskKind},
};
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum OrbMode {
    Idle,
    Listening,
    Followup,
    Thinking,
    Speaking,
    Error,
}
impl From<OrbMode> for Mode {
    fn from(mode: OrbMode) -> Self {
        match mode {
            OrbMode::Idle => Mode::Idle,
            OrbMode::Listening => Mode::Listening,
            OrbMode::Followup => Mode::Followup,
            OrbMode::Thinking => Mode::Thinking,
            OrbMode::Speaking => Mode::Speaking,
            OrbMode::Error => Mode::Error,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum OrbTask {
    None,
    Working,
    Attention,
    Finished,
    Review,
}
impl From<OrbTask> for TaskKind {
    fn from(task: OrbTask) -> Self {
        match task {
            OrbTask::None => TaskKind::None,
            OrbTask::Working => TaskKind::Working,
            OrbTask::Attention => TaskKind::Attention,
            OrbTask::Finished => TaskKind::Finished,
            OrbTask::Review => TaskKind::Review,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum OrbQuota {
    Normal,
    Warning,
    Exhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct OrbState {
    pub mode: OrbMode,
    pub task: OrbTask,
    pub hue: f64,
    /// Metered audio RMS 0-1, or none when the audio is not metered.
    pub level: Option<f64>,
    pub reduced_motion: bool,
    pub quota: OrbQuota,
    pub task_available: bool,
    pub microphone_muted: bool,
    pub wake_ready: bool,
    pub offline: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum OverlayKind {
    /// Radial glow: `radius` in orb radii, colours in `stops`.
    Glow,
    /// Stroked circle of `radius` orb radii, `width_px` points/dp wide.
    Ring,
    /// Filled dot at (`dx`, `dy`) orb radii, `size_px` radius.
    Dot,
    /// Horizontal stroke at `dy` orb radii, half-width `size_px`.
    Bar,
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct GlowStop {
    pub offset: f64,
    /// Straight sRGB RGBA, 0-1.
    pub rgba: Vec<f32>,
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct OrbOverlay {
    pub kind: OverlayKind,
    pub radius: f64,
    pub dx: f64,
    pub dy: f64,
    pub size_px: f64,
    pub width_px: f64,
    pub rgba: Vec<f32>,
    pub stops: Vec<GlowStop>,
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct OrbFrame {
    /// Values for `orb_uniform_names()` (GLSL): resolution(2), tint(3), 9 floats.
    pub packed: Vec<f32>,
    /// The same values as four vec4s for the Metal uniform buffer.
    pub block: Vec<f32>,
    /// Overlays in draw order, after the shader.
    pub overlays: Vec<OrbOverlay>,
    /// Plain-language state for VoiceOver/TalkBack.
    pub accessibility_label: String,
}

fn overlay(kind: OverlayKind, paint: Paint) -> OrbOverlay {
    OrbOverlay {
        kind,
        radius: 0.,
        dx: 0.,
        dy: 0.,
        size_px: 0.,
        width_px: 0.,
        rgba: paint.rgba().to_vec(),
        stops: Vec::new(),
    }
}

fn label(state: &OrbState, task: TaskKind) -> String {
    let mode = match state.mode {
        OrbMode::Idle => "Ready",
        OrbMode::Listening => "Listening",
        OrbMode::Followup => "Listening for a follow-up",
        OrbMode::Thinking => "Thinking",
        OrbMode::Speaking => "Speaking",
        OrbMode::Error => "Something went wrong",
    };
    let mut parts = vec![mode];
    if state.offline {
        parts.insert(0, "Computer offline");
    }
    parts.push(match task {
        TaskKind::None => "",
        TaskKind::Working => "A task is running",
        TaskKind::Attention => "A task needs your attention",
        TaskKind::Finished => "A task finished",
        TaskKind::Review => "A task result is ready to review",
    });
    if state.microphone_muted {
        parts.push("Microphone muted");
    } else if state.wake_ready {
        parts.push("Wake word on");
    }
    match state.quota {
        OrbQuota::Normal => {}
        OrbQuota::Warning => parts.push("Voice allowance running low"),
        OrbQuota::Exhausted => parts.push("Voice allowance used up"),
    }
    parts.retain(|p| !p.is_empty());
    parts.join(". ") + "."
}

/// One animated orb. Call `frame` from the display link / frame callback.
#[derive(uniffi::Object)]
pub struct OrbEngine {
    inner: Mutex<(OrbAnimator, FrameClock)>,
}

#[uniffi::export]
impl OrbEngine {
    #[uniffi::constructor]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new((OrbAnimator::default(), FrameClock::default())),
        }
    }

    /// The frame for `now_ms` (monotonic), or none when no frame is due:
    /// 30 fps normally, 10 fps with reduced motion, as on desktop.
    pub fn frame(&self, state: OrbState, now_ms: f64, width: f32, height: f32) -> Option<OrbFrame> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let (animator, clock) = &mut *inner;
        let seconds = clock.tick(now_ms, state.reduced_motion)?;
        let input = OrbInput {
            mode: state.mode.into(),
            task: state.task.into(),
            hue: state.hue,
            level: state.level,
            reduced: state.reduced_motion,
            quota: match state.quota {
                OrbQuota::Normal => Quota::Normal,
                OrbQuota::Warning => Quota::Warning,
                OrbQuota::Exhausted => Quota::Exhausted,
            },
            task_available: state.task_available,
            microphone_muted: state.microphone_muted,
            wake_ready: state.wake_ready,
            offline: state.offline,
        };
        let frame = animator.advance(&input, seconds);
        let mut overlays = Vec::new();
        let o = frame.overlays;
        if let Some(glow) = o.glow {
            overlays.push(OrbOverlay {
                radius: glow.radius,
                stops: glow
                    .stops
                    .iter()
                    .map(|(offset, paint)| GlowStop {
                        offset: *offset,
                        rgba: paint.rgba().to_vec(),
                    })
                    .collect(),
                ..overlay(OverlayKind::Glow, contract::Paint::Clear)
            });
        }
        if let Some(ring) = o.quota {
            overlays.push(OrbOverlay {
                radius: ring.radius,
                width_px: ring.width_px,
                ..overlay(OverlayKind::Ring, ring.paint)
            });
        }
        if let Some(dot) = o.task_available {
            overlays.push(OrbOverlay {
                dx: dot.dx,
                dy: dot.dy,
                size_px: dot.radius_px,
                ..overlay(OverlayKind::Dot, dot.paint)
            });
        }
        match o.microphone {
            Some(Microphone::Muted(bar)) => overlays.push(OrbOverlay {
                dy: bar.dy,
                size_px: bar.half_width_px,
                width_px: bar.width_px,
                ..overlay(OverlayKind::Bar, bar.paint)
            }),
            Some(Microphone::WakeReady(dot)) => overlays.push(OrbOverlay {
                dx: dot.dx,
                dy: dot.dy,
                size_px: dot.radius_px,
                ..overlay(OverlayKind::Dot, dot.paint)
            }),
            None => {}
        }
        Some(OrbFrame {
            packed: frame.uniforms.packed(width, height).to_vec(),
            block: frame.uniforms.block(width, height).to_vec(),
            overlays,
            accessibility_label: label(&state, frame.task),
        })
    }

    /// Replay the task light's entry (a different task finished).
    pub fn restart_task(&self) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .0
            .restart_task();
    }
}

impl Default for OrbEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Default theme hue (degrees).
#[uniffi::export]
pub fn orb_default_hue() -> f64 {
    f64::from(notdot_orb::theme::DEFAULT_HUE)
}

/// Orb radius as a fraction of the drawing area's shorter side.
#[uniffi::export]
pub fn orb_radius_fraction() -> f64 {
    contract::RADIUS_FRACTION
}

#[uniffi::export]
pub fn orb_contract_version() -> u32 {
    contract::ORB_CONTRACT_VERSION
}

#[uniffi::export]
pub fn orb_shader_sha256() -> String {
    contract::shader_sha256()
}

/// GLSL uniform names in `OrbFrame.packed` order.
#[uniffi::export]
pub fn orb_uniform_names() -> Vec<String> {
    contract::UNIFORMS.iter().map(|s| (*s).to_owned()).collect()
}

/// Android: the canonical fragment shader (GLSL ES 1.00), with its
/// fingerprint header. Generated at build time from `orb.frag`.
#[uniffi::export]
pub fn orb_fragment_glsl() -> String {
    include_str!(concat!(env!("OUT_DIR"), "/orb.frag")).to_owned()
}

#[uniffi::export]
pub fn orb_vertex_glsl() -> String {
    contract::VERTEX_SHADER.to_owned()
}

/// Two-triangle full-screen quad as `position` (x, y) pairs.
#[uniffi::export]
pub fn orb_quad() -> Vec<f32> {
    contract::QUAD.to_vec()
}

/// iOS: Metal source for the fragment stage (`notdot_orb_fragment`).
#[uniffi::export]
pub fn orb_metal_fragment() -> String {
    include_str!(concat!(env!("OUT_DIR"), "/OrbFragment.metal")).to_owned()
}

/// iOS: Metal source for the vertex stage (`notdot_orb_vertex`).
#[uniffi::export]
pub fn orb_metal_vertex() -> String {
    include_str!(concat!(env!("OUT_DIR"), "/OrbVertex.metal")).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> OrbState {
        OrbState {
            mode: OrbMode::Idle,
            task: OrbTask::None,
            hue: orb_default_hue(),
            level: None,
            reduced_motion: false,
            quota: OrbQuota::Normal,
            task_available: false,
            microphone_muted: false,
            wake_ready: false,
            offline: false,
        }
    }

    #[test]
    fn frames_follow_the_desktop_clock_and_contract() {
        let engine = OrbEngine::new();
        let first = engine.frame(state(), 0., 300., 300.).unwrap();
        assert_eq!(first.packed.len(), 14);
        assert_eq!(first.block.len(), 16);
        // 16 ms later at 30 fps: no frame due yet.
        assert!(engine.frame(state(), 16., 300., 300.).is_none());
        assert!(engine.frame(state(), 34., 300., 300.).is_some());
        // Same inputs through the contract directly give the same values.
        let mut animator = OrbAnimator::default();
        let expected = animator.advance(&OrbInput::default(), 0.);
        assert_eq!(first.packed, expected.uniforms.packed(300., 300.).to_vec());
    }

    #[test]
    fn overlays_and_labels_describe_state_without_colour() {
        let engine = OrbEngine::new();
        let frame = engine
            .frame(
                OrbState {
                    mode: OrbMode::Listening,
                    quota: OrbQuota::Exhausted,
                    task_available: true,
                    microphone_muted: true,
                    offline: true,
                    ..state()
                },
                0.,
                300.,
                300.,
            )
            .unwrap();
        let kinds: Vec<_> = frame.overlays.iter().map(|o| o.kind).collect();
        assert_eq!(
            kinds,
            [
                OverlayKind::Glow,
                OverlayKind::Ring,
                OverlayKind::Dot,
                OverlayKind::Bar
            ]
        );
        assert_eq!(
            frame.accessibility_label,
            "Computer offline. Listening. Microphone muted. Voice allowance used up."
        );
        let idle = OrbEngine::new()
            .frame(
                OrbState {
                    task: OrbTask::Attention,
                    wake_ready: true,
                    ..state()
                },
                0.,
                1.,
                1.,
            )
            .unwrap();
        assert_eq!(
            idle.accessibility_label,
            "Ready. A task needs your attention. Wake word on."
        );
    }

    #[test]
    fn generated_shaders_are_current() {
        notdot_orb_gen::verify(&orb_fragment_glsl()).unwrap();
        notdot_orb_gen::verify(&orb_metal_fragment()).unwrap();
        notdot_orb_gen::verify(&orb_metal_vertex()).unwrap();
        assert!(orb_fragment_glsl().ends_with(contract::FRAGMENT_SHADER));
        assert_eq!(orb_uniform_names().len(), 11);
    }
}
