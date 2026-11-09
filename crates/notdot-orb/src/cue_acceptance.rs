//! Debug-build-only graphics acceptance surface. Never present in release WASM.
//! Calls the real shader, fallback and cue renderer at controlled frame phases.
use crate::{
    orb_renderer::{self, Frame},
    shader_renderer::Shader,
    visual_state::{OrbMotion, TaskKind, TaskMotion},
};
use wasm_bindgen::{JsCast, prelude::*};
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};
#[wasm_bindgen]
pub struct CueAcceptance {
    output: CanvasRenderingContext2d,
    gpu: HtmlCanvasElement,
    shader: Shader,
}
#[wasm_bindgen]
impl CueAcceptance {
    #[wasm_bindgen(constructor)]
    pub fn new(canvas: HtmlCanvasElement) -> Result<CueAcceptance, JsValue> {
        let gpu: HtmlCanvasElement = web_sys::window()
            .ok_or(JsValue::NULL)?
            .document()
            .ok_or(JsValue::NULL)?
            .create_element("canvas")?
            .dyn_into()?;
        gpu.set_width(canvas.width());
        gpu.set_height(canvas.height());
        let output = canvas.get_context("2d")?.ok_or(JsValue::NULL)?.dyn_into()?;
        let shader = Shader::new(&gpu)?;
        Ok(Self {
            output,
            gpu,
            shader,
        })
    }
    pub fn draw(
        &mut self,
        hue: f64,
        phase: f64,
        cue: &str,
        reduced: bool,
        background: &str,
    ) -> Result<(), JsValue> {
        let side = f64::from(self.gpu.width());
        let motion = OrbMotion {
            phase,
            speaking: 1.,
            ..Default::default()
        };
        let task = TaskMotion {
            kind: TaskKind::from_cue(cue),
            elapsed: if reduced { 0. } else { phase / 12. },
            strength: if cue == "none" { 0. } else { 1. },
        };
        let frame = Frame {
            center: (side / 2., side / 2.),
            radius: side * 0.17,
            hue,
            energy: 1.,
            reduced,
            motion: &motion,
            task: &task,
        };
        self.output.clear_rect(0., 0., side, side);
        self.output.set_fill_style_str(if background == "shader" {
            "#0b1018"
        } else {
            "white"
        });
        self.output.fill_rect(0., 0., side, side);
        match background {
            "shader" | "white" => {
                if !self.shader.draw(&self.gpu, &frame) {
                    return Err(JsValue::from_str("Shader unavailable"));
                }
                self.output
                    .draw_image_with_html_canvas_element(&self.gpu, 0., 0.)?;
            }
            "canvas" => orb_renderer::draw(
                &self.output,
                Frame {
                    center: frame.center,
                    radius: frame.radius,
                    hue,
                    energy: 1.,
                    reduced,
                    motion: &motion,
                    task: &task,
                },
            ),
            _ => return Err(JsValue::from_str("Unknown background")),
        }
        Ok(())
    }
}
