//! On-device wake keyword spotting, shared by notdot desktop and mobile.
//!
//! sherpa-onnx keyword spotting with the bundled phonetic (or BPE) model,
//! plus notdot's acoustic guards: speech must support the keyword, an
//! explicit "Hey" prefix must be tight, and a coarticulated "Hey Ava" must
//! hold its shared vowel. Audio stays in memory; there is no network access.
//! The desktop adds its Whisper fallback around [`Spotter`].
use sherpa_onnx::{KeywordSpotter, KeywordSpotterConfig, OnlineStream};
use std::path::Path;

/// Validate a companion name and build the wake phrase ("Hey {name}" or
/// the name alone). Restricting characters prevents keyword-directive
/// injection into the spotter's keyword list.
pub fn phrase(name: &str, name_only: bool) -> Result<String, String> {
    if name
        .chars()
        .any(|c| !c.is_ascii_alphabetic() && !c.is_whitespace() && c != '-' && c != '\'')
    {
        return Err("Wake phrases currently support English names using letters, spaces, apostrophes and hyphens. Turn wake listening off to use another name.".into());
    }
    let name = name
        .replace('-', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if name.is_empty() || name.len() > 80 {
        return Err("Choose a name for the wake phrase".into());
    }
    Ok(if name_only {
        name
    } else {
        format!("Hey {name}")
    })
}

/// Keyword spotter state for one wake phrase.
pub struct Spotter {
    kws: KeywordSpotter,
    stream: OnlineStream,
    require_hey: bool,
    /// Token count of the coarticulated variant, when the phrase has one.
    merged: Option<usize>,
    pub candidates: u64,
    pub rejected_energy: u64,
    pub rejected_prefix: u64,
    elapsed: f64,
    quiet_seconds: f64,
    voice_gate: VoiceGate,
    voiced: std::collections::VecDeque<(f64, f64, bool)>,
}
impl Spotter {
    pub fn new(models: &Path, phrase: &str) -> Result<Self, String> {
        let lexicon = std::fs::read_to_string(models.join("phonetic/en.phone"))
            .map_err(|_| "Bundled wake pronunciation dictionary is unavailable")?;
        let pronunciations: std::collections::HashMap<&str, Vec<&str>> = lexicon
            .lines()
            .filter_map(|l| {
                let mut parts = l.split_whitespace();
                Some((parts.next()?, parts.collect()))
            })
            .collect();
        let upper = phrase.to_uppercase();
        let phones: Option<Vec<_>> = upper
            .split_whitespace()
            .map(|word| pronunciations.get(word))
            .collect();
        let require_hey = phones.is_some() && upper.starts_with("HEY ");
        let (models, tokens) = if let Some(phones) = phones {
            (
                models.join("phonetic"),
                phones
                    .into_iter()
                    .flatten()
                    .map(|s| (*s).to_owned())
                    .collect::<Vec<_>>(),
            )
        } else {
            let tokenizer =
                sentencepiece_rs::SentencePieceProcessor::open(models.join("bpe.model"))
                    .map_err(|_| "Bundled wake tokenizer is unavailable")?;
            (
                models.to_path_buf(),
                tokenizer
                    .encode(&upper)
                    .map_err(|_| "Could not encode wake phrase")?,
            )
        };

        let vocabulary = std::fs::read_to_string(models.join("tokens.txt"))
            .map_err(|_| "Bundled wake vocabulary is unavailable")?;
        if tokens.is_empty()
            || tokens.iter().any(|t| {
                t == "<unk>"
                    || !vocabulary
                        .lines()
                        .any(|l| l.split_whitespace().next() == Some(t.as_str()))
            })
        {
            return Err("This name cannot be encoded by the English wake model".into());
        }
        let path = |name| models.join(name).to_string_lossy().into_owned();
        let mut c = KeywordSpotterConfig::default();
        c.model_config.transducer.encoder = Some(path("encoder.onnx"));
        c.model_config.transducer.decoder = Some(path("decoder.onnx"));
        c.model_config.transducer.joiner = Some(path("joiner.onnx"));
        c.model_config.tokens = Some(path("tokens.txt"));
        c.model_config.provider = Some("cpu".into());
        c.model_config.num_threads = 1;
        let mut keywords = format!("{} :3.0 #0.20 @wake\n", tokens.join(" "));
        // Hey Ava shares the EY vowel across the word boundary. Model both
        // separated and coarticulated vowels without dropping other phonemes.
        let mut merged = None;
        if require_hey && tokens.get(1) == tokens.get(2) {
            let mut joined = tokens.clone();
            joined.remove(2);
            merged = Some(joined.len());
            keywords.push_str(&format!("{} :3.0 #0.20 @wake\n", joined.join(" ")));
        }
        c.keywords_buf = Some(keywords);
        let kws = KeywordSpotter::create(&c).ok_or("Could not load the bundled wake model")?;
        let stream = kws.create_stream();
        Ok(Self {
            kws,
            stream,
            require_hey,
            merged,
            candidates: 0,
            rejected_energy: 0,
            rejected_prefix: 0,
            elapsed: 0.,
            quiet_seconds: 0.,
            voice_gate: VoiceGate::default(),
            voiced: Default::default(),
        })
    }
    /// Feed mono samples at `rate`; `true` when the wake phrase is heard.
    pub fn accept(&mut self, rate: u32, samples: &[f32]) -> bool {
        let voiced = self.voice_gate.observe(energy(samples));
        let seconds = samples.len() as f64 / f64::from(rate);
        self.quiet_seconds = if voiced {
            0.
        } else {
            self.quiet_seconds + seconds
        };
        // Sherpa 2.0 resets decoder offsets after 1.5 s of trailing blanks,
        // without rebasing its feature stream. Recreate the stream in quiet
        // audio first, so token times and acoustic-support times stay aligned.
        // Keep both the Hey-prefix and speech-support guards unchanged.
        if self.elapsed >= 120. || self.quiet_seconds >= 1. {
            self.reset_keyword();
        }
        let start = self.elapsed;
        self.elapsed += seconds;
        self.voiced.push_back((start, self.elapsed, voiced));
        while self
            .voiced
            .front()
            .is_some_and(|(_, end, _)| *end < self.elapsed - 8.)
        {
            self.voiced.pop_front();
        }
        self.stream.accept_waveform(rate as i32, samples);
        while self.kws.is_ready(&self.stream) {
            self.kws.decode(&self.stream);
            if let Some(result) = self
                .kws
                .get_result(&self.stream)
                .filter(|r| !r.keyword.is_empty())
            {
                // H is unvoiced and often aligned before its acoustic onset.
                // Check speech across the explicit Hey prefix, not only at H.
                let first = result
                    .timestamps
                    .first()
                    .map(|t| f64::from(result.start_time + t));
                let prefix_end = result
                    .timestamps
                    .get(usize::from(self.require_hey))
                    .map(|t| f64::from(result.start_time + t));
                let supported = first.zip(prefix_end).is_some_and(|(first, end)| {
                    self.voiced.iter().any(|(start, stop, voice)| {
                        *voice && *start <= end + 0.10 && *stop >= first - 0.04
                    })
                });

                self.kws.reset(&self.stream);
                // An inserted H can stretch across the name's opening vowel.
                // Require the short H-to-EY transition for the explicit Hey prefix.
                let prefix = (!self.require_hey
                    || result
                        .timestamps
                        .get(1)
                        .zip(result.timestamps.first())
                        .is_some_and(|(ey, h)| *ey > *h && ey - h <= 0.16))
                    && merged_vowel_is_held(&result.timestamps, self.merged);
                #[cfg(test)]
                eprintln!(
                    "wake candidate: supported={supported} prefix={prefix} timestamps={:?}",
                    result.timestamps
                );
                self.candidates = self.candidates.saturating_add(1);
                if !supported {
                    self.rejected_energy = self.rejected_energy.saturating_add(1);
                }
                if !prefix {
                    self.rejected_prefix = self.rejected_prefix.saturating_add(1);
                }
                if supported && prefix {
                    return true;
                }
            }
        }
        false
    }

    pub fn reset(&mut self) {
        self.reset_keyword();
    }
    /// Seconds of audio in the current keyword stream (resets in quiet).
    pub fn elapsed(&self) -> f64 {
        self.elapsed
    }
    fn reset_keyword(&mut self) {
        self.stream = self.kws.create_stream();
        self.elapsed = 0.;
        self.quiet_seconds = 0.;
        self.voiced.clear();
    }
}

/// The coarticulated "Hey Ava" variant drops the second EY, which leaves it one
/// vowel away from everyday speech such as "have a" or "heavy". A spoken
/// "Hey Ava" holds that shared vowel; "have a" moves on almost at once. In 176
/// synthetic clips the merged vowel lasted 0.24-0.28 s in true wakes and
/// 0.04-0.08 s in every false accept, so require 0.16 s, midway between them.
/// Timestamps arrive in 0.04 s steps; the tolerance keeps 0.16 itself held.
pub const MIN_MERGED_VOWEL_SECONDS: f32 = 0.16 - 0.005;
pub fn merged_vowel_is_held(timestamps: &[f32], merged: Option<usize>) -> bool {
    if merged != Some(timestamps.len()) {
        return true;
    }
    timestamps
        .get(2)
        .zip(timestamps.get(1))
        .is_some_and(|(next, vowel)| next - vowel >= MIN_MERGED_VOWEL_SECONDS)
}

/// Sample-time endpointing keeps working independently of webview animation frames.
/// Estimate a quiet floor from a bounded two-second window of 20 ms packets.
/// Keep the old upper threshold in loud environments, but support quiet inputs.
pub struct VoiceGate {
    levels: [f32; 100],
    count: usize,
    cursor: usize,
}
impl Default for VoiceGate {
    fn default() -> Self {
        Self {
            levels: [0.; 100],
            count: 0,
            cursor: 0,
        }
    }
}
impl VoiceGate {
    pub fn observe(&mut self, rms: f32) -> bool {
        let rms = if rms.is_finite() { rms.max(0.) } else { 0. };
        self.levels[self.cursor] = rms;
        self.cursor = (self.cursor + 1) % 100;
        self.count = (self.count + 1).min(100);
        let floor = self.levels[..self.count]
            .iter()
            .copied()
            .fold(f32::INFINITY, f32::min);
        let threshold = (floor * 4.).clamp(0.00015, 0.015);
        rms >= threshold
    }
}

pub fn energy(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        0.
    } else {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn merged_hey_ava_vowel_must_be_held_unlike_have_a() {
        // Timestamps of HH EY V AH from the merged keyword variant (seconds).
        let have_a = [0.16, 0.20, 0.24, 0.32];
        let heavy = [0.16, 0.24, 0.32, 0.36];
        let hey_ava = [0.20, 0.24, 0.52, 0.60];
        let boundary = [0.20, 0.24, 0.40, 0.48];
        assert!(merged_vowel_is_held(&boundary, Some(4)));
        assert!(!merged_vowel_is_held(&[0.20, 0.24, 0.36, 0.44], Some(4)));
        assert!(!merged_vowel_is_held(&have_a, Some(4)));
        assert!(!merged_vowel_is_held(&heavy, Some(4)));
        assert!(merged_vowel_is_held(&hey_ava, Some(4)));
        // The full keyword and phrases without a merged variant are unaffected.
        assert!(merged_vowel_is_held(
            &[0.0, 0.04, 0.08, 0.12, 0.16],
            Some(4)
        ));
        assert!(merged_vowel_is_held(&have_a, None));
    }
    #[test]
    fn phrase_validation_prevents_keyword_directive_injection() {
        assert_eq!(phrase(" Ava ", false).unwrap(), "Hey Ava");
        assert_eq!(phrase("Mary-Jane", true).unwrap(), "Mary Jane");
        for name in ["", "Ava #0.01", "Ava @other", "Ava\0", "艾娃"] {
            assert!(phrase(name, false).is_err());
        }
    }
    #[test]
    fn voice_gate_tracks_a_quiet_floor_and_ignores_bad_levels() {
        let mut gate = VoiceGate::default();
        for _ in 0..50 {
            assert!(!gate.observe(0.0001));
        }
        assert!(gate.observe(0.01));
        assert!(!gate.observe(f32::NAN));
        assert_eq!(energy(&[]), 0.);
        assert!((energy(&[0.5, -0.5]) - 0.5).abs() < 1e-6);
    }
}
