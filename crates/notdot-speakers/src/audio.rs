//! PCM handling and sample quality. All work is bounded by [`MAX_SECONDS`].
use serde::Serialize;

/// Longest audio considered for one embedding.
pub const MAX_SECONDS: f32 = 8.0;
/// Minimum voiced speech for a usable sample.
pub const MIN_VOICED_SECONDS: f32 = 1.2;
const FRAME_MS: u32 = 20;
/// Frames below this RMS (about −42 dBFS) are treated as silence.
const VOICED_RMS: f32 = 0.008;
const CLIP_LEVEL: f32 = 0.999;
const MAX_CLIPPED: f32 = 0.01;
/// Voiced RMS below about −38 dBFS is too quiet to trust.
const MIN_VOICED_RMS: f32 = 0.0125;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SampleIssue {
    TooShort,
    TooQuiet,
    Clipped,
    UnsupportedRate,
    /// Enrolment only: did not sound like the earlier samples.
    Inconsistent,
    /// The model could not produce an embedding from this audio.
    NotAssessable,
}
impl SampleIssue {
    /// Short, friendly, jargon-free guidance for a retry.
    pub fn guidance(self) -> &'static str {
        match self {
            Self::TooShort => "That was a little short. Please say the whole phrase.",
            Self::TooQuiet => "I could barely hear that. Please move a little closer.",
            Self::Clipped => "That was a bit loud. Please move back slightly.",
            Self::UnsupportedRate => "Your microphone format isn't supported yet.",
            Self::Inconsistent => "That sounded different from before. Let's try that one again.",
            Self::NotAssessable => "I couldn't make that out. Let's try again.",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Quality {
    pub voiced_seconds: f32,
    pub voiced_rms: f32,
    pub clipped_ratio: f32,
}

pub fn supported_rate(rate: u32) -> bool {
    (8_000..=96_000).contains(&rate)
}

/// Decode little-endian signed 16-bit mono PCM, as produced by notdot's
/// capture path. A trailing odd byte is ignored.
pub fn decode_s16le(pcm: &[u8]) -> Vec<f32> {
    pcm.chunks_exact(2)
        .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0)
        .collect()
}

/// Check speech quality on voiced frames and return the audio to embed: the
/// original signal from the first to the last voiced frame (silence inside is
/// kept, which measured better than splicing), bounded to the last
/// [`MAX_SECONDS`].
pub fn voiced(rate: u32, samples: &[f32]) -> Result<(Vec<f32>, Quality), SampleIssue> {
    if !supported_rate(rate) {
        return Err(SampleIssue::UnsupportedRate);
    }
    let frame = (rate * FRAME_MS / 1000) as usize;
    let frames: Vec<&[f32]> = samples.chunks(frame).collect();
    let rms: Vec<f32> = frames
        .iter()
        .map(|f| (f.iter().map(|s| s * s).sum::<f32>() / f.len().max(1) as f32).sqrt())
        .collect();
    let active: Vec<bool> = rms.iter().map(|&r| r >= VOICED_RMS).collect();
    let mut energy = 0.0f64;
    let mut clipped = 0usize;
    let mut voiced_frames = 0usize;
    for (i, f) in frames.iter().enumerate() {
        if active[i] {
            voiced_frames += 1;
            energy += f.iter().map(|s| f64::from(s * s)).sum::<f64>();
            clipped += f.iter().filter(|s| s.abs() >= CLIP_LEVEL).count();
        }
    }
    let voiced_samples = voiced_frames * frame;
    let quality = Quality {
        voiced_seconds: voiced_samples as f32 / rate as f32,
        voiced_rms: if voiced_samples == 0 {
            0.0
        } else {
            (energy / voiced_samples as f64).sqrt() as f32
        },
        clipped_ratio: clipped as f32 / voiced_samples.max(1) as f32,
    };
    let first = active
        .iter()
        .position(|&a| a)
        .unwrap_or(0)
        .saturating_sub(1)
        * frame;
    let last = (active.iter().rposition(|&a| a).unwrap_or(0) + 2) * frame;
    let mut kept = samples[first.min(samples.len())..last.min(samples.len())].to_vec();
    let max = (MAX_SECONDS * rate as f32) as usize;
    if kept.len() > max {
        kept.drain(..kept.len() - max);
    }
    if quality.voiced_seconds < MIN_VOICED_SECONDS {
        return Err(SampleIssue::TooShort);
    }
    if quality.voiced_rms < MIN_VOICED_RMS {
        return Err(SampleIssue::TooQuiet);
    }
    if quality.clipped_ratio > MAX_CLIPPED {
        return Err(SampleIssue::Clipped);
    }
    Ok((kept, quality))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tone(rate: u32, seconds: f32, amp: f32) -> Vec<f32> {
        (0..(rate as f32 * seconds) as usize)
            .map(|i| amp * (i as f32 * 220.0 * std::f32::consts::TAU / rate as f32).sin())
            .collect()
    }
    #[test]
    fn decodes_little_endian_pcm() {
        let pcm = [0x00, 0x80, 0xff, 0x7f, 0x00, 0x00, 0x01];
        assert_eq!(decode_s16le(&pcm), vec![-1.0, 32767.0 / 32768.0, 0.0]);
    }
    #[test]
    fn rejects_short_quiet_clipped_and_odd_rates() {
        assert_eq!(
            voiced(16_000, &tone(16_000, 0.5, 0.3)).unwrap_err(),
            SampleIssue::TooShort
        );
        assert_eq!(
            voiced(16_000, &tone(16_000, 3.0, 0.012)).unwrap_err(),
            SampleIssue::TooQuiet
        );
        let loud: Vec<f32> = tone(16_000, 3.0, 2.0)
            .iter()
            .map(|s| s.clamp(-1.0, 1.0))
            .collect();
        assert_eq!(voiced(16_000, &loud).unwrap_err(), SampleIssue::Clipped);
        assert_eq!(
            voiced(4_000, &tone(4_000, 3.0, 0.3)).unwrap_err(),
            SampleIssue::UnsupportedRate
        );
    }
    #[test]
    fn trims_silence_and_bounds_length() {
        let mut audio = vec![0.0; 16_000 * 2];
        audio.extend(tone(16_000, 2.0, 0.3));
        audio.extend(vec![0.0; 16_000 * 2]);
        let (kept, q) = voiced(16_000, &audio).unwrap();
        assert!((q.voiced_seconds - 2.0).abs() < 0.05);
        assert!(kept.len() < 16_000 * 21 / 10);
        let (long, _) = voiced(48_000, &tone(48_000, 20.0, 0.3)).unwrap();
        assert_eq!(long.len(), (MAX_SECONDS * 48_000.0) as usize);
    }
}
