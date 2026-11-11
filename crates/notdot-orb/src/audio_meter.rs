use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use web_sys::{
    AnalyserNode, AudioContext, AudioContextState, Event, GainNode, HtmlAudioElement,
    MediaElementAudioSourceNode, MediaStream, MediaStreamAudioSourceNode,
};
/// Wake cue envelope, in seconds after the cue starts. The level and the
/// 80 ms fade-out are unchanged; only the onset rises over 5 ms.
const CUE_LEVEL: f32 = 0.035;
const CUE_ATTACK: f64 = 0.005;
const CUE_RELEASE_END: f64 = 0.08;
const CUE_STOP: f64 = 0.09;
/// (gain, time) automation points: silent start, peak, then fade to silence.
fn cue_envelope(now: f64) -> [(f32, f64); 3] {
    [
        (0., now),
        (CUE_LEVEL, now + CUE_ATTACK),
        (0., now + CUE_RELEASE_END),
    ]
}
/// Linux WebKit drops the first 15-40 ms of sound after silence, which cut
/// off the attack and left an audible step. A -70 dBFS pre-roll absorbs that
/// loss; the cue then starts once output is flowing. Measured in the Linux
/// VM: 30-40 ms leads still clipped some cues, 50 ms and longer never did.
const LINUX_CUE_LEAD: f64 = 0.08;
const PRE_ROLL_LEVEL: f32 = 0.0003;
/// When the cue starts, and the (start, stop) of any inaudible pre-roll.
fn cue_schedule(now: f64, linux: bool) -> (f64, Option<(f64, f64)>) {
    if linux {
        let start = now + LINUX_CUE_LEAD;
        // Overlap the cue briefly so output never falls silent between them.
        (start, Some((now, start + CUE_ATTACK)))
    } else {
        (now, None)
    }
}
fn linux_webkit() -> bool {
    web_sys::window()
        .and_then(|window| window.navigator().user_agent().ok())
        .is_some_and(|agent| agent.contains("Linux"))
}
// Keep speech on the ordinary media output in WebKit webviews. Routing a
// media element through Web Audio broke sentence playback on macOS and Linux.
// The speaking animation has an unmetered fallback; microphone metering and
// local activation cues remain available.
fn direct_speech_output(agent: &str) -> bool {
    agent.contains("Macintosh") || agent.contains("Linux")
}
fn needs_direct_speech_output() -> bool {
    web_sys::window()
        .and_then(|window| window.navigator().user_agent().ok())
        .is_some_and(|agent| direct_speech_output(&agent))
}
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
    gain: GainNode,
    audio: HtmlAudioElement,
    on_playing: Closure<dyn FnMut(Event)>,
}
impl Drop for Playback {
    fn drop(&mut self) {
        let _ = self.audio.remove_event_listener_with_callback(
            "playing",
            self.on_playing.as_ref().unchecked_ref(),
        );
        let _ = self.source.disconnect();
        let _ = self.analyser.disconnect();
        let _ = self.gain.disconnect();
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
    /// A short, local activation cue; never uses a voice provider.
    pub fn wake_cue(&mut self) -> Result<(), JsValue> {
        self.unlock()?;
        let context = self.context.as_ref().expect("audio context initialized");
        let tone = |level: f32| -> Result<_, JsValue> {
            let tone = context.create_oscillator()?;
            tone.set_type(web_sys::OscillatorType::Sine);
            tone.frequency().set_value(740.);
            let volume = context.create_gain()?;
            volume.gain().set_value(level);
            tone.connect_with_audio_node(&volume)?;
            volume.connect_with_audio_node(&context.destination())?;
            Ok((tone, volume))
        };
        let (start, pre_roll) = cue_schedule(context.current_time(), linux_webkit());
        if let Some((from, until)) = pre_roll {
            let (pre_roll, _) = tone(PRE_ROLL_LEVEL)?;
            pre_roll.start_with_when(from)?;
            pre_roll.stop_with_when(until)?;
        }
        let (cue, volume) = tone(0.)?;
        // Start silent and rise briefly: a step to full level ticks audibly.
        // Explicit events keep the shape the same on every Web Audio engine.
        let gain = volume.gain();
        let [(silent, at), rise, fall] = cue_envelope(start);
        gain.set_value_at_time(silent, at)?;
        gain.linear_ramp_to_value_at_time(rise.0, rise.1)?;
        gain.linear_ramp_to_value_at_time(fall.0, fall.1)?;
        cue.start_with_when(start)?;
        cue.stop_with_when(start + CUE_STOP)?;
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
        let context = self.context.as_ref().ok_or_else(|| {
            JsValue::from_str("Microphone level monitoring is unavailable. Try again.")
        })?;
        if context.state() != AudioContextState::Running {
            return Err(JsValue::from_str(
                "Microphone level monitoring has not started. Try again.",
            ));
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
        // WebKit media-source routing can repeat starts or truncate speech.
        // Keep the decoder's direct output and the unmetered speaking animation.
        if needs_direct_speech_output() {
            return Ok(false);
        }
        let Some(context) = &self.context else {
            return Ok(false);
        };
        // Preserve ordinary HTML audio output if Web Audio could not be unlocked.
        if context.state() != AudioContextState::Running {
            return Ok(false);
        }
        // Keep the output graph attached to the same media element across
        // sentences and turns. Rebuilding the destination is unnecessary and
        // avoids disconnecting and reconnecting the output at every reply.
        if let Some(playback) = &self.playback
            && playback.audio == *audio
        {
            let now = context.current_time();
            playback.gain.gain().cancel_scheduled_values(now)?;
            playback.gain.gain().set_value_at_time(0., now)?;
            return Ok(true);
        }
        self.playback = None;
        let source = context.create_media_element_source(audio.unchecked_ref())?;
        let analyser = Self::analyser(context)?;
        let gain = context.create_gain()?;
        gain.gain().set_value(0.);
        source.connect_with_audio_node(&gain)?;
        gain.connect_with_audio_node(&analyser)?;
        analyser.connect_with_audio_node(&context.destination())?;
        // Start the ramp at actual playback, not before network/decoder startup.
        let ramp_context = context.clone();
        let ramp_gain = gain.clone();
        let on_playing = Closure::<dyn FnMut(Event)>::new(move |_| {
            let now = ramp_context.current_time();
            let parameter = ramp_gain.gain();
            let _ = parameter.cancel_scheduled_values(now);
            let _ = parameter.set_value_at_time(0., now);
            let _ = parameter.linear_ramp_to_value_at_time(1., now + 0.025);
        });
        audio.add_event_listener_with_callback("playing", on_playing.as_ref().unchecked_ref())?;
        self.playback = Some(Playback {
            source,
            analyser,
            gain,
            audio: audio.clone(),
            on_playing,
        });
        Ok(true)
    }
    pub fn clear_microphone(&mut self) {
        self.microphone = None;
    }
    pub fn playback_element(&self) -> Option<HtmlAudioElement> {
        self.playback
            .as_ref()
            .map(|playback| playback.audio.clone())
    }
    pub fn clear_playback(&mut self) {
        if let (Some(playback), Some(context)) = (&self.playback, &self.context) {
            let now = context.current_time();
            let _ = playback.gain.gain().cancel_scheduled_values(now);
            let _ = playback.gain.gain().set_value_at_time(0., now);
        }
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
            metered: analyser.is_some(),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    /// Linear Web Audio automation evaluated at sample time `t`.
    fn level(points: &[(f32, f64); 3], t: f64) -> f64 {
        let pairs = points.windows(2);
        for pair in pairs {
            let ((a, ta), (b, tb)) = (pair[0], pair[1]);
            if t <= tb {
                let x = ((t - ta) / (tb - ta)).clamp(0., 1.);
                return f64::from(a) + (f64::from(b) - f64::from(a)) * x;
            }
        }
        0.
    }
    #[test]
    fn wake_cue_rises_smoothly_without_changing_its_timing_or_level() {
        let now = 12.5;
        let points = cue_envelope(now);
        let rate = 48_000.;
        let samples: Vec<f64> = (0..(rate * CUE_STOP) as usize)
            .map(|n| level(&points, now + n as f64 / rate))
            .collect();
        let peak = samples.iter().copied().fold(0., f64::max);
        let largest_step = samples
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0., f64::max);
        // Same audible level and duration as before the attack was added.
        assert!((peak - f64::from(CUE_LEVEL)).abs() < 1e-6);
        assert_eq!(samples[0], 0.);
        assert!(level(&points, now + 0.04) > peak / 3.);
        assert_eq!(level(&points, now + 0.08), 0.);
        assert!(CUE_STOP > points[2].1 - now);
        // No step: the gain never moves more than 1% of its peak per sample.
        assert!(largest_step < peak / 100., "step {largest_step}");
        // Still crisp: full level within 10 ms.
        assert!(level(&points, now + 0.01) > peak * 0.9);
    }
    #[test]
    fn linux_cue_waits_behind_an_inaudible_pre_roll_and_others_do_not() {
        let now = 3.25;
        assert_eq!(cue_schedule(now, false), (now, None));
        let (start, pre_roll) = cue_schedule(now, true);
        let (from, until) = pre_roll.expect("Linux pre-roll");
        // Starts at once and keeps output flowing into the cue's attack.
        assert_eq!(from, now);
        assert!(until > start && until <= start + CUE_ATTACK);
        // Longer than the 50 ms that kept every measured Linux cue intact,
        // but short enough to still mark the start of listening promptly.
        let lead = start - now;
        assert!((0.05..=0.1).contains(&lead), "lead {lead}");
        // Inaudible: at least 60 dB below full scale and the cue itself.
        let below_full = -20. * f64::from(PRE_ROLL_LEVEL).log10();
        let below_cue = 20. * f64::from(CUE_LEVEL / PRE_ROLL_LEVEL).log10();
        assert!(
            below_full >= 60. && below_cue >= 30.,
            "{below_full} {below_cue}"
        );
    }
}
