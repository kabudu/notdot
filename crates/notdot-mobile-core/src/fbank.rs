//! Kaldi log-mel filterbank, as sherpa-onnx computes features for its
//! streaming transducers (kaldi-native-fbank with 80 bins, 16 kHz, dither
//! 0, snip_edges false, 20 Hz to 7600 Hz, samples in [-1, 1]).
//!
//! sherpa-onnx does not expose its feature extractor, so this is a port of
//! the same steps; `tests` checks it against golden values from
//! kaldi-native-fbank 1.23.1 (tests/fixtures/fbank-golden.txt). Per frame:
//! a 25 ms window centred on each 10 ms step (samples past either end are
//! reflected), DC removed, pre-emphasis 0.97 (the first sample against
//! itself), Povey window, 512-point power spectrum, 80 triangular mel bands
//! over FFT bins 0-255, and the natural log floored at f32 epsilon.

pub const BINS: usize = 80;
const RATE: f64 = 16_000.;
const SHIFT: usize = 160;
const LENGTH: usize = 400;
const NFFT: usize = 512;
const PREEMPH: f64 = 0.97;
const LOW_HZ: f64 = 20.;
const HIGH_HZ: f64 = RATE / 2. - 400.;

struct Tables {
    window: Vec<f64>,
    /// Per band: (first FFT bin, weights).
    bands: Vec<(usize, Vec<f64>)>,
    twiddles: Vec<(f64, f64)>,
}

fn mel(hz: f64) -> f64 {
    1127. * (1. + hz / 700.).ln()
}

fn tables() -> &'static Tables {
    static T: std::sync::OnceLock<Tables> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let a = 2. * std::f64::consts::PI / (LENGTH - 1) as f64;
        let window = (0..LENGTH)
            .map(|i| (0.5 - 0.5 * (a * i as f64).cos()).powf(0.85))
            .collect();
        let (lo, hi) = (mel(LOW_HZ), mel(HIGH_HZ));
        let delta = (hi - lo) / (BINS + 1) as f64;
        let width = RATE / NFFT as f64;
        let bands = (0..BINS)
            .map(|b| {
                let (left, centre, right) = (
                    lo + b as f64 * delta,
                    lo + (b + 1) as f64 * delta,
                    lo + (b + 2) as f64 * delta,
                );
                let mut first = None;
                let mut weights = Vec::new();
                for i in 0..NFFT / 2 {
                    let m = mel(width * i as f64);
                    if m > left && m < right {
                        first.get_or_insert(i);
                        weights.push(if m <= centre {
                            (m - left) / (centre - left)
                        } else {
                            (right - m) / (right - centre)
                        });
                    } else if first.is_some() {
                        break;
                    }
                }
                (first.unwrap_or(0), weights)
            })
            .collect();
        let twiddles = (0..NFFT / 2)
            .map(|k| {
                let ang = -2. * std::f64::consts::PI * k as f64 / NFFT as f64;
                (ang.cos(), ang.sin())
            })
            .collect();
        Tables {
            window,
            bands,
            twiddles,
        }
    })
}

fn fft(re: &mut [f64; NFFT], im: &mut [f64; NFFT], tw: &[(f64, f64)]) {
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
        let step = NFFT / len;
        for start in (0..NFFT).step_by(len) {
            for k in 0..len / 2 {
                let (c, s) = tw[k * step];
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

/// Number of frames for `n` samples (Kaldi, snip_edges false, flushed).
pub fn frames(n: usize) -> usize {
    (n + SHIFT / 2) / SHIFT
}

/// Log-mel frames for `x` (16 kHz, in [-1, 1]). Empty input gives none.
pub fn compute(x: &[f32]) -> Vec<[f32; BINS]> {
    let n = x.len();
    (0..frames(n))
        .map(|f| {
            let start = (f * SHIFT + SHIFT / 2) as i64 - (LENGTH / 2) as i64;
            let mut w = [0f64; LENGTH];
            for (i, v) in w.iter_mut().enumerate() {
                let mut s = start + i as i64;
                // Reflect past either end, as Kaldi does.
                while s < 0 || s >= n as i64 {
                    s = if s < 0 { -s - 1 } else { 2 * n as i64 - 1 - s };
                }
                *v = f64::from(x[s as usize]);
            }
            frame(w)
        })
        .collect()
}

/// One frame from its 25 ms of samples.
fn frame(mut w: [f64; LENGTH]) -> [f32; BINS] {
    let t = tables();
    let floor = f64::from(f32::EPSILON);
    let mean = w.iter().sum::<f64>() / LENGTH as f64;
    for v in &mut w {
        *v -= mean;
    }
    for i in (1..LENGTH).rev() {
        w[i] -= PREEMPH * w[i - 1];
    }
    w[0] -= PREEMPH * w[0];
    let mut re = [0f64; NFFT];
    let mut im = [0f64; NFFT];
    for i in 0..LENGTH {
        re[i] = w[i] * t.window[i];
    }
    fft(&mut re, &mut im, &t.twiddles);
    let mut out = [0f32; BINS];
    for (b, (first, weights)) in t.bands.iter().enumerate() {
        let e: f64 = weights
            .iter()
            .enumerate()
            .map(|(k, wt)| {
                let i = first + k;
                wt * (re[i] * re[i] + im[i] * im[i])
            })
            .sum();
        out[b] = e.max(floor).ln() as f32;
    }
    out
}

/// The same frames as [`compute`] for audio that arrives in pieces: each
/// frame is produced once its last sample has arrived. Frames near the start
/// reflect the first samples, as [`compute`] does; there is no end, so the
/// end reflection never applies. Holds under one frame of samples.
#[derive(Default)]
pub struct Stream {
    pending: Vec<f32>,
    /// Absolute index of `pending[0]`.
    base: usize,
    next: usize,
}

impl Stream {
    pub fn push(&mut self, x: &[f32]) -> Vec<[f32; BINS]> {
        self.pending.extend_from_slice(x);
        let have = self.base + self.pending.len();
        let mut out = Vec::new();
        loop {
            let centre = self.next * SHIFT + SHIFT / 2;
            let start = centre as i64 - (LENGTH / 2) as i64;
            let end = start + LENGTH as i64;
            if end > have as i64 {
                break;
            }
            let mut w = [0f64; LENGTH];
            for (i, v) in w.iter_mut().enumerate() {
                let mut s = start + i as i64;
                if s < 0 {
                    s = -s - 1; // only before the first sample, which is still held
                }
                *v = f64::from(self.pending[s as usize - self.base]);
            }
            out.push(frame(w));
            self.next += 1;
            // Keep what the next frame (and the start reflection) still needs.
            let keep_from = (self.next * SHIFT + SHIFT / 2).saturating_sub(LENGTH / 2);
            let keep_from = if keep_from < LENGTH { 0 } else { keep_from };
            if keep_from > self.base {
                self.pending.drain(..keep_from - self.base);
                self.base = keep_from;
            }
        }
        out
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Must match `signal` in .local/eval/fbank_golden.py.
    pub(crate) fn golden_signal(kind: u32, n: usize) -> Vec<f32> {
        let mut state: u64 = 12345;
        (0..n)
            .map(|i| {
                state = (state * 1_103_515_245 + 12_345) % (1 << 31);
                let noise = state as f64 / (1u64 << 31) as f64 - 0.5;
                let t = i as f64 / 16_000.;
                let v = match kind {
                    0 => {
                        0.3 * (2. * std::f64::consts::PI * 220. * t).sin()
                            + 0.2 * (2. * std::f64::consts::PI * 1330. * t).sin()
                            + 0.05 * noise
                    }
                    1 => 0.5 * noise,
                    _ => 0.,
                };
                v as f32
            })
            .collect()
    }

    /// Golden parity with kaldi-native-fbank 1.23.1: a tone with noise,
    /// noise shorter than one frame (reflection at both ends), and silence
    /// (the epsilon floor).
    #[test]
    fn matches_kaldi_native_fbank() {
        let text = include_str!("../tests/fixtures/fbank-golden.txt");
        let mut lines = text.lines().filter(|l| !l.starts_with('#'));
        let mut checked = 0;
        while let Some(header) = lines.next() {
            let h: Vec<usize> = header
                .split_whitespace()
                .skip(1)
                .map(|v| v.parse().unwrap())
                .collect();
            let (kind, n, count) = (h[0] as u32, h[1], h[2]);
            let ours = compute(&golden_signal(kind, n));
            assert_eq!(ours.len(), count, "signal {kind}: frame count");
            assert_eq!(frames(n), count);
            let mut worst = 0f32;
            for frame in &ours {
                let want: Vec<f32> = lines
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .map(|v| v.parse().unwrap())
                    .collect();
                assert_eq!(want.len(), BINS);
                for (a, b) in frame.iter().zip(&want) {
                    worst = worst.max((a - b).abs());
                }
            }
            // Golden values are printed to 5 decimals from f32 arithmetic.
            assert!(worst < 2e-3, "signal {kind}: max abs difference {worst}");
            checked += 1;
        }
        assert_eq!(checked, 3);
    }

    /// Streaming gives exactly the whole-clip frames, whatever the piece
    /// sizes, for every frame whose samples have all arrived.
    #[test]
    fn streaming_matches_whole_clip_frames() {
        let x = golden_signal(0, 16_000);
        let whole = compute(&x);
        for piece in [1usize, 160, 320, 333, 4_000] {
            let mut s = Stream::default();
            let mut got = Vec::new();
            for c in x.chunks(piece) {
                got.extend(s.push(c));
            }
            // The last frames of `compute` reflect past the end; a stream has no end.
            let complete = (x.len() - 280) / SHIFT + 1;
            assert_eq!(got.len(), complete, "piece {piece}");
            assert_eq!(got[..], whole[..complete], "piece {piece}");
            assert!(
                s.pending.len() <= LENGTH + 2 * SHIFT + piece,
                "piece {piece}: held {}",
                s.pending.len()
            );
        }
        assert!(Stream::default().push(&[]).is_empty());
    }

    #[test]
    fn edge_cases_are_bounded() {
        assert!(compute(&[]).is_empty());
        assert_eq!(compute(&[0.5]).len(), 0);
        assert_eq!(compute(&[0.5; 80]).len(), 1);
        assert!(
            compute(&[0.; 400])
                .iter()
                .flatten()
                .all(|v| (*v - f32::EPSILON.ln()).abs() < 1e-6)
        );
        assert!(
            compute(&vec![1.; 16_000])
                .iter()
                .flatten()
                .all(|v| v.is_finite())
        );
    }
}
