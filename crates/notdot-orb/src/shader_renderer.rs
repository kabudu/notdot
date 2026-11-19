//! GPU fluid light over an opaque ink core. Canvas remains the fallback.
use crate::contract;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use web_sys::{HtmlCanvasElement, WebGlProgram, WebGlRenderingContext as Gl, WebGlShader};

pub(crate) struct Shader {
    gl: Gl,
    program: WebGlProgram,
    buffer: web_sys::WebGlBuffer,
    lost: bool,
    failed: bool,
    uniforms: [Option<web_sys::WebGlUniformLocation>; 11],
}
impl Shader {
    pub fn new(canvas: &HtmlCanvasElement) -> Result<Self, JsValue> {
        let options = js_sys::Object::new();
        js_sys::Reflect::set(&options, &"alpha".into(), &true.into())?;
        js_sys::Reflect::set(&options, &"antialias".into(), &false.into())?;
        js_sys::Reflect::set(&options, &"premultipliedAlpha".into(), &true.into())?;
        let gl: Gl = canvas
            .get_context_with_context_options("webgl", &options)?
            .ok_or_else(|| JsValue::from_str("WebGL unavailable"))?
            .dyn_into()?;
        let prevent = Closure::<dyn FnMut(web_sys::Event)>::new(|event: web_sys::Event| {
            event.prevent_default()
        });
        canvas.add_event_listener_with_callback(
            "webglcontextlost",
            prevent.as_ref().unchecked_ref(),
        )?;
        prevent.forget(); // One listener for the lifetime of the animation canvas.
        Self::setup(gl)
    }
    fn compile(gl: &Gl, kind: u32, source: &str) -> Result<WebGlShader, JsValue> {
        let shader = gl
            .create_shader(kind)
            .ok_or_else(|| JsValue::from_str("Shader allocation failed"))?;
        gl.shader_source(&shader, source);
        gl.compile_shader(&shader);
        if gl
            .get_shader_parameter(&shader, Gl::COMPILE_STATUS)
            .as_bool()
            != Some(true)
        {
            let error = gl.get_shader_info_log(&shader).unwrap_or_default();
            gl.delete_shader(Some(&shader));
            return Err(JsValue::from_str(&error));
        }
        Ok(shader)
    }
    fn setup(gl: Gl) -> Result<Self, JsValue> {
        let vertex = Self::compile(&gl, Gl::VERTEX_SHADER, contract::VERTEX_SHADER)?;
        let fragment = match Self::compile(&gl, Gl::FRAGMENT_SHADER, contract::FRAGMENT_SHADER) {
            Ok(fragment) => fragment,
            Err(error) => {
                gl.delete_shader(Some(&vertex));
                return Err(error);
            }
        };
        let Some(program) = gl.create_program() else {
            gl.delete_shader(Some(&vertex));
            gl.delete_shader(Some(&fragment));
            return Err(JsValue::from_str("Program allocation failed"));
        };
        gl.attach_shader(&program, &vertex);
        gl.attach_shader(&program, &fragment);
        gl.link_program(&program);
        gl.delete_shader(Some(&vertex));
        gl.delete_shader(Some(&fragment));
        if gl
            .get_program_parameter(&program, Gl::LINK_STATUS)
            .as_bool()
            != Some(true)
        {
            let error = gl.get_program_info_log(&program).unwrap_or_default();
            gl.delete_program(Some(&program));
            return Err(JsValue::from_str(&error));
        }
        gl.use_program(Some(&program));
        let Some(buffer) = gl.create_buffer() else {
            gl.delete_program(Some(&program));
            return Err(JsValue::from_str("Buffer allocation failed"));
        };
        gl.bind_buffer(Gl::ARRAY_BUFFER, Some(&buffer));
        let vertices = js_sys::Float32Array::from(&contract::QUAD[..]);
        gl.buffer_data_with_array_buffer_view(Gl::ARRAY_BUFFER, &vertices, Gl::STATIC_DRAW);
        let position = gl.get_attrib_location(&program, "position");
        if position < 0 {
            gl.delete_buffer(Some(&buffer));
            gl.delete_program(Some(&program));
            return Err(JsValue::from_str("Shader position missing"));
        }
        gl.enable_vertex_attrib_array(position as u32);
        gl.vertex_attrib_pointer_with_i32(position as u32, 2, Gl::FLOAT, false, 0, 0);
        let uniforms = contract::UNIFORMS.map(|name| gl.get_uniform_location(&program, name));
        Ok(Self {
            gl,
            program,
            buffer,
            lost: false,
            failed: false,
            uniforms,
        })
    }
    pub fn draw(&mut self, canvas: &HtmlCanvasElement, uniforms: &contract::Uniforms) -> bool {
        if self.failed {
            return false;
        }
        if self.gl.is_context_lost() {
            self.lost = true;
            return false;
        }
        if self.lost {
            match Self::setup(self.gl.clone()) {
                Ok(restored) => *self = restored,
                Err(_) => {
                    self.failed = true;
                    return false;
                }
            }
        }
        let gl = &self.gl;
        gl.viewport(0, 0, canvas.width() as i32, canvas.height() as i32);
        gl.use_program(Some(&self.program));
        let packed = uniforms.packed(canvas.width() as f32, canvas.height() as f32);
        gl.uniform2f(self.uniforms[0].as_ref(), packed[0], packed[1]);
        gl.uniform3f(self.uniforms[1].as_ref(), packed[2], packed[3], packed[4]);
        for (location, value) in self.uniforms[2..].iter().zip(&packed[5..]) {
            gl.uniform1f(location.as_ref(), *value);
        }
        gl.draw_arrays(Gl::TRIANGLES, 0, 6);
        true
    }
}
impl Drop for Shader {
    fn drop(&mut self) {
        // Handles from a lost context are already invalid. Deleting those
        // after restoration would queue INVALID_OPERATION in the new context.
        if !self.lost {
            self.gl.delete_buffer(Some(&self.buffer));
            self.gl.delete_program(Some(&self.program));
        }
    }
}
