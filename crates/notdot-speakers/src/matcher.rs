//! Scoring an embedding against enrolled voiceprints.
use serde::Serialize;
use zeroize::Zeroize;

/// One person's enrolled embeddings. Keyed by the opaque profile ID only.
#[derive(Clone, Debug, PartialEq)]
pub struct Voiceprint {
    pub profile: String,
    /// L2-normalised enrolment embeddings (3–[`MAX_SAMPLES`]).
    pub samples: Vec<Vec<f32>>,
}
pub const MIN_SAMPLES: usize = 3;
pub const MAX_SAMPLES: usize = 6;
impl Drop for Voiceprint {
    fn drop(&mut self) {
        for s in &mut self.samples {
            s.zeroize();
        }
    }
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

pub fn centroid(samples: &[Vec<f32>]) -> Vec<f32> {
    let dim = samples.first().map_or(0, Vec::len);
    let mut c = vec![0.0f32; dim];
    for s in samples {
        for (acc, x) in c.iter_mut().zip(s) {
            *acc += x;
        }
    }
    let norm = c.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    c.iter_mut().for_each(|x| *x /= norm);
    c
}

impl Voiceprint {
    pub fn score(&self, embedding: &[f32]) -> f32 {
        cosine(&centroid(&self.samples), embedding)
    }
    fn closest_sample(&self, embedding: &[f32]) -> f32 {
        self.samples
            .iter()
            .map(|s| cosine(s, embedding))
            .fold(f32::MIN, f32::max)
    }
}

/// Decision bands. Defaults are calibrated on notdot's synthetic evidence set
/// (see docs/validation/2026-11-04-speaker-identity.md) and are deliberately
/// conservative: an uncertain match is refused, not guessed.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Thresholds {
    /// Minimum centroid cosine to recognise.
    pub accept: f32,
    /// Below this the voice is unknown; between `reject` and `accept` it is
    /// ambiguous.
    pub reject: f32,
    /// Best profile must beat the runner-up by this much.
    pub margin: f32,
    /// Cosine to a single stored enrolment sample at or above this is treated
    /// as a possible verbatim replay of that recording.
    pub replay: f32,
}
impl Default for Thresholds {
    fn default() -> Self {
        Self {
            accept: 0.70,
            reject: 0.50,
            margin: 0.10,
            replay: 0.985,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "result")]
pub enum Match {
    Recognised {
        profile: String,
        score: f32,
        margin: f32,
    },
    Ambiguous {
        best: f32,
        runner_up: f32,
    },
    SuspectedReplay {
        score: f32,
    },
    Unknown {
        best: f32,
    },
}

pub fn identify(prints: &[Voiceprint], embedding: &[f32], t: &Thresholds) -> Match {
    let mut scored: Vec<(&Voiceprint, f32)> =
        prints.iter().map(|p| (p, p.score(embedding))).collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    let Some(&(best, score)) = scored.first() else {
        return Match::Unknown { best: f32::MIN };
    };
    let runner_up = scored.get(1).map_or(f32::MIN, |s| s.1);
    if best.closest_sample(embedding) >= t.replay {
        return Match::SuspectedReplay { score };
    }
    if score < t.reject {
        return Match::Unknown { best: score };
    }
    if score < t.accept || score - runner_up < t.margin {
        return Match::Ambiguous {
            best: score,
            runner_up,
        };
    }
    Match::Recognised {
        profile: best.profile.clone(),
        score,
        margin: score - runner_up,
    }
}

/// Does one window of a capture still sound like the person the capture was
/// admitted for?
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "result")]
pub enum Window {
    /// Consistent with the admitted person.
    Same { score: f32 },
    /// Clearly another enrolled person.
    OtherPerson {
        profile: String,
        score: f32,
        own: f32,
    },
    /// Clearly not the admitted person (an unenrolled voice, or noise that no
    /// longer resembles them).
    NotThisPerson { own: f32 },
}

/// Continuity rule for one window. `own` is the admitted person's centroid
/// score. Another enrolled person counts as heard when they clear `accept`
/// and beat `own` by `margin` (a handover). An own score under `floor` means
/// the window is not the admitted person (for example an unenrolled guest).
/// Simultaneous speech is mostly not detected: a mixture usually still
/// resembles one of the two speakers (see the speaker-identity evidence).
pub fn continuity(
    prints: &[Voiceprint],
    embedding: &[f32],
    profile: &str,
    t: &Thresholds,
    floor: f32,
) -> Window {
    let own = prints
        .iter()
        .find(|p| p.profile == profile)
        .map_or(f32::MIN, |p| p.score(embedding));
    let other = prints
        .iter()
        .filter(|p| p.profile != profile)
        .map(|p| (p, p.score(embedding)))
        .max_by(|a, b| a.1.total_cmp(&b.1));
    if let Some((p, score)) = other
        && score >= t.accept
        && score - own >= t.margin
    {
        return Window::OtherPerson {
            profile: p.profile.clone(),
            score,
            own,
        };
    }
    if own < floor {
        return Window::NotThisPerson { own };
    }
    Window::Same { score: own }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn unit(seed: u64, dim: usize) -> Vec<f32> {
        let mut x = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let v: Vec<f32> = (0..dim)
            .map(|_| {
                x = x
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                ((x >> 33) as f32 / (1u64 << 31) as f32) - 0.5
            })
            .collect();
        crate::embedding::normalise(v).unwrap()
    }
    fn near(base: &[f32], noise: u64, amount: f32) -> Vec<f32> {
        let n = unit(noise, base.len());
        crate::embedding::normalise(base.iter().zip(&n).map(|(b, e)| b + amount * e).collect())
            .unwrap()
    }
    fn print(id: &str, base: &[f32], seed: u64) -> Voiceprint {
        Voiceprint {
            profile: id.into(),
            samples: (0..4).map(|i| near(base, seed + i, 0.5)).collect(),
        }
    }
    // These vectors exercise decision logic only; they say nothing about
    // acoustic accuracy, which is measured with the real model separately.
    #[test]
    fn continuity_detects_handover_and_strangers_but_not_variation() {
        let t = Thresholds::default();
        let (a, b, c) = (unit(1, 64), unit(2, 64), unit(3, 64));
        let prints = [print("a", &a, 100), print("b", &b, 200)];
        assert!(matches!(
            continuity(&prints, &near(&a, 7, 0.5), "a", &t, 0.45),
            Window::Same { .. }
        ));
        assert!(
            matches!(continuity(&prints, &near(&b, 7, 0.5), "a", &t, 0.45), Window::OtherPerson { ref profile, .. } if profile == "b")
        );
        assert!(matches!(
            continuity(&prints, &c, "a", &t, 0.45),
            Window::NotThisPerson { .. }
        ));
        // No voiceprint for the admitted person: never "same".
        assert!(!matches!(
            continuity(&prints, &a, "zzz", &t, 0.45),
            Window::Same { .. }
        ));
    }
    #[test]
    fn decision_bands() {
        let t = Thresholds::default();
        let (a, b) = (unit(1, 64), unit(2, 64));
        let prints = [print("a", &a, 100), print("b", &b, 200)];
        assert!(
            matches!(identify(&prints, &near(&a, 9, 0.5), &t), Match::Recognised { ref profile, .. } if profile == "a")
        );
        assert!(matches!(
            identify(&prints, &unit(3, 64), &t),
            Match::Unknown { .. }
        ));
        let mid =
            crate::embedding::normalise(a.iter().zip(&b).map(|(x, y)| x + y).collect()).unwrap();
        assert!(matches!(
            identify(&prints, &mid, &t),
            Match::Ambiguous { .. }
        ));
        let replay = prints[0].samples[1].clone();
        assert!(matches!(
            identify(&prints, &replay, &t),
            Match::SuspectedReplay { .. }
        ));
        assert!(matches!(identify(&[], &a, &t), Match::Unknown { .. }));
    }
}
