use wasm_bindgen::{JsCast, JsValue};
use web_sys::{
    AnalyserNode, AudioContext, AudioContextState, GainNode, HtmlAudioElement,
    MediaElementAudioSourceNode, MediaStream, MediaStreamAudioSourceNode,
};
struct Microphone {
    source: MediaStreamAudioSourceNode,
    analyser: AnalyserNode,
    silent: GainNode,
}
impl Drop for Microphone {
    fn drop(&mut self) {
        let _ = self.source.disconnect();
        let _ = self.analyser.disconnect();
        let _ = self.silent.disconnect();
    }
}
struct Playback {
    source: MediaElementAudioSourceNode,
    analyser: AnalyserNode,
}
impl Drop for Playback {
    fn drop(&mut self) {
        let _ = self.source.disconnect();
        let _ = self.analyser.disconnect();
    }
}
#[derive(Default)]
pub(crate) struct Meter {
    context: Option<AudioContext>,
    microphone: Option<Microphone>,
    playback: Option<Playback>,
}
pub(crate) struct Level {
    pub rms: f64,
    pub waveform: [f64; 48],
    pub metered: bool,
}
impl Meter {
    /// Called synchronously from a user gesture, before awaiting network or microphone permission.
    pub fn unlock(&mut self) -> Result<(), JsValue> {
        if self.context.is_none() {
            self.context = Some(AudioContext::new()?);
        }
        if let Some(context) = &self.context {
            let _ = context.resume()?;
        }
        Ok(())
    }
    fn analyser(context: &AudioContext) -> Result<AnalyserNode, JsValue> {
        let analyser = context.create_analyser()?;
        analyser.set_fft_size(256);
        analyser.set_smoothing_time_constant(0.65);
        Ok(analyser)
    }
    pub fn microphone(&mut self, stream: &MediaStream) -> Result<(), JsValue> {
        self.microphone = None;
        let Some(context) = &self.context else {
            return Ok(());
        };
        if context.state() != AudioContextState::Running {
            return Ok(());
        }
        let source = context.create_media_stream_source(stream)?;
        let analyser = Self::analyser(context)?;
        let silent = context.create_gain()?;
        silent.gain().set_value(0.);
        source.connect_with_audio_node(&analyser)?;
        analyser.connect_with_audio_node(&silent)?;
        silent.connect_with_audio_node(&context.destination())?;
        self.microphone = Some(Microphone {
            source,
            analyser,
            silent,
        });
        Ok(())
    }
    pub fn playback(&mut self, audio: &HtmlAudioElement) -> Result<bool, JsValue> {
        self.playback = None;
        let Some(context) = &self.context else {
            return Ok(false);
        };
        // Preserve ordinary HTML audio output if Web Audio could not be unlocked.
        if context.state() != AudioContextState::Running {
            return Ok(false);
        }
        let source = context.create_media_element_source(audio.unchecked_ref())?;
        let analyser = Self::analyser(context)?;
        source.connect_with_audio_node(&analyser)?;
        analyser.connect_with_audio_node(&context.destination())?;
        self.playback = Some(Playback { source, analyser });
        Ok(true)
    }
    pub fn clear_microphone(&mut self) {
        self.microphone = None;
    }
    pub fn clear_playback(&mut self) {
        self.playback = None;
    }
    pub fn sample(&self, listening: bool) -> Level {
        let analyser = if listening {
            self.microphone.as_ref().map(|m| &m.analyser)
        } else {
            self.playback.as_ref().map(|m| &m.analyser)
        };
        let mut bytes = [128; 256];
        if let Some(analyser) = analyser {
            analyser.get_byte_time_domain_data(&mut bytes);
        }
        Level {
            rms: crate::visual_state::rms(&bytes),
            waveform: std::array::from_fn(|i| {
                ((f64::from(bytes[i * bytes.len() / 48]) - 128.) / 128.).abs()
            }),
            metered: analyser.is_some(),
        }
    }
}
