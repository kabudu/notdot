//! Guided enrolment: four short phrases, the first doubling as spoken consent.
//! Weak samples get a friendly retry; cancelling drops everything. No audio is
//! kept: each sample is reduced to an embedding immediately.
use crate::{
    audio::{self, SampleIssue},
    embedding::Extractor,
    matcher::{self, Thresholds, Voiceprint, cosine},
};
use serde::Serialize;
use zeroize::Zeroize;

/// Short, natural phrases with varied sounds. The first is the consent line.
pub const PROMPTS: [&str; 4] = [
    "notdot, this is my voice, and I'm happy for you to recognise it.",
    "The quick brown fox jumps over the lazy dog.",
    "Could you remind me to water the plants tomorrow morning?",
    "What's the weather looking like this weekend?",
];
pub const RETRIES_PER_PROMPT: u8 = 2;
/// Each new sample must resemble the earlier ones at least this much. Lower
/// than the recognition threshold because single phrases vary.
pub const CONSISTENCY: f32 = 0.50;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "step")]
pub enum Progress {
    /// Ask the person to say `phrase`. `index` counts from 1.
    Prompt {
        index: usize,
        of: usize,
        phrase: &'static str,
    },
    Retry {
        index: usize,
        of: usize,
        phrase: &'static str,
        issue: SampleIssue,
        guidance: &'static str,
    },
    /// All samples collected; call [`Enrolment::finish`].
    Ready { of: usize },
    /// Too many weak samples for one phrase. Nothing was saved.
    Failed {
        issue: SampleIssue,
        guidance: &'static str,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "reason")]
pub enum EnrolError {
    Incomplete,
    /// The new voice is very close to someone already enrolled, so the two
    /// could not be told apart reliably.
    TooSimilar {
        profile: String,
        score: f32,
    },
}

pub struct Enrolment {
    profile: String,
    samples: Vec<Vec<f32>>,
    retries: u8,
    failed: bool,
}
impl Drop for Enrolment {
    fn drop(&mut self) {
        for s in &mut self.samples {
            s.zeroize();
        }
    }
}

impl Enrolment {
    pub fn start(profile: &str) -> (Self, Progress) {
        let e = Self {
            profile: profile.to_owned(),
            samples: Vec::with_capacity(PROMPTS.len()),
            retries: 0,
            failed: false,
        };
        let p = e.current();
        (e, p)
    }
    pub fn profile(&self) -> &str {
        &self.profile
    }
    pub fn current(&self) -> Progress {
        if self.samples.len() == PROMPTS.len() {
            return Progress::Ready { of: PROMPTS.len() };
        }
        Progress::Prompt {
            index: self.samples.len() + 1,
            of: PROMPTS.len(),
            phrase: PROMPTS[self.samples.len()],
        }
    }

    /// Submit the audio recorded for the current phrase. The caller zeroises
    /// its own copy of the audio afterwards.
    pub fn submit(&mut self, extractor: &dyn Extractor, rate: u32, samples: &[f32]) -> Progress {
        if self.failed {
            return Progress::Failed {
                issue: SampleIssue::NotAssessable,
                guidance: "Let's start again from the beginning.",
            };
        }
        if self.samples.len() == PROMPTS.len() {
            return self.current();
        }
        let result = audio::voiced(rate, samples).and_then(|(mut voiced, _)| {
            let e = extractor
                .embed(rate, &voiced)
                .map_err(|_| SampleIssue::NotAssessable);
            voiced.zeroize();
            let e = e?;
            if !self.samples.is_empty()
                && cosine(&matcher::centroid(&self.samples), &e) < CONSISTENCY
            {
                return Err(SampleIssue::Inconsistent);
            }
            Ok(e)
        });
        match result {
            Ok(e) => {
                self.samples.push(e);
                self.retries = 0;
                self.current()
            }
            Err(issue) if self.retries < RETRIES_PER_PROMPT => {
                self.retries += 1;
                Progress::Retry {
                    index: self.samples.len() + 1,
                    of: PROMPTS.len(),
                    phrase: PROMPTS[self.samples.len()],
                    issue,
                    guidance: issue.guidance(),
                }
            }
            Err(issue) => {
                self.failed = true;
                Progress::Failed {
                    issue,
                    guidance: "That didn't quite work. We can try again whenever you like, perhaps somewhere quieter.",
                }
            }
        }
    }

    /// Produce the voiceprint. `others` are everyone else's existing prints
    /// (the same profile's old print is ignored, so re-enrolment works).
    pub fn finish(
        mut self,
        others: &[Voiceprint],
        thresholds: &Thresholds,
    ) -> Result<Voiceprint, EnrolError> {
        if self.failed || self.samples.len() < PROMPTS.len() {
            return Err(EnrolError::Incomplete);
        }
        let c = matcher::centroid(&self.samples);
        for other in others.iter().filter(|o| o.profile != self.profile) {
            let score = other.score(&c);
            if score >= thresholds.accept {
                return Err(EnrolError::TooSimilar {
                    profile: other.profile.clone(),
                    score,
                });
            }
        }
        Ok(Voiceprint {
            profile: std::mem::take(&mut self.profile),
            samples: std::mem::take(&mut self.samples),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Fake extractor: the "voice" is the sign pattern of the first samples.
    /// Exercises the state machine only, not acoustics.
    struct Fake;
    impl Extractor for Fake {
        fn embed(&self, _: u32, v: &[f32]) -> Result<Vec<f32>, String> {
            let mut e: Vec<f32> = (0..8)
                .map(|i| if v[i * 97] > 0.0 { 1.0 } else { -1.0 })
                .collect();
            e[0] += v.len() as f32 * 1e-7;
            crate::embedding::normalise(e)
        }
        fn dim(&self) -> usize {
            8
        }
        fn model_id(&self) -> [u8; 32] {
            [0; 32]
        }
    }
    fn speech(phase: f32) -> Vec<f32> {
        (0..16_000 * 3)
            .map(|i| 0.3 * ((i as f32) * 0.07 + phase).sin())
            .collect()
    }
    #[test]
    fn guided_flow_with_retries_and_completion() {
        let (mut e, p) = Enrolment::start("p1");
        assert!(matches!(
            p,
            Progress::Prompt {
                index: 1,
                of: 4,
                ..
            }
        ));
        // Weak sample → retry with guidance, same phrase.
        let p = e.submit(&Fake, 16_000, &vec![0.0; 16_000]);
        assert!(matches!(
            p,
            Progress::Retry {
                index: 1,
                issue: SampleIssue::TooShort,
                ..
            }
        ));
        for i in 0..4 {
            let p = e.submit(&Fake, 16_000, &speech(0.0));
            if i < 3 {
                assert!(
                    matches!(p, Progress::Prompt { index, .. } if index == i + 2),
                    "{p:?}"
                );
            } else {
                assert_eq!(p, Progress::Ready { of: 4 });
            }
        }
        let print = e.finish(&[], &Thresholds::default()).unwrap();
        assert_eq!(print.samples.len(), 4);
        assert_eq!(print.profile, "p1");
    }
    #[test]
    fn repeated_weak_samples_fail_and_incomplete_cannot_finish() {
        let (mut e, _) = Enrolment::start("p1");
        // Above the silence floor but below the minimum speech level.
        let quiet = vec![0.01; 16_000 * 3];
        assert!(matches!(
            e.submit(&Fake, 16_000, &quiet),
            Progress::Retry { .. }
        ));
        assert!(matches!(
            e.submit(&Fake, 16_000, &quiet),
            Progress::Retry { .. }
        ));
        assert!(matches!(
            e.submit(&Fake, 16_000, &quiet),
            Progress::Failed {
                issue: SampleIssue::TooQuiet,
                ..
            }
        ));
        assert!(matches!(
            e.submit(&Fake, 16_000, &speech(0.0)),
            Progress::Failed { .. }
        ));
        assert_eq!(
            e.finish(&[], &Thresholds::default()).unwrap_err(),
            EnrolError::Incomplete
        );
        let (e, _) = Enrolment::start("p2");
        assert_eq!(
            e.finish(&[], &Thresholds::default()).unwrap_err(),
            EnrolError::Incomplete
        );
    }
    #[test]
    fn inconsistent_sample_is_retried_and_duplicates_are_refused() {
        let (mut e, _) = Enrolment::start("p1");
        e.submit(&Fake, 16_000, &speech(0.0));
        let different: Vec<f32> = speech(0.0).iter().map(|s| -s).collect();
        assert!(matches!(
            e.submit(&Fake, 16_000, &different),
            Progress::Retry {
                issue: SampleIssue::Inconsistent,
                ..
            }
        ));
        for _ in 0..3 {
            e.submit(&Fake, 16_000, &speech(0.0));
        }
        let existing = Voiceprint {
            profile: "someone-else".into(),
            samples: vec![Fake.embed(16_000, &speech(0.0)).unwrap(); 3],
        };
        assert!(matches!(
            e.finish(&[existing], &Thresholds::default()),
            Err(EnrolError::TooSimilar { .. })
        ));
    }
}
