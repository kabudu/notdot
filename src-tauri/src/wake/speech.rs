//! Speech classification for request endpointing; sound energy alone cannot
//! distinguish the user's voice from fans, keyboard taps or speaker noise.
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use sherpa_onnx::{SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};
use std::{collections::VecDeque, path::Path};

pub(super) struct SpeechGate {
    vad: VoiceActivityDetector,
    rate: u32,
    resampler: Option<SincFixedIn<f32>>,
    pending: VecDeque<f32>,
}
impl SpeechGate {
    pub(super) fn new(models: &Path) -> Result<Self, String> {
        let config = VadModelConfig {
            silero_vad: SileroVadModelConfig {
                model: Some(
                    models
                        .join("silero_vad.onnx")
                        .to_string_lossy()
                        .into_owned(),
                ),
                threshold: 0.5,
                min_silence_duration: 0.1,
                min_speech_duration: 0.1,
                window_size: 512,
                max_speech_duration: 30.,
            },
            sample_rate: 16000,
            num_threads: 1,
            provider: Some("cpu".into()),
            ..Default::default()
        };
        let vad = VoiceActivityDetector::create(&config, 32.)
            .ok_or("Could not load local speech detection")?;
        Ok(Self {
            vad,
            rate: 0,
            resampler: None,
            pending: VecDeque::new(),
        })
    }
    pub(super) fn reset(&mut self) {
        self.vad.reset();
        self.vad.clear();
        self.pending.clear();
        if let Some(r) = &mut self.resampler {
            r.reset();
        }
    }
    pub(super) fn accept(&mut self, samples: &[f32], rate: u32) -> Result<bool, String> {
        if !(8000..=192000).contains(&rate) {
            return Err("Unsupported microphone sample rate".into());
        }
        if rate != self.rate {
            self.reset();
            self.rate = rate;
            self.resampler = if rate == 16000 {
                None
            } else {
                let params = SincInterpolationParameters {
                    sinc_len: 128,
                    f_cutoff: 0.95,
                    interpolation: SincInterpolationType::Linear,
                    oversampling_factor: 128,
                    window: WindowFunction::BlackmanHarris2,
                };
                Some(
                    SincFixedIn::new(16000. / f64::from(rate), 1., params, rate as usize / 50, 1)
                        .map_err(|e| e.to_string())?,
                )
            };
        }
        let frame_size = rate as usize / 50;
        for chunk in samples.chunks(frame_size) {
            self.pending.extend(chunk.iter().copied());
            while self.pending.len() >= frame_size {
                let frame: Vec<_> = self.pending.drain(..frame_size).collect();
                if let Some(r) = &mut self.resampler {
                    let audio = r.process(&[frame], None).map_err(|e| e.to_string())?;
                    self.vad.accept_waveform(&audio[0]);
                } else {
                    self.vad.accept_waveform(&frame);
                }
                while !self.vad.is_empty() {
                    self.vad.pop();
                }
            }
        }
        Ok(self.vad.detected())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "private local voice fixture; do not commit audio"]
    fn local_speech_fixture() {
        let _models = super::super::tests::MODEL_FIXTURE.lock().unwrap();
        let models = super::super::fixture_root().join("resources/wake");
        let mut gate = SpeechGate::new(&models).unwrap();
        let path = std::env::var("NOTDOT_WAKE_FIXTURE").unwrap();
        let mut wav = hound::WavReader::open(path).unwrap();
        let rate = wav.spec().sample_rate;
        let audio: Vec<_> = wav
            .samples::<i16>()
            .map(|s| f32::from(s.unwrap()) / 32768.)
            .take(rate as usize * 30 + 1)
            .collect();
        assert!(audio.len() <= rate as usize * 30);
        let offset = std::env::var("NOTDOT_VAD_START_SEC")
            .unwrap_or("0".into())
            .parse::<usize>()
            .unwrap()
            * rate as usize;
        let mut count = 0;
        for frame in audio[offset..].chunks(rate as usize / 50) {
            if gate.accept(frame, rate).unwrap() {
                count += 1;
            }
        }
        eprintln!("classified speech_packets={count}");
        assert!(count > 10, "the private spoken request must be detected");
    }
    #[test]
    fn spoken_request_ends_despite_non_speech_background_sound() {
        let _models = super::super::tests::MODEL_FIXTURE.lock().unwrap();
        let models = super::super::fixture_root().join("resources/wake");
        let mut wav = hound::WavReader::open(
            super::super::fixture_root().join("tests/fixtures/negative.wav"),
        )
        .unwrap();
        let samples: Vec<_> = wav
            .samples::<i16>()
            .map(|s| f32::from(s.unwrap()) / 32768.)
            .collect();
        for rate in [16000, 44100, 48000] {
            let mut gate = SpeechGate::new(&models).unwrap();
            let input = if rate == 16000 {
                samples.clone()
            } else {
                let params = SincInterpolationParameters {
                    sinc_len: 128,
                    f_cutoff: 0.95,
                    interpolation: SincInterpolationType::Linear,
                    oversampling_factor: 128,
                    window: WindowFunction::BlackmanHarris2,
                };
                let mut r =
                    SincFixedIn::new(f64::from(rate) / 16000., 1., params, samples.len(), 1)
                        .unwrap();
                r.process(&[samples.as_slice()], None).unwrap().remove(0)
            };
            let mut capture = super::super::Capture::default();
            let mut heard = false;
            for frame in input.chunks(rate as usize / 50) {
                let voice = gate.accept(frame, rate).unwrap();
                heard |= voice;
                assert!(matches!(
                    capture.accept(frame, rate, voice),
                    super::super::End::Continue
                ));
            }
            assert!(heard, "spoken request must be detected");
            let mut submitted = false;
            for block in 0..100 {
                let noise: Vec<_> = (0..rate as usize / 50)
                    .map(|i| {
                        let t = (block * (rate as usize / 50) + i) as f32 / rate as f32;
                        0.025 * (std::f32::consts::TAU * 120. * t).sin()
                    })
                    .collect();
                let voice = gate.accept(&noise, rate).unwrap();
                if matches!(
                    capture.accept(&noise, rate, voice),
                    super::super::End::Submit
                ) {
                    submitted = true;
                    break;
                }
            }
            assert!(
                submitted,
                "steady background sound must not extend the request indefinitely"
            );
            gate.reset();
            assert!(!gate.accept(&[0.; 320], rate).unwrap());
        }
    }
}
