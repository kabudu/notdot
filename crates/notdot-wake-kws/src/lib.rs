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

/// Cuts speech into utterances for the full-context (Whisper) backup
/// decoder. Shared by desktop and mobile so both cut audio the same way:
/// half a second of lead-in before speech, a 350 ms pause (a natural gap
/// between greeting and name stays inside one utterance), at most six
/// seconds, and nothing with under 200 ms of speech.
///
/// Utterances end at their pause even while the decoder is busy, so speech
/// that arrives during a slow decode is never joined to the next utterance
/// (the decode rules accept the wake phrase only at an utterance's start).
/// Finished utterances wait, oldest first, in a queue of at most
/// [`Self::MAX_WAITING`]; when it is full the oldest is dropped (and
/// counted), so a decoder that cannot keep up costs recall of old speech,
/// never unbounded memory or latency. Memory is bounded by the waiting
/// utterances, the one being collected (six seconds each) and the
/// half-second lead-in.
pub struct Utterances {
    gate: VoiceGate,
    tail: std::collections::VecDeque<f32>,
    samples: Vec<f32>,
    silence: usize,
    speech: usize,
    pause_ms: u32,
    rate: u32,
    waiting: std::collections::VecDeque<Vec<f32>>,
    dropped: u64,
}
impl Default for Utterances {
    fn default() -> Self {
        Self {
            gate: VoiceGate::default(),
            tail: std::collections::VecDeque::new(),
            samples: Vec::new(),
            silence: 0,
            speech: 0,
            pause_ms: 350,
            rate: 0,
            waiting: std::collections::VecDeque::new(),
            dropped: 0,
        }
    }
}
impl Utterances {
    /// Longest utterance kept, in seconds.
    pub const MAX_SECONDS: usize = 6;
    /// Finished utterances kept while the decoder is busy.
    pub const MAX_WAITING: usize = 2;

    /// Feed one packet. Returns the oldest finished utterance when `ready`
    /// (the decoder can take work). A finished segment with too little
    /// speech is dropped silently. A change of rate starts afresh.
    pub fn push(&mut self, rate: u32, samples: &[f32], ready: bool) -> Option<Vec<f32>> {
        if rate != self.rate {
            self.reset();
            self.rate = rate;
        }
        self.collect(rate as usize, samples);
        if ready {
            self.waiting.pop_front()
        } else {
            None
        }
    }

    fn collect(&mut self, rate: usize, samples: &[f32]) {
        let voiced = self.gate.observe(energy(samples));
        if self.samples.is_empty() {
            if voiced {
                self.samples.extend(self.tail.drain(..));
            } else {
                self.tail.extend(samples.iter().copied());
                while self.tail.len() > rate / 2 {
                    self.tail.pop_front();
                }
                return;
            }
        }
        let remaining = (rate * Self::MAX_SECONDS).saturating_sub(self.samples.len());
        self.samples
            .extend_from_slice(&samples[..samples.len().min(remaining)]);
        if voiced {
            self.silence = 0;
            self.speech += samples.len();
        } else {
            self.silence += samples.len();
        }
        if self.silence >= rate * self.pause_ms as usize / 1000
            || self.samples.len() >= rate * Self::MAX_SECONDS
        {
            let finished = std::mem::take(&mut self.samples);
            if self.speech >= rate / 5 {
                if self.waiting.len() == Self::MAX_WAITING {
                    self.waiting.pop_front();
                    self.dropped += 1;
                }
                self.waiting.push_back(finished);
            }
            self.speech = 0;
            self.silence = 0;
        }
    }

    /// Whether an utterance has started and is still being collected.
    pub fn is_collecting(&self) -> bool {
        !self.samples.is_empty()
    }

    /// Finished utterances waiting for the decoder.
    pub fn waiting(&self) -> usize {
        self.waiting.len()
    }

    /// Utterances dropped because the decoder could not keep up.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Change the end-of-utterance pause (diagnostics only; default 350 ms).
    pub fn set_pause_ms(&mut self, ms: u32) {
        self.pause_ms = ms;
    }

    /// Drop collected and waiting audio but keep the learned noise floor
    /// (after a decoder error).
    pub fn clear(&mut self) {
        self.samples.clear();
        self.tail.clear();
        self.waiting.clear();
        self.speech = 0;
        self.silence = 0;
    }

    /// Start afresh, including the noise floor (new stream or reset).
    pub fn reset(&mut self) {
        self.clear();
        self.gate = VoiceGate::default();
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
    fn utterances_need_speech_a_pause_and_a_free_decoder() {
        let rate = 16_000;
        let packet = 320;
        let quiet = vec![0.0001f32; packet];
        let loud = vec![0.05f32; packet];
        let mut cut = Utterances::default();
        for _ in 0..50 {
            assert!(cut.push(rate, &quiet, true).is_none());
        }
        // 400 ms of speech, then a pause while the decoder is busy: it ends
        // at the pause (18 packets reach 350 ms) and waits.
        for _ in 0..20 {
            assert!(cut.push(rate, &loud, true).is_none());
        }
        for _ in 0..30 {
            assert!(cut.push(rate, &quiet, false).is_none());
        }
        assert_eq!(cut.waiting(), 1);
        assert!(!cut.is_collecting());
        // Released once the decoder is free, with half a second of lead-in.
        let utterance = cut.push(rate, &quiet, true).expect("utterance");
        assert_eq!(utterance.len(), 8_000 + 20 * packet + 18 * packet);
        // A click under 200 ms of speech is dropped, not decoded.
        cut.push(rate, &loud, true);
        for _ in 0..20 {
            assert!(cut.push(rate, &quiet, true).is_none());
        }
        // Continuous speech is cut at the cap.
        let mut capped = None;
        for _ in 0..400 {
            if let Some(u) = cut.push(rate, &loud, true) {
                capped = Some(u);
                break;
            }
        }
        assert_eq!(
            capped.unwrap().len(),
            rate as usize * Utterances::MAX_SECONDS
        );
        cut.reset();
        assert!(cut.push(rate, &quiet, true).is_none());
    }
    #[test]
    fn a_busy_decoder_never_joins_utterances_and_its_queue_is_bounded() {
        let rate = 16_000;
        let packet = 320;
        let quiet = vec![0.0001f32; packet];
        let mut cut = Utterances::default();
        for _ in 0..50 {
            cut.push(rate, &quiet, true);
        }
        // Five utterances of distinct lengths (300..700 ms of speech), each
        // followed by a 400 ms pause, all while the decoder is busy.
        for n in 0..5 {
            let loud = vec![0.05f32 + 0.01 * n as f32; packet];
            for _ in 0..15 + 5 * n {
                assert!(cut.push(rate, &loud, false).is_none());
            }
            for _ in 0..20 {
                assert!(cut.push(rate, &quiet, false).is_none());
            }
            assert!(cut.waiting() <= Utterances::MAX_WAITING);
        }
        assert_eq!(cut.dropped(), 3);
        // The two newest come out in order, each on its own: the last
        // speech packet of each is its own level, never another's.
        for n in [3usize, 4] {
            let u = cut.push(rate, &quiet, true).expect("waiting utterance");
            let speech: Vec<_> = u.iter().filter(|s| **s > 0.01).collect();
            assert_eq!(speech.len(), (15 + 5 * n) * packet);
            assert!(
                speech
                    .iter()
                    .all(|s| (**s - (0.05 + 0.01 * n as f32)).abs() < 1e-6)
            );
        }
        assert!(cut.push(rate, &quiet, true).is_none());
        // A rate change or reset forgets waiting audio.
        let loud = vec![0.05f32; packet];
        for _ in 0..20 {
            cut.push(rate, &loud, false);
        }
        for _ in 0..20 {
            cut.push(rate, &quiet, false);
        }
        assert_eq!(cut.waiting(), 1);
        assert!(cut.push(48_000, &quiet, true).is_none());
        assert_eq!(cut.waiting(), 0);
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
