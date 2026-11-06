//! Bounded speaker identification. One inference at a time; callers run
//! [`Engine::identify`] on a blocking thread under a timeout and pass a
//! cancellation flag that is honoured before and after inference.
use crate::{
    audio::{self, SampleIssue},
    embedding::Extractor,
    matcher::{self, Match, Thresholds, Voiceprint},
};
use serde::Serialize;
use std::sync::{
    Arc, Mutex, RwLock,
    atomic::{AtomicBool, Ordering},
};
use zeroize::Zeroize;

/// Recommended timeout around one identification call.
pub const IDENTIFY_TIMEOUT_MS: u64 = 2_000;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "result")]
pub enum Identification {
    /// Nobody is enrolled.
    NoVoiceprints,
    Recognised {
        profile: String,
        score: f32,
        margin: f32,
    },
    Ambiguous,
    /// Possible verbatim replay of an enrolment recording; treat as ambiguous.
    SuspectedReplay,
    Unknown,
    Unusable {
        issue: SampleIssue,
    },
    Cancelled,
    /// Inference failed; locked modes must refuse.
    Unavailable,
}

pub struct Engine {
    extractor: Arc<dyn Extractor>,
    prints: RwLock<Arc<Vec<Voiceprint>>>,
    thresholds: Thresholds,
    busy: Mutex<()>,
}

impl Engine {
    pub fn new(
        extractor: Arc<dyn Extractor>,
        prints: Vec<Voiceprint>,
        thresholds: Thresholds,
    ) -> Self {
        Self {
            extractor,
            prints: RwLock::new(Arc::new(prints)),
            thresholds,
            busy: Mutex::new(()),
        }
    }
    pub fn extractor(&self) -> &dyn Extractor {
        self.extractor.as_ref()
    }
    /// Enrolment shares the same inference lock as recognition.
    pub fn enrol_sample(
        &self,
        enrolment: &mut crate::enrol::Enrolment,
        rate: u32,
        samples: &[f32],
    ) -> crate::enrol::Progress {
        let _guard = self.busy.lock().expect("speaker inference");
        enrolment.submit(self.extractor.as_ref(), rate, samples)
    }
    pub fn thresholds(&self) -> &Thresholds {
        &self.thresholds
    }
    pub fn prints(&self) -> Arc<Vec<Voiceprint>> {
        self.prints.read().expect("prints").clone()
    }
    /// Swap in a new set of voiceprints (after enrolment, removal or reset).
    pub fn replace_prints(&self, prints: Vec<Voiceprint>) {
        *self.prints.write().expect("prints") = Arc::new(prints);
    }

    /// Embed `samples` (mono f32 at `rate`, any length; bounded internally to
    /// the last few seconds of voiced audio).
    pub fn embed(
        &self,
        rate: u32,
        samples: &[f32],
        cancelled: &AtomicBool,
    ) -> Result<Vec<f32>, Identification> {
        let (mut voiced, _) =
            audio::voiced(rate, samples).map_err(|issue| Identification::Unusable { issue })?;
        let _one_at_a_time = self.busy.lock().map_err(|_| Identification::Unavailable)?;
        if cancelled.load(Ordering::Acquire) {
            voiced.zeroize();
            return Err(Identification::Cancelled);
        }
        let e = self.extractor.embed(rate, &voiced);
        voiced.zeroize();
        if cancelled.load(Ordering::Acquire) {
            return Err(Identification::Cancelled);
        }
        e.map_err(|_| Identification::Unusable {
            issue: SampleIssue::NotAssessable,
        })
    }

    pub fn identify(&self, rate: u32, samples: &[f32], cancelled: &AtomicBool) -> Identification {
        let prints = self.prints();
        if prints.is_empty() {
            return Identification::NoVoiceprints;
        }
        let mut e = match self.embed(rate, samples, cancelled) {
            Ok(e) => e,
            Err(outcome) => return outcome,
        };
        let result = match matcher::identify(&prints, &e, &self.thresholds) {
            Match::Recognised {
                profile,
                score,
                margin,
            } => Identification::Recognised {
                profile,
                score,
                margin,
            },
            Match::Ambiguous { .. } => Identification::Ambiguous,
            Match::SuspectedReplay { .. } => Identification::SuspectedReplay,
            Match::Unknown { .. } => Identification::Unknown,
        };
        e.zeroize();
        result
    }
}

/// Window length for continuity checks.
pub const CONTINUITY_WINDOW_SECONDS: f32 = 3.0;
/// Bound on windows checked; longer captures widen the hop instead.
pub const MAX_CONTINUITY_WINDOWS: usize = 40;
/// Lowest own-voice score per window still treated as the admitted person.
pub const CONTINUITY_FLOOR: f32 = 0.45;

/// Whether a whole capture was spoken by the person it was admitted for.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "result")]
pub enum Continuity {
    /// Every assessable window matched; `checked` may be 0 for very short
    /// captures, which are then covered only by the admission check.
    Consistent {
        checked: usize,
    },
    /// Another enrolled person, or a voice that is clearly not the admitted
    /// person, was heard in window `window` (0-based).
    Switched {
        window: usize,
        other: Option<String>,
    },
    Cancelled,
    Unavailable,
}

impl Engine {
    /// Check every window of a capture against the admitted `profile`.
    /// Windows without enough clear speech are skipped. `floor` is the lowest
    /// own-voice score still treated as the same person.
    pub fn continuity(
        &self,
        rate: u32,
        samples: &[f32],
        profile: &str,
        floor: f32,
        cancelled: &AtomicBool,
    ) -> Continuity {
        let prints = self.prints();
        if !prints.iter().any(|p| p.profile == profile) {
            return Continuity::Unavailable;
        }
        let mut checked = 0;
        for (i, window) in audio::windows(
            rate,
            samples,
            CONTINUITY_WINDOW_SECONDS,
            MAX_CONTINUITY_WINDOWS,
        )
        .into_iter()
        .enumerate()
        {
            let mut e = match self.embed(rate, window, cancelled) {
                Ok(e) => e,
                Err(Identification::Unusable { .. }) => continue,
                Err(Identification::Cancelled) => return Continuity::Cancelled,
                Err(_) => return Continuity::Unavailable,
            };
            let verdict = matcher::continuity(&prints, &e, profile, &self.thresholds, floor);
            e.zeroize();
            checked += 1;
            match verdict {
                matcher::Window::Same { .. } => {}
                matcher::Window::OtherPerson { profile, .. } => {
                    return Continuity::Switched {
                        window: i,
                        other: Some(profile),
                    };
                }
                matcher::Window::NotThisPerson { .. } => {
                    return Continuity::Switched {
                        window: i,
                        other: None,
                    };
                }
            }
        }
        Continuity::Consistent { checked }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixed(Vec<f32>);
    impl Extractor for Fixed {
        fn embed(&self, _: u32, _: &[f32]) -> Result<Vec<f32>, String> {
            Ok(self.0.clone())
        }
        fn dim(&self) -> usize {
            self.0.len()
        }
        fn model_id(&self) -> [u8; 32] {
            [0; 32]
        }
    }
    fn speech() -> Vec<f32> {
        (0..16_000 * 2)
            .map(|i| 0.3 * (i as f32 * 0.05).sin())
            .collect()
    }
    #[test]
    fn identification_paths() {
        let e = vec![1.0, 0.0, 0.0];
        let engine = Engine::new(Arc::new(Fixed(e.clone())), vec![], Thresholds::default());
        let go = AtomicBool::new(false);
        assert_eq!(
            engine.identify(16_000, &speech(), &go),
            Identification::NoVoiceprints
        );
        let close = crate::embedding::normalise(vec![1.0, 0.3, 0.0]).unwrap();
        let close2 = crate::embedding::normalise(vec![1.0, -0.3, 0.0]).unwrap();
        engine.replace_prints(vec![Voiceprint {
            profile: "p".into(),
            samples: vec![close.clone(), close2.clone(), close],
        }]);
        assert!(
            matches!(engine.identify(16_000, &speech(), &go), Identification::Recognised { ref profile, .. } if profile == "p")
        );
        assert_eq!(
            engine.identify(16_000, &[0.0; 100], &go),
            Identification::Unusable {
                issue: SampleIssue::TooShort
            }
        );
        let stop = AtomicBool::new(true);
        assert_eq!(
            engine.identify(16_000, &speech(), &stop),
            Identification::Cancelled
        );
        engine.replace_prints(vec![Voiceprint {
            profile: "p".into(),
            samples: vec![e.clone(), close2.clone(), close2],
        }]);
        assert_eq!(
            engine.identify(16_000, &speech(), &go),
            Identification::SuspectedReplay
        );
    }
}
