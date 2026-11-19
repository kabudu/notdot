//! Experimental personalised wake-phrase templates: a local diagnostic only.
//!
//! The user records a few takes of their wake phrase (and, optionally, of
//! a few similar phrases such as "Hey Ada"); each take becomes a small
//! matrix of acoustic features (MFCC and deltas, 10 ms frames). A held-out
//! recording is then compared with every template by subsequence dynamic
//! time warping, and the result is reported as distances, not a verdict on
//! reliability. This is wake-phrase matching only: it is not speaker
//! authentication and proves nothing about identity.
//!
//! Nothing here is connected to background activation. Raw audio is only
//! read, never kept: features are derived in memory and the samples are
//! dropped when each call returns. Models are bound to the phrase they were
//! made for, and versioned; native code encrypts them at rest.
//!
//! Bounds: takes up to 4 s, test recordings up to 6 s, 16 kHz 16-bit mono
//! only; at most 8 phrase takes and 6 contrast takes of at most 300 frames
//! each, so one comparison is at most 14 x 300 x 600 cells.
use crate::MobileError;

pub const RATE: u32 = 16_000;
pub(crate) const MAX_TAKE_SAMPLES: usize = 4 * RATE as usize;
pub(crate) const MAX_TEST_SAMPLES: usize = 6 * RATE as usize;
const MIN_SAMPLES: usize = RATE as usize * 3 / 10;
pub const MAX_POSITIVES: usize = 8;
pub const MIN_POSITIVES: usize = 3;
pub const MAX_CONTRASTS: usize = 6;
const MAX_TEMPLATE_FRAMES: usize = 300;
const MIN_TEMPLATE_FRAMES: usize = 5;
const MAX_LABEL: usize = 40;
const MAX_PHRASE: usize = 120;

const WIN: usize = 400;
const HOP: usize = 160;
const NFFT: usize = 512;
const MELS: usize = 40;
const CEPS: usize = 13;
pub const DIMS: usize = 2 * CEPS;
/// Frames within 30 dB (natural-log power 6.9) of the loudest are speech.
const SPEECH_RANGE: f64 = 6.9;
const TRIM_PAD: usize = 3;

const FEATURE_MAGIC: &[u8; 4] = b"NDPF";
const MODEL_MAGIC: &[u8; 4] = b"NDPW";
/// Bump when features or matching change: older models must be re-made.
pub const MODEL_VERSION: u16 = 1;

/// Why a recording cannot be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum TakeIssue {
    NoSpeech,
    TooQuiet,
    Clipped,
    TooShort,
    TooLong,
    Noisy,
}

/// One checked take: its features (opaque, in memory only) if usable.
#[derive(Clone, Debug, uniffi::Record)]
pub struct PersonalTake {
    pub issue: Option<TakeIssue>,
    pub speech_ms: u32,
    pub features: Vec<u8>,
}

/// A take of a similar phrase the wake phrase must be told apart from.
#[derive(Clone, Debug, uniffi::Record)]
pub struct ContrastTake {
    pub label: String,
    pub features: Vec<u8>,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct PersonalModelBuild {
    pub model: Vec<u8>,
    /// Phrase takes much further from the others than usual (indices into
    /// the positives): worth recording again.
    pub inconsistent: Vec<u32>,
}

/// One test recording against a model. Distances are per matched frame
/// (0 = identical, 2 = opposite); smaller is closer.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct PersonalMatch {
    pub issue: Option<TakeIssue>,
    /// None when the recording could not be compared (see `issue`).
    pub nearest_phrase: Option<f32>,
    pub nearest_contrast: Option<f32>,
    pub contrast_label: Option<String>,
    /// Calibrated from the enrolment takes only.
    pub phrase_bound: f32,
    pub within_bound: bool,
    /// Closer to the phrase than to every contrast (true if none).
    pub closer_than_contrasts: bool,
    pub matched: bool,
}

pub(crate) fn invalid(detail: &str) -> MobileError {
    MobileError::Invalid {
        detail: detail.to_owned(),
    }
}

pub(crate) fn samples(pcm16: &[u8], rate: u32, max: usize) -> Result<Vec<f64>, MobileError> {
    if rate != RATE {
        return Err(invalid("Personal wake recordings must be 16 kHz"));
    }
    if !pcm16.len().is_multiple_of(2) {
        return Err(invalid("Recording is not whole 16-bit samples"));
    }
    if pcm16.len() / 2 > max {
        return Err(invalid("Recording is too long"));
    }
    Ok(pcm16
        .chunks_exact(2)
        .map(|b| f64::from(i16::from_le_bytes([b[0], b[1]])) / 32768.)
        .collect())
}

// ---- Signal processing (bounded, allocation per call only) ----

/// In-place radix-2 FFT of length NFFT.
fn fft(re: &mut [f64; NFFT], im: &mut [f64; NFFT]) {
    let mut j = 0;
    for i in 1..NFFT {
        let mut bit = NFFT >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= NFFT {
        let angle = -2. * std::f64::consts::PI / len as f64;
        for start in (0..NFFT).step_by(len) {
            for k in 0..len / 2 {
                let (s, c) = (angle * k as f64).sin_cos();
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * c - im[b] * s;
                let ti = re[b] * s + im[b] * c;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len <<= 1;
    }
}

struct Dsp {
    window: Vec<f64>,
    mel: Vec<Vec<(usize, f64)>>,
    dct: Vec<[f64; MELS]>,
}

impl Dsp {
    fn new() -> Self {
        // Symmetric Hann, as numpy.hanning.
        let window = (0..WIN)
            .map(|n| 0.5 - 0.5 * (2. * std::f64::consts::PI * n as f64 / (WIN - 1) as f64).cos())
            .collect();
        let hz2mel = |f: f64| 2595. * (1. + f / 700.).log10();
        let mel2hz = |m: f64| 700. * (10f64.powf(m / 2595.) - 1.);
        let (lo, hi) = (hz2mel(60.), hz2mel(7600.));
        let bins: Vec<usize> = (0..MELS + 2)
            .map(|i| {
                let m = lo + (hi - lo) * i as f64 / (MELS + 1) as f64;
                ((NFFT + 1) as f64 * mel2hz(m) / f64::from(RATE)).floor() as usize
            })
            .collect();
        let mel = (1..=MELS)
            .map(|m| {
                let (l, c, r) = (bins[m - 1], bins[m], bins[m + 1]);
                let mut taps = Vec::new();
                for k in l..c {
                    taps.push((k, (k - l) as f64 / (c - l).max(1) as f64));
                }
                for k in c..r {
                    taps.push((k, (r - k) as f64 / (r - c).max(1) as f64));
                }
                taps
            })
            .collect();
        let dct = (0..CEPS)
            .map(|k| {
                let mut row = [0.; MELS];
                for (m, v) in row.iter_mut().enumerate() {
                    *v = (std::f64::consts::PI / MELS as f64 * (m as f64 + 0.5) * k as f64).cos();
                }
                row
            })
            .collect();
        Self { window, mel, dct }
    }

    /// MFCC + deltas per frame, and each frame's loudest log-mel band.
    fn features(&self, x: &[f64]) -> (Vec<[f64; DIMS]>, Vec<f64>) {
        let mut pre = Vec::with_capacity(x.len());
        pre.push(x[0]);
        pre.extend(x.windows(2).map(|w| w[1] - 0.97 * w[0]));
        let frames = 1 + (pre.len() - WIN) / HOP;
        let mut cep = Vec::with_capacity(frames);
        let mut energy = Vec::with_capacity(frames);
        for f in 0..frames {
            let mut re = [0.; NFFT];
            let mut im = [0.; NFFT];
            for (n, v) in re.iter_mut().take(WIN).enumerate() {
                *v = pre[f * HOP + n] * self.window[n];
            }
            fft(&mut re, &mut im);
            let mut logmel = [0.; MELS];
            for (m, taps) in self.mel.iter().enumerate() {
                let p: f64 = taps
                    .iter()
                    .map(|&(k, w)| (re[k] * re[k] + im[k] * im[k]) * w)
                    .sum();
                logmel[m] = (p + 1e-8).ln();
            }
            energy.push(logmel.iter().copied().fold(f64::NEG_INFINITY, f64::max));
            let mut c = [0.; CEPS];
            for (k, row) in self.dct.iter().enumerate() {
                c[k] = row.iter().zip(&logmel).map(|(a, b)| a * b).sum();
            }
            cep.push(c);
        }
        // Deltas as numpy.gradient: centred inside, one-sided at the ends.
        let n = cep.len();
        let out = (0..n)
            .map(|i| {
                let mut v = [0.; DIMS];
                v[..CEPS].copy_from_slice(&cep[i]);
                for k in 0..CEPS {
                    v[CEPS + k] = if n < 2 {
                        0.
                    } else if i == 0 {
                        cep[1][k] - cep[0][k]
                    } else if i == n - 1 {
                        cep[n - 1][k] - cep[n - 2][k]
                    } else {
                        (cep[i + 1][k] - cep[i - 1][k]) / 2.
                    };
                }
                v
            })
            .collect();
        (out, energy)
    }
}

fn dsp() -> &'static Dsp {
    static DSP: std::sync::OnceLock<Dsp> = std::sync::OnceLock::new();
    DSP.get_or_init(Dsp::new)
}

/// The speech span of `x` in 10 ms frames (within 30 dB of the loudest,
/// plus 3 frames each side), as [`prepare`] trims templates. None if empty.
#[cfg_attr(
    not(all(feature = "personal-phonetic", not(windows))),
    allow(dead_code)
)]
pub(crate) fn speech_span(x: &[f64]) -> Option<(usize, usize)> {
    let (_, e) = dsp().features(x);
    let top = e.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let first = (0..e.len()).find(|&i| e[i] > top - SPEECH_RANGE)?;
    let last = (0..e.len()).rev().find(|&i| e[i] > top - SPEECH_RANGE)?;
    Some((
        first.saturating_sub(TRIM_PAD),
        (last + 1 + TRIM_PAD).min(e.len()),
    ))
}

/// Mean-normalised over speech frames, optionally trimmed to speech, and
/// each frame scaled to unit length.
fn prepare(x: &[f64], trim: bool) -> Vec<[f64; DIMS]> {
    let (mut f, e) = dsp().features(x);
    let top = e.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let speech: Vec<usize> = (0..e.len())
        .filter(|&i| e[i] > top - SPEECH_RANGE)
        .collect();
    let mut mean = [0.; DIMS];
    for &i in &speech {
        for (m, v) in mean.iter_mut().zip(&f[i]) {
            *m += v / speech.len() as f64;
        }
    }
    if trim && !speech.is_empty() {
        let a = speech[0].saturating_sub(TRIM_PAD);
        let b = (speech[speech.len() - 1] + 1 + TRIM_PAD).min(f.len());
        f = f[a..b].to_vec();
    }
    for frame in &mut f {
        for (v, m) in frame.iter_mut().zip(&mean) {
            *v -= m;
        }
        let norm = frame.iter().map(|v| v * v).sum::<f64>().sqrt() + 1e-8;
        for v in frame.iter_mut() {
            *v /= norm;
        }
    }
    f
}

/// Subsequence DTW of template `t` within query `q`: steps (1,1), (2,1),
/// (1,2) bound tempo to 0.5-2x; every traversed cell is charged (a (2,1)
/// step pays template frames i-1 and i at query frame j; a (1,2) step pays
/// template frame i at query frames j-1 and j); cost 1 - cosine; the result
/// is the mean cost per charged cell of the best path ending anywhere.
pub(crate) fn sdtw<const D: usize>(t: &[[f64; D]], q: &[[f64; D]]) -> f64 {
    let (tn, qn) = (t.len(), q.len());
    if tn == 0 || qn == 0 {
        return f64::INFINITY;
    }
    // Clamped: f32 storage can push a unit-vector dot product just past 1.
    let cost = |i: usize, j: usize| {
        1. - t[i]
            .iter()
            .zip(&q[j])
            .map(|(a, b)| a * b)
            .sum::<f64>()
            .clamp(-1., 1.)
    };
    let inf = f64::INFINITY;
    // Two previous columns of (cost, cells), plus the current one.
    let mut d2: Vec<(f64, f64)> = vec![(inf, 0.); tn];
    let mut d1: Vec<(f64, f64)> = vec![(inf, 0.); tn];
    let mut prev_col = vec![inf; tn];
    let mut best = inf;
    for j in 0..qn {
        let col: Vec<f64> = (0..tn).map(|i| cost(i, j)).collect();
        let mut cur: Vec<(f64, f64)> = vec![(inf, 0.); tn];
        cur[0] = (col[0], 1.);
        for i in 1..tn {
            let mut pick = (d1[i - 1].0, d1[i - 1].1 + 1.);
            if i >= 2 {
                let cand = d1[i - 2].0 + col[i - 1];
                if cand < pick.0 {
                    pick = (cand, d1[i - 2].1 + 2.);
                }
            }
            if j >= 1 {
                let cand = d2[i - 1].0 + prev_col[i];
                if cand < pick.0 {
                    pick = (cand, d2[i - 1].1 + 2.);
                }
            }
            cur[i] = (pick.0 + col[i], pick.1);
        }
        let (c, n) = cur[tn - 1];
        if c.is_finite() {
            best = best.min(c / n.max(1.));
        }
        d2 = std::mem::replace(&mut d1, cur);
        prev_col = col;
    }
    best
}

// ---- Checks ----

pub(crate) fn check(x: &[f64], max_speech_ms: u32) -> (Option<TakeIssue>, u32) {
    if x.len() < MIN_SAMPLES {
        return (Some(TakeIssue::TooShort), 0);
    }
    let peak = x.iter().fold(0f64, |m, v| m.max(v.abs()));
    if peak < 0.005 {
        return (Some(TakeIssue::NoSpeech), 0);
    }
    if peak < 0.02 {
        return (Some(TakeIssue::TooQuiet), 0);
    }
    let clipped = x.iter().filter(|v| v.abs() >= 0.99).count();
    if clipped * 1000 > x.len() {
        return (Some(TakeIssue::Clipped), 0);
    }
    let (_, e) = dsp().features(x);
    let top = e.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let mut sorted = e.clone();
    sorted.sort_by(f64::total_cmp);
    let floor = sorted[sorted.len() / 10];
    let speech: Vec<usize> = (0..e.len())
        .filter(|&i| e[i] > top - SPEECH_RANGE)
        .collect();
    let span = speech.last().map_or(0, |l| l + 1 - speech[0]);
    let speech_ms = (span * HOP * 1000 / RATE as usize) as u32;
    // Less than 15 dB between the quietest tenth and the loudest frame.
    if top - floor < 3.45 {
        return (Some(TakeIssue::Noisy), speech_ms);
    }
    if speech_ms < 250 {
        return (Some(TakeIssue::TooShort), speech_ms);
    }
    if speech_ms > max_speech_ms {
        return (Some(TakeIssue::TooLong), speech_ms);
    }
    (None, speech_ms)
}

// ---- Serialisation ----

fn encode_features(f: &[[f64; DIMS]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + f.len() * DIMS * 4);
    out.extend_from_slice(FEATURE_MAGIC);
    out.extend_from_slice(&MODEL_VERSION.to_le_bytes());
    out.extend_from_slice(&(f.len() as u16).to_le_bytes());
    for frame in f {
        for v in frame {
            out.extend_from_slice(&(*v as f32).to_le_bytes());
        }
    }
    out
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], MobileError> {
        if self.0.len() < n {
            return Err(invalid("Personal wake data is damaged"));
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
        if v.is_finite() {
            Ok(v)
        } else {
            Err(invalid("Personal wake data is damaged"))
        }
    }
    fn text(&mut self, max: usize) -> Result<String, MobileError> {
        let n = self.u8()? as usize;
        if n > max {
            return Err(invalid("Personal wake data is damaged"));
        }
        String::from_utf8(self.take(n)?.to_vec())
            .map_err(|_| invalid("Personal wake data is damaged"))
    }
    fn frames(&mut self) -> Result<Vec<[f64; DIMS]>, MobileError> {
        let n = self.u16()? as usize;
        if !(MIN_TEMPLATE_FRAMES..=MAX_TEMPLATE_FRAMES).contains(&n) {
            return Err(invalid("Personal wake data is damaged"));
        }
        (0..n)
            .map(|_| {
                let mut frame = [0.; DIMS];
                for v in &mut frame {
                    *v = f64::from(self.f32()?);
                }
                // Frames are unit length (or zero): anything else is damage.
                let norm2: f64 = frame.iter().map(|v| v * v).sum();
                if frame.iter().any(|v| v.abs() > 1. + 1e-3) || norm2 > 1. + 1e-3 {
                    return Err(invalid("Personal wake data is damaged"));
                }
                Ok(frame)
            })
            .collect()
    }
    fn end(&self) -> Result<(), MobileError> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(invalid("Personal wake data is damaged"))
        }
    }
}

fn decode_features(bytes: &[u8]) -> Result<Vec<[f64; DIMS]>, MobileError> {
    let mut r = Reader(bytes);
    if r.take(4)? != FEATURE_MAGIC || r.u16()? != MODEL_VERSION {
        return Err(invalid(
            "This take was made by another version; record it again",
        ));
    }
    let f = r.frames()?;
    r.end()?;
    Ok(f)
}

/// The phrase as compared for binding: case and spacing ignored.
pub(crate) fn phrase_key(phrase: &str) -> Result<String, MobileError> {
    if phrase.len() > 4 * MAX_PHRASE {
        return Err(invalid("Choose a shorter wake phrase"));
    }
    let key = phrase
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    if key.is_empty() || key.len() > MAX_PHRASE {
        return Err(invalid("Choose a wake phrase first"));
    }
    Ok(key)
}

struct Model {
    phrase: String,
    bound: f64,
    positives: Vec<Vec<[f64; DIMS]>>,
    contrasts: Vec<(String, Vec<[f64; DIMS]>)>,
}

fn encode_model(m: &Model) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MODEL_MAGIC);
    out.extend_from_slice(&MODEL_VERSION.to_le_bytes());
    let phrase = m.phrase.as_bytes();
    out.push(phrase.len() as u8);
    out.extend_from_slice(phrase);
    out.extend_from_slice(&(m.bound as f32).to_le_bytes());
    out.push(m.positives.len() as u8);
    out.push(m.contrasts.len() as u8);
    let frames = |out: &mut Vec<u8>, f: &[[f64; DIMS]]| {
        out.extend_from_slice(&(f.len() as u16).to_le_bytes());
        for frame in f {
            for v in frame {
                out.extend_from_slice(&(*v as f32).to_le_bytes());
            }
        }
    };
    for p in &m.positives {
        frames(&mut out, p);
    }
    for (label, c) in &m.contrasts {
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
        frames(&mut out, c);
    }
    out
}

fn decode_model(bytes: &[u8], phrase: &str) -> Result<Model, MobileError> {
    let mut r = Reader(bytes);
    if r.take(4)? != MODEL_MAGIC {
        return Err(invalid("Personal wake data is damaged"));
    }
    if r.u16()? != MODEL_VERSION {
        return Err(invalid(
            "These recordings were made by another version; record them again",
        ));
    }
    let stored = r.text(MAX_PHRASE)?;
    if stored != phrase_key(phrase)? {
        return Err(invalid(
            "These recordings are for a different wake phrase; record them again",
        ));
    }
    let bound = f64::from(r.f32()?);
    let (np, nc) = (r.u8()? as usize, r.u8()? as usize);
    if !(MIN_POSITIVES..=MAX_POSITIVES).contains(&np)
        || nc > MAX_CONTRASTS
        || !(0. ..=2.).contains(&bound)
    {
        return Err(invalid("Personal wake data is damaged"));
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

// ---- Exported API ----

/// Check one enrolment take (16 kHz 16-bit mono, at most 4 s) and derive
/// its features. The audio is not kept.
#[uniffi::export]
pub fn personal_take(pcm16: Vec<u8>, rate: u32) -> Result<PersonalTake, MobileError> {
    let x = samples(&pcm16, rate, MAX_TAKE_SAMPLES)?;
    drop(pcm16);
    let (issue, speech_ms) = check(&x, 2_500);
    let features = match issue {
        None => {
            let f = prepare(&x, true);
            let f = &f[..f.len().min(MAX_TEMPLATE_FRAMES)];
            if f.len() < MIN_TEMPLATE_FRAMES {
                return Ok(PersonalTake {
                    issue: Some(TakeIssue::TooShort),
                    speech_ms,
                    features: Vec::new(),
                });
            }
            encode_features(f)
        }
        Some(_) => Vec::new(),
    };
    Ok(PersonalTake {
        issue,
        speech_ms,
        features,
    })
}

/// Build a model for `phrase` from phrase takes (3-8) and optional contrast
/// takes (at most 6). The phrase bound is 1.2 x the largest distance of any
/// phrase take from the others (enrolment only).
#[uniffi::export]
pub fn personal_model(
    phrase: String,
    positives: Vec<Vec<u8>>,
    contrasts: Vec<ContrastTake>,
) -> Result<PersonalModelBuild, MobileError> {
    let phrase = phrase_key(&phrase)?;
    if !(MIN_POSITIVES..=MAX_POSITIVES).contains(&positives.len()) {
        return Err(invalid("Record three to eight takes of your wake phrase"));
    }
    if contrasts.len() > MAX_CONTRASTS {
        return Err(invalid("Record at most six similar-phrase takes"));
    }
    let positives = positives
        .iter()
        .map(|p| decode_features(p))
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
            Ok((label.to_owned(), decode_features(&c.features)?))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let loo: Vec<f64> = (0..positives.len())
        .map(|k| {
            positives
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != k)
                .map(|(_, t)| sdtw(t, &positives[k]))
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
    let model = Model {
        phrase,
        bound,
        positives,
        contrasts,
    };
    Ok(PersonalModelBuild {
        model: encode_model(&model),
        inconsistent,
    })
}

/// What a saved model holds, for display. Fails as [`personal_match`] does.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct PersonalModelSummary {
    pub phrase_takes: u32,
    pub contrast_labels: Vec<String>,
}

#[uniffi::export]
pub fn personal_model_summary(
    model: Vec<u8>,
    phrase: String,
) -> Result<PersonalModelSummary, MobileError> {
    let m = decode_model(&model, &phrase)?;
    Ok(PersonalModelSummary {
        phrase_takes: m.positives.len() as u32,
        contrast_labels: m.contrasts.into_iter().map(|(label, _)| label).collect(),
    })
}

/// Compare a held-out recording (16 kHz 16-bit mono, at most 6 s) with a
/// model made for `phrase`. Fails if the model is damaged, from another
/// version, or for another phrase.
#[uniffi::export]
pub fn personal_match(
    model: Vec<u8>,
    phrase: String,
    pcm16: Vec<u8>,
    rate: u32,
) -> Result<PersonalMatch, MobileError> {
    let m = decode_model(&model, &phrase)?;
    let x = samples(&pcm16, rate, MAX_TEST_SAMPLES)?;
    drop(pcm16);
    let (issue, _) = check(&x, 6_000);
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
    let q = prepare(&x, false);
    let nearest = m
        .positives
        .iter()
        .map(|t| sdtw(t, &q))
        .fold(f64::INFINITY, f64::min);
    let contrast = m
        .contrasts
        .iter()
        .map(|(label, t)| (sdtw(t, &q), label))
        .min_by(|a, b| a.0.total_cmp(&b.0));
    let within = nearest <= m.bound;
    let closer = contrast.is_none_or(|(c, _)| nearest < c);
    Ok(PersonalMatch {
        issue: None,
        nearest_phrase: Some(nearest as f32),
        nearest_contrast: contrast.map(|(c, _)| c as f32),
        contrast_label: contrast.map(|(_, l)| l.clone()),
        phrase_bound: m.bound as f32,
        within_bound: within,
        closer_than_contrasts: closer,
        matched: within && closer,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(v: [f64; 2]) -> [f64; DIMS] {
        let mut f = [0.; DIMS];
        f[0] = v[0];
        f[1] = v[1];
        f
    }

    /// The review's regression and the exact/tempo controls, as asserted in
    /// the Python baseline.
    #[test]
    fn dtw_charges_every_template_frame() {
        let (a, b) = (unit([1., 0.]), unit([0., 1.]));
        let tmpl = [a, a, b, a, a];
        let missing = sdtw(&tmpl, &[a; 8]);
        assert!((missing - 0.2).abs() < 1e-12, "{missing}");
        assert_eq!(sdtw(&[a; 5], &[a; 8]), 0.);
        let padded: Vec<_> = [a; 3].iter().chain(&tmpl).chain(&[a; 3]).copied().collect();
        assert_eq!(sdtw(&tmpl, &padded), 0.);
        let doubled: Vec<_> = tmpl.iter().flat_map(|f| [*f, *f]).collect();
        assert_eq!(sdtw(&tmpl, &doubled), 0.);
        assert_eq!(sdtw(&doubled, &padded), 0.);
        let tripled: Vec<_> = tmpl.iter().flat_map(|f| [*f, *f, *f]).collect();
        assert!(sdtw(&tripled, &[a, b, a]) > 0.);
        assert_eq!(sdtw(&[], &[a]), f64::INFINITY);
    }

    #[test]
    fn fft_matches_a_direct_transform() {
        let mut re = [0.; NFFT];
        let mut im = [0.; NFFT];
        for (n, v) in re.iter_mut().enumerate() {
            *v = (n as f64 * 0.37).sin() + 0.3 * (n as f64 * 1.9).cos();
        }
        let input = re;
        fft(&mut re, &mut im);
        for k in [0, 1, 7, 100, 255, 256] {
            let (mut r, mut i) = (0., 0.);
            for (n, x) in input.iter().enumerate() {
                let a = -2. * std::f64::consts::PI * (k * n) as f64 / NFFT as f64;
                r += x * a.cos();
                i += x * a.sin();
            }
            assert!(
                (re[k] - r).abs() < 1e-9 && (im[k] - i).abs() < 1e-9,
                "bin {k}"
            );
        }
    }

    /// A deterministic "syllable" signal: tones at two pitches with silence
    /// around, so tests need no recordings.
    fn speech(pitches: &[f64], gain: f64) -> Vec<f64> {
        let mut x = vec![0.; 4_800];
        let mut noise = 7u32;
        for &p in pitches {
            for n in 0..3_200 {
                let env = (std::f64::consts::PI * n as f64 / 3_200.).sin();
                x.push(
                    gain * env
                        * ((2. * std::f64::consts::PI * p * n as f64 / 16_000.).sin()
                            + 0.5
                                * (2. * std::f64::consts::PI * 2.7 * p * n as f64 / 16_000.).sin()),
                );
            }
        }
        x.extend(std::iter::repeat_n(0., 4_800));
        // A faint floor, as a real microphone has.
        for v in &mut x {
            noise ^= noise << 13;
            noise ^= noise >> 17;
            noise ^= noise << 5;
            *v += (noise as f64 / u32::MAX as f64 - 0.5) * 1e-4;
        }
        x
    }

    fn pcm(x: &[f64]) -> Vec<u8> {
        x.iter()
            .flat_map(|v| ((v.clamp(-1., 1.) * 32767.) as i16).to_le_bytes())
            .collect()
    }

    #[test]
    fn takes_are_checked_and_features_bounded() {
        let ok = personal_take(pcm(&speech(&[300., 500., 400.], 0.3)), RATE).unwrap();
        assert_eq!(ok.issue, None, "{ok:?}");
        assert!(
            ok.speech_ms > 400 && ok.speech_ms < 1_000,
            "{}",
            ok.speech_ms
        );
        assert!(!ok.features.is_empty());
        let issue = |x: &[f64]| personal_take(pcm(x), RATE).unwrap().issue;
        assert_eq!(issue(&vec![0.; 16_000]), Some(TakeIssue::NoSpeech));
        assert_eq!(
            issue(&speech(&[300., 500.], 0.01)),
            Some(TakeIssue::TooQuiet)
        );
        assert_eq!(issue(&speech(&[300., 500.], 4.)), Some(TakeIssue::Clipped));
        assert_eq!(issue(&vec![0.; 1_000]), Some(TakeIssue::TooShort));
        assert_eq!(issue(&speech(&[300.; 14], 0.3)), Some(TakeIssue::TooLong));
        let hiss: Vec<f64> = speech(&[300., 500.], 0.3)
            .iter()
            .enumerate()
            .map(|(n, v)| v + 0.2 * ((n as f64 * 12.9898).sin() * 43_758.545).fract())
            .collect();
        assert_eq!(issue(&hiss), Some(TakeIssue::Noisy));
        assert!(personal_take(pcm(&speech(&[300.], 0.3)), 48_000).is_err());
        assert!(personal_take(vec![0; 3], RATE).is_err());
        assert!(personal_take(vec![0; MAX_TAKE_SAMPLES * 2 + 2], RATE).is_err());
    }

    fn enrol(contrast: bool) -> Vec<u8> {
        let takes: Vec<Vec<u8>> = [0.25, 0.3, 0.35, 0.4, 0.5]
            .iter()
            .map(|g| {
                personal_take(pcm(&speech(&[300., 500., 400.], *g)), RATE)
                    .unwrap()
                    .features
            })
            .collect();
        let contrasts = if contrast {
            vec![ContrastTake {
                label: "Similar".into(),
                features: personal_take(pcm(&speech(&[300., 700., 400.], 0.3)), RATE)
                    .unwrap()
                    .features,
            }]
        } else {
            Vec::new()
        };
        personal_model("Hey  Ava".into(), takes, contrasts)
            .unwrap()
            .model
    }

    #[test]
    fn matching_reports_distances_and_binds_the_phrase() {
        let model = enrol(true);
        let same = personal_match(
            model.clone(),
            "hey ava".into(),
            pcm(&speech(&[300., 500., 400.], 0.6)),
            RATE,
        )
        .unwrap();
        assert!(
            same.matched && same.within_bound && same.closer_than_contrasts,
            "{same:?}"
        );
        assert_eq!(same.contrast_label.as_deref(), Some("Similar"));
        let near = personal_match(
            model.clone(),
            "Hey Ava".into(),
            pcm(&speech(&[300., 700., 400.], 0.3)),
            RATE,
        )
        .unwrap();
        assert!(!near.closer_than_contrasts && !near.matched, "{near:?}");
        let other = personal_match(
            model.clone(),
            "Hey Ava".into(),
            pcm(&speech(&[900., 150.], 0.3)),
            RATE,
        )
        .unwrap();
        assert!(!other.within_bound && !other.matched, "{other:?}");
        let silent = personal_match(
            model.clone(),
            "Hey Ava".into(),
            pcm(&vec![0.; 16_000]),
            RATE,
        )
        .unwrap();
        assert_eq!(silent.issue, Some(TakeIssue::NoSpeech));
        assert!(!silent.matched && silent.nearest_phrase.is_none());
        assert!(same.nearest_phrase.is_some_and(|d| (0. ..=2.).contains(&d)));
        assert_eq!(
            personal_model_summary(model.clone(), "HEY AVA".into()).unwrap(),
            PersonalModelSummary {
                phrase_takes: 5,
                contrast_labels: vec!["Similar".into()]
            }
        );
        assert!(personal_model_summary(model.clone(), "Hey Nova".into()).is_err());
        // Renamed phrase: never reused.
        assert!(
            personal_match(model, "Hey Nova".into(), pcm(&speech(&[300.], 0.3)), RATE).is_err()
        );
        // Positive-only models match on the bound alone.
        let plain = personal_match(
            enrol(false),
            "Hey Ava".into(),
            pcm(&speech(&[300., 500., 400.], 0.45)),
            RATE,
        )
        .unwrap();
        assert!(
            plain.matched && plain.nearest_contrast.is_none(),
            "{plain:?}"
        );
    }

    #[test]
    fn damaged_or_foreign_models_are_refused() {
        let model = enrol(true);
        let test = || pcm(&speech(&[300., 500., 400.], 0.3));
        assert!(personal_match(model.clone(), "Hey Ava".into(), test(), RATE).is_ok());
        let refused = |m: Vec<u8>| personal_match(m, "Hey Ava".into(), test(), RATE).is_err();
        assert!(refused(Vec::new()));
        assert!(refused(model[..model.len() - 1].to_vec()));
        let mut longer = model.clone();
        longer.push(0);
        assert!(refused(longer));
        let mut magic = model.clone();
        magic[0] ^= 1;
        assert!(refused(magic));
        let mut version = model.clone();
        version[4] = version[4].wrapping_add(1);
        assert!(refused(version));
        // A NaN in the templates, and a template count beyond the bounds.
        let mut nan = model.clone();
        let at = model.len() - 4;
        nan[at..].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(refused(nan));
        let phrase_len = model[6] as usize;
        let mut many = model.clone();
        many[7 + phrase_len + 4] = 200;
        assert!(refused(many));
        // Every single-byte corruption is either refused or still a
        // well-formed comparison; none panics.
        for i in 0..model.len().min(400) {
            let mut m = model.clone();
            m[i] ^= 0x5a;
            let _ = personal_match(m, "Hey Ava".into(), test(), RATE);
        }
        // Bad inputs to the builder.
        let take = personal_take(test(), RATE).unwrap().features;
        assert!(personal_model("Hey Ava".into(), vec![take.clone(); 2], Vec::new()).is_err());
        assert!(personal_model("Hey Ava".into(), vec![take.clone(); 9], Vec::new()).is_err());
        assert!(personal_model(" ".into(), vec![take.clone(); 5], Vec::new()).is_err());
        assert!(personal_model("Hey Ava".into(), vec![vec![1, 2, 3]; 5], Vec::new()).is_err());
        let bad_label = ContrastTake {
            label: " ".into(),
            features: take.clone(),
        };
        assert!(personal_model("Hey Ava".into(), vec![take; 5], vec![bad_label]).is_err());
    }

    /// Review regression: identical unit-length takes whose f32 rounding
    /// makes their dot product slightly above 1 still build, decode and
    /// compare with finite distances in [0, 2] and a non-negative bound.
    #[test]
    fn rounding_never_makes_distances_or_bounds_negative() {
        let raw: Vec<f64> = (0..DIMS)
            .map(|k| 0.1 + (k as f64 * 0.731).sin().abs())
            .collect();
        let norm = raw.iter().map(|v| v * v).sum::<f64>().sqrt();
        let mut frame = [0.; DIMS];
        for (f, r) in frame.iter_mut().zip(&raw) {
            *f = r / norm;
        }
        let stored: Vec<f64> = frame.iter().map(|v| f64::from(*v as f32)).collect();
        let dot: f64 = stored.iter().map(|v| v * v).sum();
        assert!(
            dot != 1.0,
            "the regression needs a non-exact f32 unit norm ({dot})"
        );
        let take = encode_features(&vec![frame; 20]);
        let build = personal_model("Hey Ava".into(), vec![take.clone(); 3], Vec::new()).unwrap();
        let m = decode_model(&build.model, "hey ava").unwrap();
        assert!(m.bound >= 0. && m.bound <= 2., "{}", m.bound);
        for t in &m.positives {
            let d = sdtw(t, &m.positives[0]);
            assert!(d.is_finite() && (0. ..=2.).contains(&d), "{d}");
        }
        assert_eq!(
            personal_model_summary(build.model, "Hey Ava".into())
                .unwrap()
                .phrase_takes,
            3
        );
    }

    #[test]
    fn huge_or_non_unit_feature_values_are_refused() {
        let mut frame = [0.; DIMS];
        frame[0] = 1e30;
        let huge = encode_features(&vec![frame; 20]);
        assert!(personal_model("Hey Ava".into(), vec![huge; 3], Vec::new()).is_err());
        let mut frame = [0.; DIMS];
        frame[0] = 0.8;
        frame[1] = 0.8; // length 1.13
        let long = encode_features(&vec![frame; 20]);
        assert!(personal_model("Hey Ava".into(), vec![long; 3], Vec::new()).is_err());
        assert!(personal_model("x".repeat(10_000), Vec::new(), Vec::new()).is_err());
        let ok = personal_take(pcm(&speech(&[300., 500., 400.], 0.3)), RATE)
            .unwrap()
            .features;
        let long_label = ContrastTake {
            label: "y".repeat(10_000),
            features: ok.clone(),
        };
        assert!(personal_model("Hey Ava".into(), vec![ok; 5], vec![long_label]).is_err());
    }

    /// Worst-case work: 14 longest templates against a 6 s recording.
    #[test]
    fn the_largest_comparison_is_bounded() {
        let long = speech(
            &[
                300., 500., 400., 600., 350., 450., 550., 380., 420., 480., 520., 360.,
            ],
            0.3,
        );
        let frames = prepare(&long, true);
        assert!(frames.len() >= MAX_TEMPLATE_FRAMES / 2);
        let t: Vec<_> = frames
            .iter()
            .cycle()
            .take(MAX_TEMPLATE_FRAMES)
            .copied()
            .collect();
        let m = Model {
            phrase: "hey ava".into(),
            bound: 0.5,
            positives: vec![t.clone(); MAX_POSITIVES],
            contrasts: vec![("x".into(), t); MAX_CONTRASTS],
        };
        let mut test = long.clone();
        test.resize(MAX_TEST_SAMPLES, 0.);
        let started = std::time::Instant::now();
        let result = personal_match(encode_model(&m), "Hey Ava".into(), pcm(&test), RATE).unwrap();
        let took = started.elapsed();
        eprintln!("largest comparison: {took:?} ({result:?})");
        // Debug build on a host; release on a phone is far faster.
        assert!(took < std::time::Duration::from_secs(20), "{took:?}");
    }

    /// Parity with the Python baseline on the local synthetic corpus (not in
    /// the repository): set NOTDOT_PERSONAL_CORPUS and compare with the
    /// numbers printed by .local/eval/personal_parity.py.
    #[test]
    #[ignore = "set NOTDOT_PERSONAL_CORPUS to the local synthetic corpus"]
    fn synthetic_parity_with_the_python_baseline() {
        let dir = std::path::PathBuf::from(std::env::var("NOTDOT_PERSONAL_CORPUS").unwrap());
        let read = |name: &str| -> Vec<f64> {
            let mut r = hound::WavReader::open(dir.join(name)).unwrap();
            r.samples::<i16>()
                .map(|s| f64::from(s.unwrap()) / 32768.)
                .collect()
        };
        let padded = |x: Vec<f64>| -> Vec<f64> {
            let peak = x.iter().fold(1e-9f64, |m, v| m.max(v.abs()));
            let mut y = vec![0.; 8_000];
            y.extend(x.iter().map(|v| v / peak * 0.3));
            y.extend(std::iter::repeat_n(0., 8_000));
            y
        };
        for voice in ["danielenglishuk", "karen", "samanthaenglishus"] {
            let templates: Vec<_> = (1..=5)
                .map(|i| prepare(&read(&format!("{voice}-enrol-{i}.wav")), true))
                .collect();
            for name in ["pos-1", "pos-4", "neg-1-160", "neg-3-205"] {
                let q = prepare(&padded(read(&format!("{voice}-{name}.wav"))), false);
                let s = templates
                    .iter()
                    .map(|t| sdtw(t, &q))
                    .fold(f64::INFINITY, f64::min);
                println!("parity {voice} {name} {s:.6}");
            }
        }
    }
}
