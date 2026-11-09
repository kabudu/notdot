//! Layered Canvas 2D glass, kept within the transparent desktop canvas.
//! Fixed point counts and whole-path strokes avoid per-segment glow passes.
use crate::visual_state::{OrbMotion, TaskKind, TaskMotion};
use std::f64::consts::TAU;
use web_sys::{CanvasGradient, CanvasRenderingContext2d as Context};

pub(crate) struct Frame<'a> {
    pub center: (f64, f64),
    pub radius: f64,
    pub hue: f64,
    pub energy: f64,
    pub reduced: bool,
    pub motion: &'a OrbMotion,
    pub task: &'a TaskMotion,
}

struct Palette {
    hue: f64,
}
/// Volumetric-looking light belongs to the orb, without separate glyphs.
fn task_light(context: &Context, palette: &Palette, r: f64, task: &TaskMotion, reduced: bool) {
    if task.kind == TaskKind::None || task.strength < 0.001 {
        return;
    }
    let t = if reduced { 0. } else { task.elapsed };
    context.save();
    context.set_global_alpha(task.strength * 0.82);
    context.set_fill_style_str("#050c17");
    context.fill_rect(-r, -r, r * 2., r * 2.);
    context.set_global_alpha(task.strength);
    let hue = palette.hue.rem_euclid(360.);
    let lift = if (215.0..285.0).contains(&hue) {
        10.
    } else {
        0.
    };
    let cloud = |x: f64, y: f64, size: f64, alpha: f64| {
        if let Ok(g) = context.create_radial_gradient(x, y, 0., x, y, size) {
            let _ = g.add_color_stop(0., &palette.colour(alpha, 60. + lift));
            let _ = g.add_color_stop(0.25, &palette.colour(alpha * 0.9, 58. + lift));
            let _ = g.add_color_stop(1., &palette.colour(0., 60.));
            context.set_fill_style_canvas_gradient(&g);
            context.begin_path();
            let _ = context.arc(x, y, size, 0., TAU);
            context.fill();
        }
    };
    match task.kind {
        TaskKind::Working => {
            let offset = 0.14 * (t * 0.3).sin() - 0.22 * (t * 0.65).sin();
            for x in [-0.7_f64, -0.35, 0., 0.35, 0.7] {
                let y = 0.48 * x + 0.14 * (x * 3.).sin() + offset;
                cloud(x * r, y * r, r * 0.3, 1.);
            }
        }
        TaskKind::Attention => {
            let cycle = t % 6.;
            let breath =
                (-((cycle - 1.) / 0.5).powi(2)).exp() + (-((cycle - 2.2) / 0.5).powi(2)).exp();
            cloud(0., 0., r * 0.72, 0.82 + 0.18 * breath);
        }
        TaskKind::Finished => {
            let size = if reduced {
                0.85
            } else {
                0.3 + 0.55 * (1. - (-t * 0.9).exp())
            };
            cloud(0., 0., r * size, 1.);
        }
        TaskKind::Review => {
            for (x, y) in [(-0.34, 0.32), (0., 0.4), (0.34, 0.32)] {
                cloud(x * r, y * r, r * 0.34, 1.);
            }
        }
        TaskKind::None => {}
    }
    context.restore();
}

impl Palette {
    fn colour(&self, alpha: f64, light: f64) -> String {
        format!("hsla({},85%,{}%,{alpha})", self.hue, light)
    }
    fn gradient(&self, gradient: &CanvasGradient, stops: &[(f32, f64, f64)]) {
        for &(position, alpha, light) in stops {
            let _ = gradient.add_color_stop(position, &self.colour(alpha, light));
        }
    }
}

fn circle(context: &Context, radius: f64) {
    context.begin_path();
    let _ = context.arc(0., 0., radius, 0., TAU);
}

fn glow(context: &Context, palette: &Palette, radius: f64, energy: f64) {
    if let Ok(gradient) =
        context.create_radial_gradient(0., 0., radius * 0.25, 0., 0., radius * 2.05)
    {
        palette.gradient(
            &gradient,
            &[
                (0., 0.07, 65.),
                (0.4, 0.10 + energy * 0.03, 62.),
                (1., 0., 65.),
            ],
        );
        context.set_fill_style_canvas_gradient(&gradient);
        circle(context, radius * 2.05);
        context.fill();
    }
}

fn stars(context: &Context, palette: &Palette, radius: f64, phase: f64) {
    for i in 0..42 {
        let angle = i as f64 * 2.399_963;
        let distance = radius * (1.38 + (i * 37 % 97) as f64 / 97. * 1.1);
        let x = angle.cos() * distance;
        let y = angle.sin() * distance;
        let alpha = 0.20 + 0.13 * (phase + i as f64 * 0.81).sin();
        context.set_fill_style_str(&palette.colour(alpha, 83.));
        context.begin_path();
        let _ = context.arc(x, y, if i % 11 == 0 { 1.05 } else { 0.5 }, 0., TAU);
        context.fill();
        if i % 11 == 0 {
            context.set_line_width(0.5);
            context.set_stroke_style_str(&palette.colour(alpha * 0.5, 90.));
            context.begin_path();
            context.move_to(x - 2.5, y);
            context.line_to(x + 2.5, y);
            context.move_to(x, y - 2.5);
            context.line_to(x, y + 2.5);
            context.stroke();
        }
    }
}

fn shell(context: &Context, palette: &Palette, radius: f64, phase: f64, alpha: f64) {
    context.save();
    let _ = context.rotate(phase * 0.18);
    let _ = context.scale(1., 0.95 + (phase * 0.25).sin() * 0.02);
    {
        let gradient = context.create_linear_gradient(-radius, -radius, radius, radius);
        palette.gradient(
            &gradient,
            &[
                (0., alpha * 0.15, 70.),
                (0.2, alpha, 85.),
                (0.45, alpha * 0.08, 65.),
                (0.8, alpha * 0.4, 78.),
                (1., alpha * 0.1, 65.),
            ],
        );
        context.set_stroke_style_canvas_gradient(&gradient);
    }
    context.set_line_width(0.8);
    circle(context, radius);
    context.stroke();
    context.restore();
}

/// Project a wisp onto a sphere. A single path gives a smooth, continuous glow.
fn ribbon(context: &Context, palette: &Palette, radius: f64, index: usize, phase: f64) {
    let rotation = phase * 0.3 + index as f64 * 0.82;
    let tilt = 0.52 + index as f64 * 0.14;
    context.begin_path();
    for j in 0..=96 {
        let angle = j as f64 / 96. * TAU;
        let latitude = 0.22 * (angle * 2. + index as f64 * 0.9 + phase * 0.18).sin()
            + (index as f64 * 1.7).sin() * 0.52;
        let x = latitude.cos() * angle.cos();
        let y = latitude.sin();
        let z = latitude.cos() * angle.sin();
        let px = (x * rotation.cos() + z * rotation.sin()) * radius;
        let depth = z * rotation.cos() - x * rotation.sin();
        let py = (y * tilt.cos() - depth * tilt.sin()) * radius;
        if j == 0 {
            context.move_to(px, py);
        } else {
            context.line_to(px, py);
        }
    }
    {
        let gradient = context.create_linear_gradient(-radius, -radius, radius, radius);
        palette.gradient(
            &gradient,
            &[
                (0., 0.05, 80.),
                (0.23, 0.24, 87.),
                (0.49, 0.02, 70.),
                (0.7, 0.10, 80.),
                (1., 0.02, 70.),
            ],
        );
        context.set_stroke_style_canvas_gradient(&gradient);
    }
    context.set_line_width(0.65);
    context.stroke();
}

fn flowing_light(context: &Context, palette: &Palette, radius: f64, phase: f64, energy: f64) {
    context.save();
    let _ = context.set_global_composite_operation("screen");
    let _ = context.rotate(-0.62 + (phase * 0.4).sin() * 0.1);
    for i in 0..2 {
        let y = (i as f64 - 0.5) * radius * 0.85;
        let drift = (phase * 0.45 + i as f64 * 0.8).sin() * radius * 0.12;
        {
            let gradient = context.create_linear_gradient(-radius, -radius, radius, radius);
            palette.gradient(
                &gradient,
                &[
                    (0., 0.01, 70.),
                    (0.26, 0.12, 78.),
                    (0.48, 0.35 + energy * 0.12, 84.),
                    (0.72, 0.08, 72.),
                    (1., 0.01, 65.),
                ],
            );
            context.set_fill_style_canvas_gradient(&gradient);
        }
        context.begin_path();
        context.move_to(-radius * 1.15, y - radius * 0.4);
        context.bezier_curve_to(
            -radius * 0.3,
            y + radius * 0.4 + drift,
            radius * 0.25,
            y - radius * 0.7,
            radius * 1.2,
            y - radius * 0.12,
        );
        context.bezier_curve_to(
            radius * 0.25,
            y - radius * 0.37,
            -radius * 0.3,
            y + radius * 0.58 + drift,
            -radius * 1.15,
            y - radius * 0.28,
        );
        context.close_path();
        context.fill();
    }
    context.restore();
}

pub(crate) fn draw(context: &Context, frame: Frame<'_>) {
    let Frame {
        center: (x, y),
        radius: base_radius,
        hue,
        energy,
        reduced,
        motion,
        task,
    } = frame;
    let palette = Palette {
        hue: hue + motion.error * ((12. - hue + 540.) % 360. - 180.),
    };
    let phase = motion.phase;
    let speech = energy * motion.speaking;
    let pulse = if reduced {
        0.
    } else {
        energy * (motion.speaking * 0.035 + (motion.listening + motion.followup) * 0.015)
            + phase.sin() * 0.005
    };
    let radius = base_radius * 1.35 * (1. + pulse);
    context.save();
    let _ = context.translate(x, y);
    glow(context, &palette, radius, speech);
    stars(context, &palette, radius, phase);
    shell(
        context,
        &palette,
        radius * (1.12 + motion.listening * 0.10 + motion.followup * 0.10),
        phase,
        0.22 + motion.listening * 0.22 + motion.followup * 0.17,
    );
    if motion.followup > 0.01 {
        shell(
            context,
            &palette,
            radius * 1.34,
            -phase,
            motion.followup * 0.25,
        );
    }
    if motion.thinking > 0.01 {
        context.save();
        let _ = context.rotate(-phase * 0.7);
        {
            let gradient = context.create_linear_gradient(-radius, 0., radius, 0.);
            palette.gradient(
                &gradient,
                &[
                    (0., 0., 80.),
                    (0.8, motion.thinking * 0.65, 85.),
                    (1., motion.thinking * 0.02, 75.),
                ],
            );
            context.set_stroke_style_canvas_gradient(&gradient);
        }
        context.set_line_width(1.8);
        context.set_line_cap("round");
        context.begin_path();
        let _ = context.arc(0., 0., radius * 1.26, -2.5, 0.4);
        context.stroke();
        context.restore();
    }
    context.save();
    circle(context, radius);
    context.clip();
    context.set_fill_style_str("#050c17");
    context.fill_rect(-radius, -radius, radius * 2., radius * 2.);
    if let Ok(gradient) = context.create_radial_gradient(
        -radius * 0.42,
        -radius * 0.42,
        0.,
        -radius * 0.42,
        -radius * 0.42,
        radius * 1.62,
    ) {
        palette.gradient(
            &gradient,
            &[
                (0., 0.48, 70.),
                (0.22, 0.32, 62.),
                (0.6, 0.16, 48.),
                (0.87, 0.38, 60.),
                (1., 0.09, 40.),
            ],
        );
        context.set_fill_style_canvas_gradient(&gradient);
        context.fill_rect(-radius, -radius, radius * 2., radius * 2.);
    }
    for i in 0..8 {
        ribbon(context, &palette, radius, i, phase);
    }
    flowing_light(context, &palette, radius, phase, speech);
    task_light(context, &palette, radius, task, reduced);
    if let Ok(gradient) = context.create_radial_gradient(0., 0., radius * 0.82, 0., 0., radius) {
        palette.gradient(
            &gradient,
            &[
                (0., 0., 75.),
                (0.65, 0.025, 75.),
                (0.95, 0.12, 84.),
                (1., 0.7, 90.),
            ],
        );
        context.set_fill_style_canvas_gradient(&gradient);
        circle(context, radius);
        context.fill();
    }
    {
        let gradient = context.create_linear_gradient(-radius, -radius, radius, radius);
        palette.gradient(
            &gradient,
            &[
                (0., 0.95, 98.),
                (0.22, 0.58, 87.),
                (0.42, 0.06, 65.),
                (0.72, 0.17, 78.),
                (1., 0.6, 90.),
            ],
        );
        context.set_stroke_style_canvas_gradient(&gradient);
        context.set_line_width(1.5);
        circle(context, radius - 0.75);
        context.stroke();
    }
    if let Ok(gradient) = context.create_radial_gradient(
        -radius * 0.59,
        -radius * 0.62,
        0.,
        -radius * 0.59,
        -radius * 0.62,
        radius * 0.75,
    ) {
        palette.gradient(
            &gradient,
            &[
                (0., 0.64, 96.),
                (0.17, 0.38, 90.),
                (0.43, 0.045, 75.),
                (1., 0., 70.),
            ],
        );
        context.set_fill_style_canvas_gradient(&gradient);
        context.fill_rect(-radius, -radius, radius * 2., radius * 2.);
    }
    context.restore();
    context.set_stroke_style_str(&palette.colour(0.45, 94.));
    context.set_line_width(1.5);
    context.set_shadow_color(&palette.colour(0.5, 85.));
    context.set_shadow_blur(if reduced { 0. } else { 12. + speech * 5. });
    context.begin_path();
    let _ = context.arc(0., 0., radius, -2.9, -1.45);
    context.stroke();
    context.restore();
}
