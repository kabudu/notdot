//! EXPERIMENTAL phonetic wake detector: the same template matching as
//! [`crate::personal`], but over frame embeddings from the bundled phonetic
//! keyword-spotting model's encoder instead of MFCCs. Enrolled in the
//! personal wake lab; [`PhoneticListener`] runs it continuously for
//! hands-free wake when a phonetic enrolment is saved. Not speaker
//! authentication, and not shown to meet the wake gate (synthetic evidence:
//! .local/mobile-design/PERSONAL-WAKE.md).
//!
//! Pipeline (as evaluated in .local/mobile-design/PERSONAL-WAKE.md):
//! - Kaldi 80-band fbank ([`crate::fbank`], golden parity with
//!   kaldi-native-fbank), then the pinned streaming zipformer2 encoder
//!   (29-frame chunks advancing 16, caches carried) gives 320-d frames;
//! - templates: the take trimmed to its speech (+0.3 s silence each side),
//!   then encoder frames outside the non-blank span (joiner blank
//!   probability < 0.99 with an empty-context decoder; 2 frames kept each
//!   side) dropped; tries are used whole;
//! - matching: [`crate::personal::sdtw`] on unit-length frames; the bound is
//!   1.2 x the largest leave-one-out distance among the phrase takes; a try
//!   matches if within the bound and closer to the phrase than to every
//!   similar-phrase take.
//!
//! Models are bound to their phrase and to this detector's own format
//! ("NDPE" version 1); an MFCC model is never read as one, or vice versa.
//! New recordings are needed: MFCC templates cannot be converted.
use crate::MobileError;
use crate::personal::{
    ContrastTake, MAX_CONTRASTS, MAX_POSITIVES, MIN_POSITIVES, PersonalMatch, PersonalModelBuild,
    PersonalModelSummary, PersonalTake, invalid, phrase_key,
};

pub const EMBED: usize = 320;
const MAGIC: &[u8; 4] = b"NDPE";
/// Bump when features, model files or matching change.
pub const VERSION: u16 = 1;
/// A maximal 4 s take gives about 80 encoder frames (40 ms each).
const MAX_TEMPLATE_FRAMES: usize = 100;
const MIN_TEMPLATE_FRAMES: usize = 3;
const MAX_LABEL: usize = 40;
const MAX_PHRASE: usize = 120;

fn damaged() -> MobileError {
    invalid("Phonetic wake data is damaged")
}

// ---- Model format (independent of the runtime, always compiled) ----

pub(crate) struct Model {
    pub phrase: String,
    pub bound: f64,
    pub positives: Vec<Vec<[f64; EMBED]>>,
    pub contrasts: Vec<(String, Vec<[f64; EMBED]>)>,
}

fn put_frames(out: &mut Vec<u8>, f: &[[f64; EMBED]]) {
    out.extend_from_slice(&(f.len() as u16).to_le_bytes());
    for frame in f {
        for v in frame {
            out.extend_from_slice(&(*v as f32).to_le_bytes());
        }
    }
}

#[cfg_attr(
    not(all(feature = "personal-phonetic", not(windows))),
    allow(dead_code)
)]
pub(crate) fn encode_take(f: &[[f64; EMBED]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + f.len() * EMBED * 4);
    out.extend_from_slice(b"NDPT");
    out.extend_from_slice(&VERSION.to_le_bytes());
    put_frames(&mut out, f);
    out
}

pub(crate) fn encode_model(m: &Model) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.push(m.phrase.len() as u8);
    out.extend_from_slice(m.phrase.as_bytes());
    out.extend_from_slice(&(m.bound as f32).to_le_bytes());
    out.push(m.positives.len() as u8);
    out.push(m.contrasts.len() as u8);
    for p in &m.positives {
        put_frames(&mut out, p);
    }
    for (label, c) in &m.contrasts {
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
        put_frames(&mut out, c);
    }
    out
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], MobileError> {
        if self.0.len() < n {
            return Err(damaged());
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8, MobileError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, MobileError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn f32(&mut self) -> Result<f32, MobileError> {
        let b = self.take(4)?;
        let v = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        if v.is_finite() { Ok(v) } else { Err(damaged()) }
    }
    fn text(&mut self, max: usize) -> Result<String, MobileError> {
        let n = self.u8()? as usize;
        if n > max {
            return Err(damaged());
        }
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| damaged())
    }
    fn frames(&mut self) -> Result<Vec<[f64; EMBED]>, MobileError> {
        let n = self.u16()? as usize;
        if !(MIN_TEMPLATE_FRAMES..=MAX_TEMPLATE_FRAMES).contains(&n) || self.0.len() < n * EMBED * 4
        {
            return Err(damaged());
        }
        (0..n)
            .map(|_| {
                let mut frame = [0.; EMBED];
                for v in &mut frame {
                    *v = f64::from(self.f32()?);
                }
                let norm2: f64 = frame.iter().map(|v| v * v).sum();
                if frame.iter().any(|v| v.abs() > 1. + 1e-3) || norm2 > 1. + 1e-3 {
                    return Err(damaged());
                }
                Ok(frame)
            })
            .collect()
    }
    fn end(&self) -> Result<(), MobileError> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(damaged())
        }
    }
}

pub(crate) fn decode_take(bytes: &[u8]) -> Result<Vec<[f64; EMBED]>, MobileError> {
    let mut r = Reader(bytes);
    if r.take(4)? != b"NDPT" || r.u16()? != VERSION {
        return Err(invalid(
            "This take was made by another detector or version; record it again",
        ));
    }
    let f = r.frames()?;
    r.end()?;
    Ok(f)
}

pub(crate) fn decode_model(bytes: &[u8], phrase: &str) -> Result<Model, MobileError> {
    let m = decode_stored(bytes)?;
    if m.phrase != phrase_key(phrase)? {
        return Err(invalid(
            "These recordings are for a different wake phrase; record them again",
        ));
    }
    Ok(m)
}

/// A model with the phrase it was enrolled for (no phrase check).
pub(crate) fn decode_stored(bytes: &[u8]) -> Result<Model, MobileError> {
    let mut r = Reader(bytes);
    if r.take(4)? != MAGIC {
        return Err(invalid("These are not phonetic wake recordings"));
    }
    if r.u16()? != VERSION {
        return Err(invalid(
            "These phonetic recordings were made by another version; record them again",
        ));
    }
    let stored = r.text(MAX_PHRASE)?;
    if stored.is_empty() || phrase_key(&stored).ok().as_deref() != Some(stored.as_str()) {
        return Err(damaged());
    }
    let bound = f64::from(r.f32()?);
    let (np, nc) = (r.u8()? as usize, r.u8()? as usize);
    if !(MIN_POSITIVES..=MAX_POSITIVES).contains(&np)
        || nc > MAX_CONTRASTS
        || !(0. ..=2.).contains(&bound)
    {
        return Err(damaged());
    }
    let positives = (0..np).map(|_| r.frames()).collect::<Result<_, _>>()?;
    let contrasts = (0..nc)
        .map(|_| Ok((r.text(MAX_LABEL)?, r.frames()?)))
        .collect::<Result<_, MobileError>>()?;
    r.end()?;
    Ok(Model {
        phrase: stored,
        bound,
        positives,
        contrasts,
    })
}

/// Build a model from phrase takes and contrast takes (features as made by
/// [`phonetic_take`]); the rule matches the MFCC lab's.
pub(crate) fn build(
    phrase: &str,
    positives: &[Vec<u8>],
    contrasts: &[ContrastTake],
) -> Result<PersonalModelBuild, MobileError> {
    let phrase = phrase_key(phrase)?;
    if !(MIN_POSITIVES..=MAX_POSITIVES).contains(&positives.len()) {
        return Err(invalid("Record three to eight takes of your wake phrase"));
    }
    if contrasts.len() > MAX_CONTRASTS {
        return Err(invalid("Record at most six similar-phrase takes"));
    }
    let positives = positives
        .iter()
        .map(|p| decode_take(p))
        .collect::<Result<Vec<_>, _>>()?;
    let contrasts = contrasts
        .iter()
        .map(|c| {
            let label = if c.label.len() > 4 * MAX_LABEL {
                ""
            } else {
                c.label.trim()
            };
            if label.is_empty() || label.len() > MAX_LABEL {
                return Err(invalid("Name each similar phrase (at most 40 characters)"));
            }
            Ok((label.to_owned(), decode_take(&c.features)?))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let loo: Vec<f64> = (0..positives.len())
        .map(|k| {
            positives
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != k)
                .map(|(_, t)| crate::personal::sdtw(t, &positives[k]))
                .fold(f64::INFINITY, f64::min)
        })
        .collect();
    let mut sorted = loo.clone();
    sorted.sort_by(f64::total_cmp);
    let median = sorted[sorted.len() / 2];
    let inconsistent = (0..loo.len())
        .filter(|&k| loo[k] > 2. * median.max(0.01))
        .map(|k| k as u32)
        .collect();
    let bound = (1.2 * sorted[sorted.len() - 1]).clamp(0., 2.);
    Ok(PersonalModelBuild {
        model: encode_model(&Model {
            phrase,
            bound,
            positives,
            contrasts,
        }),
        inconsistent,
    })
}

/// Compare a try's frames with a model.
#[cfg_attr(
    not(all(feature = "personal-phonetic", not(windows))),
    allow(dead_code)
)]
pub(crate) fn compare(m: &Model, q: &[[f64; EMBED]]) -> PersonalMatch {
    let nearest = m
        .positives
        .iter()
        .map(|t| crate::personal::sdtw(t, q))
        .fold(f64::INFINITY, f64::min);
    let contrast = m
        .contrasts
        .iter()
        .map(|(label, t)| (crate::personal::sdtw(t, q), label))
        .min_by(|a, b| a.0.total_cmp(&b.0));
    let finite = nearest.is_finite();
    let within = finite && nearest <= m.bound;
    let closer = contrast.is_none_or(|(c, _)| nearest < c);
    PersonalMatch {
        issue: None,
        nearest_phrase: finite.then_some(nearest as f32),
        nearest_contrast: contrast.and_then(|(c, _)| c.is_finite().then_some(c as f32)),
        contrast_label: contrast.map(|(_, l)| l.clone()),
        phrase_bound: m.bound as f32,
        within_bound: within,
        closer_than_contrasts: closer,
        matched: within && closer,
    }
}

/// Unit-length frames (zero frames stay zero).
#[cfg_attr(
    not(all(feature = "personal-phonetic", not(windows))),
    allow(dead_code)
)]
pub(crate) fn unit(frames: Vec<[f32; EMBED]>) -> Vec<[f64; EMBED]> {
    frames
        .into_iter()
        .map(|f| {
            let n = f.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>().sqrt();
            let mut out = [0.; EMBED];
            if n > 1e-8 && n.is_finite() {
                for (o, v) in out.iter_mut().zip(f) {
                    *o = f64::from(v) / n;
                }
            }
            out
        })
        .collect()
}

/// Template edges: frames outside the non-blank span (blank probability
/// below 0.99), keeping two each side; all frames if none is non-blank.
#[cfg_attr(
    not(all(feature = "personal-phonetic", not(windows))),
    allow(dead_code)
)]
pub(crate) fn trim_span(blank: &[f32]) -> (usize, usize) {
    let first = blank.iter().position(|&p| p < 0.99);
    let last = blank.iter().rposition(|&p| p < 0.99);
    match (first, last) {
        (Some(a), Some(b)) => (a.saturating_sub(2), (b + 3).min(blank.len())),
        _ => (0, blank.len()),
    }
}

// ---- Runtime: the bundled model through ONNX Runtime ----

#[cfg(all(feature = "personal-phonetic", not(windows)))]
pub(crate) mod runtime {
    use super::EMBED;
    use crate::fbank;
    use crate::ort::{Input, Kind, Session, TensorData, TensorInfo};
    use std::path::Path;

    /// Pinned files (src-tauri/resources/wake/phonetic/SHA256SUMS); the
    /// chunking constants below hold for exactly these.
    const PINS: [(&str, &str); 3] = [
        (
            "encoder.onnx",
            "2ca84d6bfe73e1ea3c9c49f600f7cad1c9ddd423c53c906b8bfe802444dd78d5",
        ),
        (
            "decoder.onnx",
            "63a22dd60f40fff082ac3e09afa507f6787da36df76ded2fbe145fa233e22c21",
        ),
        (
            "joiner.onnx",
            "190d4067b4cc20b72a42a1916e69d92052000fb7051a427ebb1bc72a69207dc1",
        ),
    ];
    const CHUNK: usize = 29;
    const SHIFT: usize = 16;
    const VOCAB: usize = 263;
    const MAX_FILE: u64 = 64 << 20;
    /// Per cache tensor and all caches together (elements).
    const MAX_STATE: usize = 1 << 16;
    const MAX_STATES_TOTAL: usize = 1 << 20;
    const MAX_STATES: usize = 48;
    /// Longest input (samples): the core's own 6 s limit.
    const MAX_SAMPLES: usize = 16_000 * 6 + 16_000;

    struct State {
        name: String,
        output: String,
        kind: Kind,
        dims: Vec<i64>,
        len: usize,
    }

    pub(crate) struct Kws {
        encoder: Session,
        joiner: Session,
        decoder_out: Vec<f32>,
        states: Vec<State>,
    }

    /// The encoder's carried caches for one audio stream.
    #[derive(Default)]
    pub(crate) struct Caches {
        f32s: Vec<Vec<f32>>,
        i64s: Vec<Vec<i64>>,
    }

    /// Fbank frames per chunk and the step between chunks.
    pub(crate) const CHUNK_FRAMES: usize = CHUNK;
    pub(crate) const CHUNK_SHIFT: usize = SHIFT;

    fn verify(dir: &Path) -> Result<(), String> {
        use sha2::{Digest, Sha256};
        for (name, pin) in PINS {
            let path = dir.join(name);
            let meta = std::fs::metadata(&path)
                .map_err(|_| "The phonetic wake model is unavailable".to_owned())?;
            if meta.len() > MAX_FILE {
                return Err("The phonetic wake model is not the expected one".into());
            }
            let digest = Sha256::digest(
                std::fs::read(&path)
                    .map_err(|_| "The phonetic wake model is unavailable".to_owned())?,
            );
            let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
            if hex != pin {
                return Err("The phonetic wake model is not the expected one".into());
            }
        }
        Ok(())
    }

    /// Check the encoder's declared schema and plan its cache tensors,
    /// before anything is allocated for them.
    /// One encoder cache: its input and output names, type, shape and size.
    pub(crate) struct CachePlan {
        pub name: String,
        pub output: String,
        pub kind: Kind,
        pub dims: Vec<i64>,
        pub len: usize,
    }

    pub(crate) fn plan(
        inputs: &[TensorInfo],
        outputs: &[TensorInfo],
    ) -> Result<Vec<CachePlan>, String> {
        let bad = |what: &str| format!("Unexpected phonetic model schema: {what}");
        let x = inputs
            .iter()
            .find(|i| i.name == "x")
            .ok_or_else(|| bad("no x"))?;
        if x.kind != Some(Kind::F32)
            || x.dims.len() != 3
            || x.dims[1] != CHUNK as i64
            || x.dims[2] != fbank::BINS as i64
        {
            return Err(bad("x"));
        }
        let out = outputs
            .iter()
            .find(|o| o.name == "encoder_out")
            .ok_or_else(|| bad("no encoder_out"))?;
        if out.kind != Some(Kind::F32) || out.dims.len() != 3 || out.dims[2] != EMBED as i64 {
            return Err(bad("encoder_out"));
        }
        let states: Vec<&TensorInfo> = inputs.iter().filter(|i| i.name != "x").collect();
        if states.is_empty() || states.len() > MAX_STATES || inputs.len() != states.len() + 1 {
            return Err(bad("cache count"));
        }
        let mut total = 0usize;
        states
            .iter()
            .map(|s| {
                let kind = match s.kind {
                    Some(k @ (Kind::F32 | Kind::I64)) => k,
                    _ => return Err(bad("cache type")),
                };
                // Exactly one dynamic (batch) dimension, set to 1.
                if s.dims.is_empty()
                    || s.dims.len() > 4
                    || s.dims.iter().filter(|d| **d < 0).count() != 1
                {
                    return Err(bad("cache rank"));
                }
                let dims: Vec<i64> = s.dims.iter().map(|d| if *d < 0 { 1 } else { *d }).collect();
                let len = dims
                    .iter()
                    .try_fold(1usize, |a, d| {
                        usize::try_from(*d)
                            .ok()
                            .filter(|d| *d > 0 || dims.len() == 1)
                            .and_then(|d| a.checked_mul(d))
                    })
                    .filter(|n| *n <= MAX_STATE)
                    .ok_or_else(|| bad("cache size"))?;
                total = total
                    .checked_add(len)
                    .filter(|t| *t <= MAX_STATES_TOTAL)
                    .ok_or_else(|| bad("cache total"))?;
                let output = format!("new_{}", s.name);
                let o = outputs
                    .iter()
                    .find(|o| o.name == output)
                    .ok_or_else(|| bad("cache output"))?;
                if o.kind != Some(kind) || o.dims.len() != s.dims.len() {
                    return Err(bad("cache output shape"));
                }
                Ok(CachePlan {
                    name: s.name.clone(),
                    output,
                    kind,
                    dims,
                    len,
                })
            })
            .collect()
    }

    impl Kws {
        /// Open the pinned model in `dir` (the wake models' `phonetic`
        /// folder) with one thread.
        pub(crate) fn open(dir: &Path) -> Result<Self, String> {
            verify(dir)?;
            let mut encoder = Session::open(&dir.join("encoder.onnx"), 1)?;
            let mut decoder = Session::open(&dir.join("decoder.onnx"), 1)?;
            let joiner = Session::open(&dir.join("joiner.onnx"), 1)?;
            let inputs = encoder.describe(false)?;
            let outputs = encoder.describe(true)?;
            let states = plan(&inputs, &outputs)?
                .into_iter()
                .map(|p| State {
                    name: p.name,
                    output: p.output,
                    kind: p.kind,
                    dims: p.dims,
                    len: p.len,
                })
                .collect();
            // Empty context [0, 0]: used only to find each template's edges.
            let y = [0i64, 0];
            let d = decoder.run(&[("y", Input::I64(&y, &[1, 2]))], "decoder_out", EMBED)?;
            if d.shape != [1, EMBED as i64] || d.data.len() != EMBED {
                return Err("Unexpected phonetic decoder output".into());
            }
            Ok(Self {
                encoder,
                joiner,
                decoder_out: d.data,
                states,
            })
        }

        /// Fresh (all-zero) encoder caches.
        pub(crate) fn caches(&self) -> Caches {
            let mut c = Caches::default();
            for s in &self.states {
                match s.kind {
                    Kind::F32 => c.f32s.push(vec![0.; s.len]),
                    _ => c.i64s.push(vec![0; s.len]),
                }
            }
            c
        }

        /// Run one 29-frame chunk, carrying `caches`; returns its encoder frames.
        pub(crate) fn chunk(
            &mut self,
            chunk: &[[f32; fbank::BINS]],
            caches: &mut Caches,
        ) -> Result<Vec<[f32; EMBED]>, String> {
            if chunk.len() != CHUNK {
                return Err("Unexpected phonetic chunk".into());
            }
            let outputs: Vec<&str> = std::iter::once("encoder_out")
                .chain(self.states.iter().map(|s| s.output.as_str()))
                .collect();
            let state_total: usize = self.states.iter().map(|s| s.len).sum();
            let max_out = (CHUNK * EMBED).max(self.states.iter().map(|s| s.len).max().unwrap_or(0));
            let flat: Vec<f32> = chunk.iter().flatten().copied().collect();
            let x_shape = [1, CHUNK as i64, fbank::BINS as i64];
            let mut inputs: Vec<(&str, Input<'_>)> = vec![("x", Input::F32(&flat, &x_shape))];
            let (mut fi, mut ii) = (0, 0);
            for s in &self.states {
                match s.kind {
                    Kind::F32 => {
                        inputs.push((&s.name, Input::F32(&caches.f32s[fi], &s.dims)));
                        fi += 1;
                    }
                    _ => {
                        inputs.push((&s.name, Input::I64(&caches.i64s[ii], &s.dims)));
                        ii += 1;
                    }
                }
            }
            let out =
                self.encoder
                    .run_many(&inputs, &outputs, max_out, CHUNK * EMBED + state_total)?;
            let e = &out[0];
            let frames_out = match (&e.data, e.shape.as_slice()) {
                (TensorData::F32(d), [1, n, w])
                    if *w == EMBED as i64 && *n >= 0 && d.len() == *n as usize * EMBED =>
                {
                    d
                }
                _ => return Err("Unexpected phonetic encoder output".into()),
            };
            let mut enc = Vec::with_capacity(frames_out.len() / EMBED);
            for f in frames_out.chunks_exact(EMBED) {
                let mut frame = [0f32; EMBED];
                frame.copy_from_slice(f);
                if frame.iter().any(|v| !v.is_finite()) {
                    return Err("Unexpected phonetic encoder output".into());
                }
                enc.push(frame);
            }
            // Carry the caches, checking each keeps its declared shape.
            let (mut fi, mut ii) = (0, 0);
            for (s, o) in self.states.iter().zip(&out[1..]) {
                if o.shape != s.dims {
                    return Err("Unexpected phonetic encoder cache".into());
                }
                match (&o.data, s.kind) {
                    (TensorData::F32(d), Kind::F32) if d.len() == s.len => {
                        caches.f32s[fi].copy_from_slice(d);
                        fi += 1;
                    }
                    (TensorData::I64(d), Kind::I64) if d.len() == s.len => {
                        caches.i64s[ii].copy_from_slice(d);
                        ii += 1;
                    }
                    _ => return Err("Unexpected phonetic encoder cache".into()),
                }
            }
            Ok(enc)
        }

        /// Encoder frames (and their blank probability) for 16 kHz audio.
        /// `cancelled` is checked between chunks.
        pub(crate) fn frames(
            &mut self,
            x: &[f32],
            cancelled: &dyn Fn() -> bool,
        ) -> Result<(Vec<[f32; EMBED]>, Vec<f32>), String> {
            if x.len() > MAX_SAMPLES {
                return Err("Recording is too long".into());
            }
            let mut feats = fbank::compute(x);
            if feats.is_empty() {
                return Ok((Vec::new(), Vec::new()));
            }
            let floor = feats
                .iter()
                .flatten()
                .copied()
                .fold(f32::INFINITY, f32::min);
            feats.extend(std::iter::repeat_n([floor; fbank::BINS], CHUNK));
            let mut caches = self.caches();
            let mut enc = Vec::new();
            let mut start = 0;
            while start + CHUNK <= feats.len() {
                if cancelled() {
                    return Err("Cancelled".into());
                }
                enc.extend(self.chunk(&feats[start..start + CHUNK], &mut caches)?);
                start += SHIFT;
            }
            let blank = self.blank(&enc)?;
            Ok((enc, blank))
        }

        #[cfg(test)]
        pub(crate) fn encoder_mut(&mut self) -> &mut Session {
            &mut self.encoder
        }

        /// Blank probability per encoder frame (joiner, empty context).
        fn blank(&mut self, enc: &[[f32; EMBED]]) -> Result<Vec<f32>, String> {
            let mut out = Vec::with_capacity(enc.len());
            for batch in enc.chunks(64) {
                let n = batch.len();
                let e: Vec<f32> = batch.iter().flatten().copied().collect();
                let d: Vec<f32> = std::iter::repeat_n(self.decoder_out.iter().copied(), n)
                    .flatten()
                    .collect();
                let shape = [n as i64, EMBED as i64];
                let logits = self.joiner.run(
                    &[
                        ("encoder_out", Input::F32(&e, &shape)),
                        ("decoder_out", Input::F32(&d, &shape)),
                    ],
                    "logit",
                    n * VOCAB,
                )?;
                if logits.shape != [n as i64, VOCAB as i64] || logits.data.len() != n * VOCAB {
                    return Err("Unexpected phonetic joiner output".into());
                }
                for row in logits.data.chunks_exact(VOCAB) {
                    let max = row.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                    let sum: f32 = row.iter().map(|v| (v - max).exp()).sum();
                    let p = (row[0] - max).exp() / sum;
                    if !p.is_finite() {
                        return Err("Unexpected phonetic joiner output".into());
                    }
                    out.push(p);
                }
            }
            Ok(out)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::path::PathBuf;

        fn dir() -> PathBuf {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../src-tauri/resources/wake/phonetic")
        }

        fn info(name: &str, kind: Option<Kind>, dims: &[i64]) -> TensorInfo {
            TensorInfo {
                name: name.into(),
                kind,
                dims: dims.to_vec(),
            }
        }

        /// Native encoder parity with the evaluated Python pipeline
        /// (onnxruntime 1.32 there, 1.28.2 here) on a synthetic signal.
        #[test]
        fn encoder_matches_the_python_reference() {
            let mut kws = Kws::open(&dir()).unwrap();
            let (enc, blank) = kws
                .frames(&crate::fbank::tests::golden_signal(0, 8_000), &|| false)
                .unwrap();
            let text = include_str!("../tests/fixtures/encoder-golden.txt");
            let mut lines = text.lines().filter(|l| !l.starts_with('#'));
            let n: usize = lines
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .parse()
                .unwrap();
            assert_eq!(enc.len(), n);
            // Bounds from the model's measured sensitivity (int8 dynamic
            // quantisation): fbank noise of +-1e-4 alone moves frames to a
            // cosine of 0.9991 and blank probability by 0.004 in the Python
            // runtime. Matching uses unit-length frames, so direction counts.
            let (mut min_cos, mut max_blank) = (1f64, 0f32);
            for (i, line) in lines.enumerate() {
                let want: Vec<f64> = line
                    .split_whitespace()
                    .map(|v| v.parse().unwrap())
                    .collect();
                assert_eq!(want.len(), 1 + EMBED);
                let ours = super::super::unit(vec![enc[i]]).remove(0);
                let cos: f64 = ours.iter().zip(&want[1..]).map(|(a, b)| a * b).sum();
                min_cos = min_cos.min(cos);
                max_blank = max_blank.max((blank[i] - want[0] as f32).abs());
            }
            assert!(min_cos >= 0.998, "min frame cosine {min_cos}");
            assert!(max_blank <= 0.008, "max blank difference {max_blank}");
            eprintln!(
                "encoder parity: min frame cosine {min_cos:.5}, max blank difference {max_blank:.4}"
            );
        }

        #[test]
        fn the_schema_is_checked_before_allocating_caches() {
            let mut kws = Kws::open(&dir()).unwrap();
            let inputs = kws.encoder.describe(false).unwrap();
            let outputs = kws.encoder.describe(true).unwrap();
            let planned = plan(&inputs, &outputs).unwrap();
            assert_eq!(planned.len(), inputs.len() - 1);
            let with = |name: &str, replace: TensorInfo| -> Vec<TensorInfo> {
                inputs
                    .iter()
                    .map(|i| {
                        if i.name == name {
                            replace.clone()
                        } else {
                            i.clone()
                        }
                    })
                    .collect()
            };
            let cache = inputs.iter().find(|i| i.name != "x").unwrap().clone();
            // Wrong input shape, cache type, cache rank, batch dimensions, size.
            assert!(
                plan(
                    &with("x", info("x", Some(Kind::F32), &[-1, 30, 80])),
                    &outputs
                )
                .is_err()
            );
            assert!(
                plan(
                    &with(
                        &cache.name,
                        info(&cache.name, Some(Kind::Bool), &cache.dims)
                    ),
                    &outputs
                )
                .is_err()
            );
            assert!(
                plan(
                    &with(&cache.name, info(&cache.name, None, &cache.dims)),
                    &outputs
                )
                .is_err()
            );
            assert!(
                plan(
                    &with(
                        &cache.name,
                        info(&cache.name, cache.kind, &[1, 2, 3, 4, -1])
                    ),
                    &outputs
                )
                .is_err()
            );
            assert!(
                plan(
                    &with(&cache.name, info(&cache.name, cache.kind, &[-1, -1, 8])),
                    &outputs
                )
                .is_err()
            );
            assert!(
                plan(
                    &with(&cache.name, info(&cache.name, cache.kind, &[-1, 1 << 20])),
                    &outputs
                )
                .is_err()
            );
            // A cache without its output, or with a different output type.
            let missing: Vec<TensorInfo> = outputs
                .iter()
                .filter(|o| o.name != format!("new_{}", cache.name))
                .cloned()
                .collect();
            assert!(plan(&inputs, &missing).is_err());
            let retyped: Vec<TensorInfo> = outputs
                .iter()
                .map(|o| {
                    if o.name == format!("new_{}", cache.name) {
                        info(&o.name, Some(Kind::Bool), &o.dims)
                    } else {
                        o.clone()
                    }
                })
                .collect();
            assert!(plan(&inputs, &retyped).is_err());
            let no_encoder: Vec<TensorInfo> = outputs
                .iter()
                .filter(|o| o.name != "encoder_out")
                .cloned()
                .collect();
            assert!(plan(&inputs, &no_encoder).is_err());
        }

        #[test]
        fn cancellation_bounds_and_tampering_are_handled() {
            let mut kws = Kws::open(&dir()).unwrap();
            let x = crate::fbank::tests::golden_signal(0, 16_000);
            assert_eq!(kws.frames(&x, &|| true).unwrap_err(), "Cancelled");
            // The same session still works after a cancelled run.
            assert!(!kws.frames(&x, &|| false).unwrap().0.is_empty());
            assert!(kws.frames(&vec![0.; MAX_SAMPLES + 1], &|| false).is_err());
            assert_eq!(kws.frames(&[], &|| false).unwrap().0.len(), 0);
            // Aggregate output bound: encoder_out fits, the caches do not;
            // the partly copied outputs are freed and an error returned.
            let inputs = kws.encoder.describe(false).unwrap();
            let names: Vec<String> = inputs
                .iter()
                .filter(|i| i.name != "x")
                .map(|i| format!("new_{}", i.name))
                .collect();
            let chunk = vec![0f32; CHUNK * fbank::BINS];
            let states: Vec<(Vec<f32>, Vec<i64>, &TensorInfo)> = inputs
                .iter()
                .filter(|i| i.name != "x")
                .map(|i| {
                    let dims: Vec<i64> =
                        i.dims.iter().map(|d| if *d < 0 { 1 } else { *d }).collect();
                    let n: usize = dims.iter().map(|d| *d as usize).product();
                    (vec![0.; n], dims, i)
                })
                .collect();
            let x_shape = [1, CHUNK as i64, fbank::BINS as i64];
            let mut feed: Vec<(&str, Input<'_>)> = vec![("x", Input::F32(&chunk, &x_shape))];
            let zeros_i64: Vec<Vec<i64>> =
                states.iter().map(|(f, _, _)| vec![0i64; f.len()]).collect();
            for (k, (f, dims, i)) in states.iter().enumerate() {
                feed.push((
                    &i.name,
                    if i.kind == Some(Kind::I64) {
                        Input::I64(&zeros_i64[k], dims)
                    } else {
                        Input::F32(f, dims)
                    },
                ));
            }
            let outs: Vec<&str> = std::iter::once("encoder_out")
                .chain(names.iter().map(|s| s.as_str()))
                .collect();
            let encoder = kws.encoder_mut();
            assert!(
                encoder
                    .run_many(&feed, &outs, 1 << 16, CHUNK * EMBED)
                    .is_err()
            );
            assert!(encoder.run_many(&feed, &outs, 8, 1 << 20).is_err());
            assert!(
                encoder
                    .run_many(&feed, &["encoder_out", "no_such_output"], 1 << 16, 1 << 20)
                    .is_err()
            );
            assert!(encoder.run_many(&feed, &outs, 1 << 16, 1 << 22).is_ok());
            // A changed or missing model file is refused before loading.
            let tmp = std::env::temp_dir().join(format!("notdot-phonetic-{}", std::process::id()));
            std::fs::create_dir_all(&tmp).unwrap();
            for f in ["encoder.onnx", "decoder.onnx", "joiner.onnx"] {
                std::fs::copy(dir().join(f), tmp.join(f)).unwrap();
            }
            assert!(Kws::open(&tmp).is_ok());
            let mut bytes = std::fs::read(tmp.join("joiner.onnx")).unwrap();
            let last = bytes.len() - 1;
            bytes[last] ^= 1;
            std::fs::write(tmp.join("joiner.onnx"), bytes).unwrap();
            assert!(Kws::open(&tmp).is_err());
            std::fs::remove_file(tmp.join("decoder.onnx")).unwrap();
            assert!(Kws::open(&tmp).is_err());
            std::fs::remove_dir_all(&tmp).unwrap();
        }

        /// Distance parity with the evaluated Python pipeline on the local
        /// synthetic corpus (not in the repository). Set
        /// NOTDOT_PERSONAL_CORPUS and compare with
        /// .local/eval/phonetic_parity.py.
        #[test]
        #[ignore = "set NOTDOT_PERSONAL_CORPUS to the local synthetic corpus"]
        fn synthetic_distance_parity_with_python() {
            let corpus = PathBuf::from(std::env::var("NOTDOT_PERSONAL_CORPUS").unwrap());
            let models = dir().parent().unwrap().to_string_lossy().into_owned();
            let read = |name: &str| -> Vec<i16> {
                let mut r = hound::WavReader::open(corpus.join(name)).unwrap();
                r.samples::<i16>().map(Result::unwrap).collect()
            };
            let mut kws = Kws::open(&dir()).unwrap();
            for voice in ["danielenglishuk", "karen", "samanthaenglishus"] {
                let templates: Vec<Vec<[f64; EMBED]>> = (1..=5)
                    .map(|i| {
                        let pcm: Vec<u8> = read(&format!("{voice}-enrol-{i}.wav"))
                            .iter()
                            .flat_map(|s| s.to_le_bytes())
                            .collect();
                        let take =
                            super::super::phonetic_take(models.clone(), pcm, 16_000).unwrap();
                        super::super::decode_take(&take.features).unwrap()
                    })
                    .collect();
                for name in ["pos-1", "pos-4", "neg-1-160", "neg-2-160", "neg-3-205"] {
                    let x: Vec<f32> = read(&format!("{voice}-{name}.wav"))
                        .iter()
                        .map(|s| f32::from(*s) / 32768.)
                        .collect();
                    let peak = x.iter().fold(1e-9f32, |m, v| m.max(v.abs()));
                    let mut y = vec![0f32; 8_000];
                    y.extend(x.iter().map(|v| v / peak * 0.3));
                    y.extend(std::iter::repeat_n(0., 8_000));
                    let q = super::super::unit(kws.frames(&y, &|| false).unwrap().0);
                    let s = templates
                        .iter()
                        .map(|t| crate::personal::sdtw(t, &q))
                        .fold(f64::INFINITY, f64::min);
                    println!("parity {voice} {name} {s:.6}");
                }
            }
        }

        /// Decision parity with the evaluated Python pipeline on the local
        /// synthetic corpus: template spans, enrolment bound and every
        /// held-out decision (clean, three levels). Compare with
        /// .local/eval/decision_parity.py via decision_compare.py.
        #[test]
        #[ignore = "set NOTDOT_PERSONAL_CORPUS to the local synthetic corpus"]
        fn synthetic_decision_records() {
            let corpus = PathBuf::from(std::env::var("NOTDOT_PERSONAL_CORPUS").unwrap());
            let read = |name: &str| -> Vec<f64> {
                let mut r = hound::WavReader::open(corpus.join(name)).unwrap();
                r.samples::<i16>()
                    .map(|s| f64::from(s.unwrap()) / 32768.)
                    .collect()
            };
            let mut voices: Vec<String> = std::fs::read_dir(&corpus)
                .unwrap()
                .filter_map(|e| e.ok()?.file_name().into_string().ok())
                .filter_map(|n| n.strip_suffix("-enrol-1.wav").map(str::to_owned))
                .collect();
            voices.sort();
            let mut kws = Kws::open(&dir()).unwrap();
            for v in &voices {
                let mut takes = Vec::new();
                for i in 1..=5 {
                    let t = super::super::template(&mut kws, &read(&format!("{v}-enrol-{i}.wav")))
                        .unwrap()
                        .unwrap();
                    println!("span {v} {i} {} {} {}", t.start, t.end, t.total);
                    takes.push(super::super::encode_take(&t.frames));
                }
                let mut contrasts = Vec::new();
                for k in ["ada", "eva", "nova"] {
                    for r in [175, 220] {
                        let t = super::super::template(
                            &mut kws,
                            &read(&format!("{v}-contrast-{k}-{r}.wav")),
                        )
                        .unwrap()
                        .unwrap();
                        contrasts.push(super::super::ContrastTake {
                            label: format!("{k}-{r}"),
                            features: super::super::encode_take(&t.frames),
                        });
                    }
                }
                let model = super::super::decode_model(
                    &super::super::build("Hey Ava", &takes, &contrasts)
                        .unwrap()
                        .model,
                    "Hey Ava",
                )
                .unwrap();
                println!("bound {v} {:.6}", model.bound);
                let mut names: Vec<String> = std::fs::read_dir(&corpus)
                    .unwrap()
                    .filter_map(|e| e.ok()?.file_name().into_string().ok())
                    .filter(|n| {
                        n.starts_with(&format!("{v}-pos-")) || n.starts_with(&format!("{v}-neg-"))
                    })
                    .collect();
                names.sort();
                for n in &names {
                    let x = read(n);
                    let peak = x.iter().fold(1e-9f64, |m, s| m.max(s.abs()));
                    for gain in [0.05, 0.3, 0.9] {
                        let mut y = vec![0f32; 8_000];
                        y.extend(x.iter().map(|s| (s / peak * gain) as f32));
                        y.extend(std::iter::repeat_n(0f32, 8_000));
                        let q = super::super::unit(kws.frames(&y, &|| false).unwrap().0);
                        let m = super::super::compare(&model, &q);
                        println!(
                            "q {v} {} {gain} {:.6} {:.6} {} {} {}",
                            n.trim_end_matches(".wav"),
                            m.nearest_phrase.unwrap_or(f32::INFINITY),
                            m.nearest_contrast.unwrap_or(f32::INFINITY),
                            u8::from(m.within_bound),
                            u8::from(m.closer_than_contrasts),
                            u8::from(m.matched)
                        );
                    }
                }
            }
        }

        /// End to end through the exported API: a synthetic phrase enrolled
        /// five times matches a new take of itself and not a different one.
        /// Streaming (features as they arrive, caches carried) gives exactly
        /// the whole-clip encoder frames, for every chunk that has arrived.
        #[test]
        fn streamed_frames_match_whole_clip_frames() {
            let mut kws = Kws::open(&dir()).unwrap();
            let x = crate::fbank::tests::golden_signal(0, 48_000);
            let (whole, _) = kws.frames(&x, &|| false).unwrap();
            let mut caches = kws.caches();
            let mut fb = crate::fbank::Stream::default();
            let (mut feats, mut streamed) = (Vec::new(), Vec::new());
            for piece in x.chunks(320) {
                feats.extend(fb.push(piece));
                while feats.len() >= CHUNK {
                    streamed.extend(kws.chunk(&feats[..CHUNK], &mut caches).unwrap());
                    feats.drain(..SHIFT);
                }
            }
            assert!(
                streamed.len() + 12 >= whole.len() && streamed.len() <= whole.len(),
                "{} {}",
                streamed.len(),
                whole.len()
            );
            let worst = streamed
                .iter()
                .zip(&whole)
                .flat_map(|(a, b)| a.iter().zip(b).map(|(x, y)| (x - y).abs()))
                .fold(0f32, f32::max);
            assert!(worst < 1e-4, "max difference {worst}");
        }

        #[test]
        fn exported_api_round_trip() {
            let models = dir().parent().unwrap().to_string_lossy().into_owned();
            let tone = |pitches: &[f64], gain: f64| -> Vec<u8> {
                let mut x = vec![0f64; 4_800];
                for &p in pitches {
                    for n in 0..3_200 {
                        let env = (std::f64::consts::PI * n as f64 / 3_200.).sin();
                        x.push(
                            gain * env * (2. * std::f64::consts::PI * p * n as f64 / 16_000.).sin(),
                        );
                    }
                }
                x.extend(std::iter::repeat_n(0., 4_800));
                let mut s = 7u32;
                x.iter()
                    .flat_map(|v| {
                        s ^= s << 13;
                        s ^= s >> 17;
                        s ^= s << 5;
                        let n = (s as f64 / u32::MAX as f64 - 0.5) * 2e-4;
                        (((v + n).clamp(-1., 1.) * 32_767.) as i16).to_le_bytes()
                    })
                    .collect()
            };
            let take = |p: &[f64], g| {
                super::super::phonetic_take(models.clone(), tone(p, g), 16_000).unwrap()
            };
            let takes: Vec<Vec<u8>> = [0.25, 0.3, 0.35, 0.4, 0.5]
                .iter()
                .map(|g| take(&[300., 500., 400.], *g))
                .inspect(|t| assert!(t.issue.is_none(), "{t:?}"))
                .map(|t| t.features)
                .collect();
            let model = super::super::phonetic_model("Hey Ava".into(), takes, Vec::new())
                .unwrap()
                .model;
            let same = super::super::phonetic_match(
                models.clone(),
                model.clone(),
                "Hey Ava".into(),
                tone(&[300., 500., 400.], 0.45),
                16_000,
            )
            .unwrap();
            let other = super::super::phonetic_match(
                models.clone(),
                model.clone(),
                "Hey Ava".into(),
                tone(&[900., 150.], 0.3),
                16_000,
            )
            .unwrap();
            assert!(
                same.nearest_phrase.unwrap() < other.nearest_phrase.unwrap(),
                "{same:?} {other:?}"
            );
            let silent = super::super::phonetic_match(
                models.clone(),
                model.clone(),
                "Hey Ava".into(),
                vec![0; 32_000],
                16_000,
            )
            .unwrap();
            assert!(silent.issue.is_some() && !silent.matched);
            // Continuous listening: the phrase comes from the enrolment, and
            // long silence in 20 ms packets never wakes it.
            let listener =
                super::super::PhoneticListener::new(models.clone(), model.clone()).unwrap();
            assert_eq!(listener.phrase(), "hey ava");
            for _ in 0..250 {
                assert!(!listener.push_pcm16(vec![0; 640]).unwrap());
            }
            assert!(listener.push_pcm16(vec![0; 3]).is_err());
            assert!(listener.push_pcm16(vec![0; 64_002]).is_err());
            listener.reset();
            assert!(!listener.push_pcm16(vec![0; 64_000]).unwrap());
            assert!(
                super::super::phonetic_match(
                    models,
                    model,
                    "Hey Nova".into(),
                    tone(&[300.], 0.3),
                    16_000
                )
                .is_err()
            );
        }
    }
}

// ---- Exported API (always present; Unavailable unless built in) ----

fn unavailable() -> MobileError {
    MobileError::Unavailable {
        detail: "This build does not include the phonetic wake experiment".into(),
    }
}

/// The phonetic model folder inside the app's wake models.
#[cfg(all(feature = "personal-phonetic", not(windows)))]
fn model_dir(models: &str) -> std::path::PathBuf {
    std::path::Path::new(models).join("phonetic")
}

/// A template and where it came from (for parity checks).
#[cfg(all(feature = "personal-phonetic", not(windows)))]
#[cfg_attr(not(test), allow(dead_code))] // spans are read by the parity test
pub(crate) struct Template {
    /// Encoder frames kept: [start, end) of `total`.
    pub start: usize,
    pub end: usize,
    pub total: usize,
    pub frames: Vec<[f64; EMBED]>,
}

/// A take's template: trimmed to its speech (+0.3 s silence each side),
/// encoded, then cut to the non-blank span. Err(issue) if unusable.
#[cfg(all(feature = "personal-phonetic", not(windows)))]
pub(crate) fn template(
    kws: &mut runtime::Kws,
    x: &[f64],
) -> Result<Result<Template, crate::personal::TakeIssue>, String> {
    use crate::personal::TakeIssue;
    let Some((a, b)) = crate::personal::speech_span(x) else {
        return Ok(Err(TakeIssue::NoSpeech));
    };
    let mut y = vec![0f32; 4_800];
    y.extend(
        x[a * 160..(b * 160 + 400).min(x.len())]
            .iter()
            .map(|v| *v as f32),
    );
    y.extend(std::iter::repeat_n(0f32, 4_800));
    let (enc, blank) = kws.frames(&y, &|| false)?;
    let (start, end) = trim_span(&blank);
    let frames = unit(enc[start..end].to_vec());
    if frames.len() < MIN_TEMPLATE_FRAMES {
        return Ok(Err(TakeIssue::TooShort));
    }
    if frames.len() > MAX_TEMPLATE_FRAMES {
        return Ok(Err(TakeIssue::TooLong));
    }
    Ok(Ok(Template {
        start,
        end,
        total: enc.len(),
        frames,
    }))
}

/// Check one enrolment take and derive its phonetic template (the audio is
/// not kept). `models` is the wake models folder.
#[uniffi::export]
pub fn phonetic_take(
    models: String,
    pcm16: Vec<u8>,
    rate: u32,
) -> Result<PersonalTake, MobileError> {
    let x = crate::personal::samples(&pcm16, rate, crate::personal::MAX_TAKE_SAMPLES)?;
    drop(pcm16);
    let (issue, speech_ms) = crate::personal::check(&x, 2_500);
    if issue.is_some() {
        return Ok(PersonalTake {
            issue,
            speech_ms,
            features: Vec::new(),
        });
    }
    #[cfg(all(feature = "personal-phonetic", not(windows)))]
    {
        let mut kws = runtime::Kws::open(&model_dir(&models))
            .map_err(|detail| MobileError::Wake { detail })?;
        match template(&mut kws, &x).map_err(|detail| MobileError::Wake { detail })? {
            Ok(t) => Ok(PersonalTake {
                issue: None,
                speech_ms,
                features: encode_take(&t.frames),
            }),
            Err(issue) => Ok(PersonalTake {
                issue: Some(issue),
                speech_ms,
                features: Vec::new(),
            }),
        }
    }
    #[cfg(not(all(feature = "personal-phonetic", not(windows))))]
    {
        let _ = models;
        Err(unavailable())
    }
}

/// Build a phonetic model for `phrase` (3-8 phrase takes, at most 6 contrast takes).
#[uniffi::export]
pub fn phonetic_model(
    phrase: String,
    positives: Vec<Vec<u8>>,
    contrasts: Vec<ContrastTake>,
) -> Result<PersonalModelBuild, MobileError> {
    if !cfg!(all(feature = "personal-phonetic", not(windows))) {
        return Err(unavailable());
    }
    build(&phrase, &positives, &contrasts)
}

/// Compare a try (16 kHz 16-bit mono, at most 6 s) with a phonetic model.
#[uniffi::export]
pub fn phonetic_match(
    models: String,
    model: Vec<u8>,
    phrase: String,
    pcm16: Vec<u8>,
    rate: u32,
) -> Result<PersonalMatch, MobileError> {
    let m = decode_model(&model, &phrase)?;
    let x = crate::personal::samples(&pcm16, rate, crate::personal::MAX_TEST_SAMPLES)?;
    drop(pcm16);
    let (issue, _) = crate::personal::check(&x, 6_000);
    if let Some(issue) = issue {
        return Ok(PersonalMatch {
            issue: Some(issue),
            nearest_phrase: None,
            nearest_contrast: None,
            contrast_label: None,
            phrase_bound: m.bound as f32,
            within_bound: false,
            closer_than_contrasts: false,
            matched: false,
        });
    }
    #[cfg(all(feature = "personal-phonetic", not(windows)))]
    {
        let y: Vec<f32> = x.iter().map(|v| *v as f32).collect();
        let mut kws = runtime::Kws::open(&model_dir(&models))
            .map_err(|detail| MobileError::Wake { detail })?;
        let (enc, _) = kws
            .frames(&y, &|| false)
            .map_err(|detail| MobileError::Wake { detail })?;
        Ok(compare(&m, &unit(enc)))
    }
    #[cfg(not(all(feature = "personal-phonetic", not(windows))))]
    {
        let _ = (models, m, x);
        Err(unavailable())
    }
}

#[uniffi::export]
pub fn phonetic_model_summary(
    model: Vec<u8>,
    phrase: String,
) -> Result<PersonalModelSummary, MobileError> {
    let m = decode_model(&model, &phrase)?;
    Ok(PersonalModelSummary {
        phrase_takes: m.positives.len() as u32,
        contrast_labels: m.contrasts.into_iter().map(|(label, _)| label).collect(),
    })
}

/// Whether this build includes the phonetic experiment.
#[uniffi::export]
pub fn phonetic_available() -> bool {
    cfg!(all(feature = "personal-phonetic", not(windows)))
}

// ---- Continuous listening ----

/// Window kept for matching: twice the longest phrase template (the DTW
/// allows half speed) plus a few frames, so a whole phrase always fits.
#[cfg_attr(
    not(all(feature = "personal-phonetic", not(windows))),
    allow(dead_code)
)]
fn window_frames(m: &Model) -> usize {
    2 * m.positives.iter().map(Vec::len).max().unwrap_or(0) + 8
}

/// Matching over a rolling window of unit-length encoder frames: after each
/// new chunk, the window is compared with the model exactly as a lab try is;
/// a match fires once and clears the window, so one phrase wakes once.
struct Window {
    model: Model,
    size: usize,
    shortest: usize,
    frames: std::collections::VecDeque<[f64; EMBED]>,
}

impl Window {
    #[cfg_attr(
        not(all(feature = "personal-phonetic", not(windows))),
        allow(dead_code)
    )]
    fn new(model: Model) -> Self {
        let size = window_frames(&model);
        let shortest = model
            .positives
            .iter()
            .map(Vec::len)
            .min()
            .unwrap_or(0)
            .max(1);
        Self {
            model,
            size,
            shortest,
            frames: std::collections::VecDeque::with_capacity(size + 8),
        }
    }

    /// Add one chunk's frames; true if the phrase matched (the window is then cleared).
    #[cfg_attr(
        not(all(feature = "personal-phonetic", not(windows))),
        allow(dead_code)
    )]
    fn push(&mut self, frames: Vec<[f64; EMBED]>) -> bool {
        self.frames.extend(frames);
        while self.frames.len() > self.size {
            self.frames.pop_front();
        }
        if self.frames.len() < self.shortest {
            return false;
        }
        let matched = compare(&self.model, self.frames.make_contiguous()).matched;
        if matched {
            self.frames.clear();
        }
        matched
    }
}

#[cfg(all(feature = "personal-phonetic", not(windows)))]
struct Encoder {
    kws: runtime::Kws,
    caches: runtime::Caches,
    fbank: crate::fbank::Stream,
    feats: Vec<[f32; crate::fbank::BINS]>,
}

struct Listen {
    window: Window,
    #[cfg(all(feature = "personal-phonetic", not(windows)))]
    encoder: Encoder,
}

/// Continuous wake listening with a saved phonetic model: feed microphone
/// audio as it arrives; [`PhoneticListener::push_pcm16`] says when the
/// enrolled phrase was heard. The phrase is the one the model was enrolled
/// for (any name). Audio is processed and dropped, never kept: only the
/// encoder's caches, under one chunk of features and the last few seconds
/// of encoder frames are held. Not speaker identification.
#[derive(uniffi::Object)]
pub struct PhoneticListener {
    phrase: String,
    inner: std::sync::Mutex<Listen>,
}

/// At most two seconds of 16 kHz audio per push.
const MAX_PUSH_BYTES: usize = 2 * 2 * 16_000;

#[uniffi::export]
impl PhoneticListener {
    /// `models` is the wake models folder; `model` a saved phonetic model.
    #[uniffi::constructor]
    pub fn new(models: String, model: Vec<u8>) -> Result<std::sync::Arc<Self>, MobileError> {
        let m = decode_stored(&model)?;
        drop(model);
        #[cfg(all(feature = "personal-phonetic", not(windows)))]
        {
            let kws = runtime::Kws::open(&model_dir(&models))
                .map_err(|detail| MobileError::Wake { detail })?;
            let caches = kws.caches();
            Ok(std::sync::Arc::new(Self {
                phrase: m.phrase.clone(),
                inner: std::sync::Mutex::new(Listen {
                    window: Window::new(m),
                    encoder: Encoder {
                        kws,
                        caches,
                        fbank: Default::default(),
                        feats: Vec::new(),
                    },
                }),
            }))
        }
        #[cfg(not(all(feature = "personal-phonetic", not(windows))))]
        {
            let _ = (models, m);
            Err(unavailable())
        }
    }

    /// The enrolled wake phrase (normalised, as saved).
    pub fn phrase(&self) -> String {
        self.phrase.clone()
    }

    /// Feed 16 kHz mono PCM16 (little-endian), at most two seconds at a
    /// time. True if the phrase was heard in this audio.
    pub fn push_pcm16(&self, pcm16: Vec<u8>) -> Result<bool, MobileError> {
        if !pcm16.len().is_multiple_of(2) || pcm16.len() > MAX_PUSH_BYTES {
            return Err(invalid(
                "push whole PCM16 samples, at most two seconds at a time",
            ));
        }
        let x: Vec<f32> = pcm16
            .chunks_exact(2)
            .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.)
            .collect();
        drop(pcm16);
        let mut l = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        #[cfg(all(feature = "personal-phonetic", not(windows)))]
        {
            let Listen { window, encoder } = &mut *l;
            let e = encoder;
            let new = e.fbank.push(&x);
            e.feats.extend(new);
            let mut heard = false;
            while e.feats.len() >= runtime::CHUNK_FRAMES {
                let out = match e
                    .kws
                    .chunk(&e.feats[..runtime::CHUNK_FRAMES], &mut e.caches)
                {
                    Ok(out) => out,
                    Err(detail) => {
                        // Start afresh rather than continue from a broken state.
                        e.caches = e.kws.caches();
                        e.fbank = Default::default();
                        e.feats.clear();
                        window.frames.clear();
                        return Err(MobileError::Wake { detail });
                    }
                };
                e.feats.drain(..runtime::CHUNK_SHIFT);
                heard |= window.push(unit(out));
            }
            Ok(heard)
        }
        #[cfg(not(all(feature = "personal-phonetic", not(windows))))]
        {
            let _ = (x, &mut l);
            Err(unavailable())
        }
    }

    /// Forget everything heard so far and start a new stream (after a gap,
    /// such as the phone's own reply playing).
    pub fn reset(&self) {
        let mut l = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        l.window.frames.clear();
        #[cfg(all(feature = "personal-phonetic", not(windows)))]
        {
            l.encoder.caches = l.encoder.kws.caches();
            l.encoder.fbank = Default::default();
            l.encoder.feats.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(seed: usize, n: usize) -> Vec<[f64; EMBED]> {
        (0..n)
            .map(|i| {
                let mut f = [0f32; EMBED];
                f[(seed * 7 + i) % EMBED] = 1.;
                f[(seed * 13 + i * 3) % EMBED] += 0.5;
                f
            })
            .map(|f| unit(vec![f]).remove(0))
            .collect()
    }

    #[test]
    fn models_round_trip_bind_the_phrase_and_refuse_other_detectors() {
        let takes: Vec<Vec<u8>> = (0..5).map(|_| encode_take(&frames(1, 12))).collect();
        let contrasts = vec![ContrastTake {
            label: "Similar".into(),
            features: encode_take(&frames(9, 12)),
        }];
        let built = build("Hey  Ava", &takes, &contrasts).unwrap();
        let m = decode_model(&built.model, "hey ava").unwrap();
        assert_eq!((m.positives.len(), m.contrasts.len()), (5, 1));
        assert!((0. ..=2.).contains(&m.bound));
        assert!(decode_model(&built.model, "Hey Nova").is_err());
        // An MFCC model and an MFCC take are never read as phonetic ones.
        let mut mfcc = built.model.clone();
        mfcc[..4].copy_from_slice(b"NDPW");
        assert!(decode_model(&mfcc, "Hey Ava").is_err());
        assert!(decode_take(b"NDPF\x01\x00\x05\x00").is_err());
        let same = compare(&m, &frames(1, 30));
        assert!(
            same.matched && same.nearest_phrase.is_some_and(|d| d < 1e-6),
            "{same:?}"
        );
        let near = compare(&m, &frames(9, 30));
        assert!(!near.closer_than_contrasts && !near.matched, "{near:?}");
        let other = compare(&m, &frames(40, 30));
        assert!(!other.matched, "{other:?}");
        assert!(compare(&m, &[]).nearest_phrase.is_none());
    }

    #[test]
    fn a_window_fires_once_per_phrase_and_keeps_bounded_frames() {
        let takes: Vec<Vec<u8>> = (0..5).map(|_| encode_take(&frames(1, 12))).collect();
        let contrasts = vec![ContrastTake {
            label: "Similar".into(),
            features: encode_take(&frames(9, 12)),
        }];
        let built = build("Hey Harriet", &takes, &contrasts).unwrap();
        // The listener reads the phrase from the enrolment itself.
        let m = decode_stored(&built.model).unwrap();
        assert_eq!(m.phrase, "hey harriet");
        let mut w = Window::new(m);
        assert_eq!(w.size, 2 * 12 + 8);
        // Unrelated sound for a long time: no wake, bounded memory.
        for _ in 0..50 {
            assert!(!w.push(frames(40, 4)));
            assert!(w.frames.len() <= w.size);
        }
        // The sound-alike, said in full: no wake.
        let near = frames(9, 12);
        assert!(!w.push(near[..6].to_vec()) && !w.push(near[6..].to_vec()));
        // The rule is the lab's: while that sound-alike is still in the window
        // it ties with the phrase, so the phrase must come after it has passed.
        for _ in 0..8 {
            assert!(!w.push(frames(40, 4)));
        }
        // The phrase arrives in chunks: one wake when its last frames arrive.
        let phrase = frames(1, 12);
        let fired: Vec<bool> = phrase.chunks(4).map(|c| w.push(c.to_vec())).collect();
        assert_eq!(fired, [false, false, true]);
        // The same phrase cannot fire twice; silence after it never fires.
        assert!(w.frames.is_empty());
        for _ in 0..10 {
            assert!(!w.push(frames(40, 4)));
        }
        // Said again later: wakes again.
        assert!(phrase.chunks(4).any(|c| w.push(c.to_vec())));
    }

    #[test]
    fn the_listener_refuses_bad_models_and_bad_audio() {
        assert!(PhoneticListener::new(".".into(), vec![1, 2, 3]).is_err());
        let takes: Vec<Vec<u8>> = (0..3).map(|i| encode_take(&frames(i, 8))).collect();
        let model = build("Hey Ava", &takes, &[]).unwrap().model;
        let mut wrong = model.clone();
        wrong[..4].copy_from_slice(b"NDPW");
        assert!(PhoneticListener::new(".".into(), wrong).is_err());
        // A stored phrase that is not in normal form is damage, not a phrase.
        let mut odd = model.clone();
        odd[7] = b'H';
        assert!(decode_stored(&odd).is_err());
    }

    #[test]
    fn damaged_phonetic_data_is_refused_without_panicking() {
        let takes: Vec<Vec<u8>> = (0..3).map(|i| encode_take(&frames(i, 8))).collect();
        let model = build("Hey Ava", &takes, &[]).unwrap().model;
        assert!(decode_model(&model[..model.len() - 1], "Hey Ava").is_err());
        let mut longer = model.clone();
        longer.push(0);
        assert!(decode_model(&longer, "Hey Ava").is_err());
        let mut version = model.clone();
        version[4] ^= 1;
        assert!(decode_model(&version, "Hey Ava").is_err());
        let mut nan = model.clone();
        let at = model.len() - 4;
        nan[at..].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(decode_model(&nan, "Hey Ava").is_err());
        let mut huge = model.clone();
        huge[at..].copy_from_slice(&1e30f32.to_le_bytes());
        assert!(decode_model(&huge, "Hey Ava").is_err());
        for i in 0..model.len().min(600) {
            let mut m = model.clone();
            m[i] ^= 0x5a;
            let _ = decode_model(&m, "Hey Ava");
        }
        assert!(build("Hey Ava", &takes[..2], &[]).is_err());
        assert!(build("Hey Ava", &vec![vec![1, 2, 3]; 3], &[]).is_err());
        let long = ContrastTake {
            label: "x".repeat(10_000),
            features: takes[0].clone(),
        };
        assert!(build("Hey Ava", &takes, &[long]).is_err());
        assert!(build(&"y".repeat(10_000), &takes, &[]).is_err());
    }

    #[test]
    fn template_edges_keep_two_frames_around_speech() {
        assert_eq!(trim_span(&[1., 1., 1., 0.5, 0.2, 1., 1., 1., 1.]), (1, 7));
        assert_eq!(trim_span(&[1.; 4]), (0, 4));
        assert_eq!(trim_span(&[0.1]), (0, 1));
        assert_eq!(unit(vec![[0.; EMBED]])[0], [0.; EMBED]);
    }

    #[cfg(not(all(feature = "personal-phonetic", not(windows))))]
    #[test]
    fn without_the_feature_the_runtime_is_unavailable() {
        assert!(!phonetic_available());
        let _ = phonetic_take(".".into(), vec![0; 32_000], 16_000);
        assert!(matches!(
            phonetic_model("Hey Ava".into(), Vec::new(), Vec::new()),
            Err(MobileError::Unavailable { .. })
        ));
    }
}
