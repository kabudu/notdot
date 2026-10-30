//! Shared full-context decode rules for the generic app and optional helper.
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext};
#[cfg(feature = "synthetic-diagnostics")]
pub(crate) fn synthetic_diagnostics_enabled() -> bool {
    std::env::var("NOTDOT_SYNTHETIC_WAKE_DIAGNOSTICS").as_deref() == Ok("synthetic-ci")
}

#[derive(Clone, Copy)]
pub struct DecodeLimits {
    pub budget: Duration,
    pub threads: i32,
}
impl Default for DecodeLimits {
    fn default() -> Self {
        Self {
            budget: Duration::from_secs(3),
            threads: 2,
        }
    }
}
pub fn recognize(
    context: &WhisperContext,
    phrase: &str,
    samples: &[f32],
    rate: u32,
    epoch: Arc<AtomicU64>,
    expected: u64,
    limits: DecodeLimits,
) -> Result<bool, String> {
    whisper_rs::install_logging_hooks();
    if epoch.load(Ordering::Relaxed) != expected {
        return Ok(false);
    }
    let audio = normalize(samples, rate)?;
    #[cfg(feature = "synthetic-diagnostics")]
    if synthetic_diagnostics_enabled() {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        for sample in &audio {
            hash.update(sample.to_le_bytes());
        }
        eprintln!(
            "synthetic_wake_input pid={} rate={rate} normalized_count={} sha256={:x}",
            std::process::id(),
            audio.len(),
            hash.finalize()
        );
    }
    let mut state = context.create_state().map_err(|e| e.to_string())?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_n_threads(limits.threads);
    params.set_max_tokens(32);
    params.set_temperature_inc(0.);
    params.set_suppress_nst(true);
    params.set_language(Some("en"));
    params.set_translate(false);
    params.set_no_context(true);
    params.set_no_timestamps(true);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    let prompt = name_prompt(phrase);
    params.set_initial_prompt(&prompt);
    let deadline = Instant::now() + limits.budget;
    let timed_out = Arc::new(AtomicBool::new(false));
    let abort_timeout = timed_out.clone();
    let cancelled = epoch.clone();
    let abort: Box<dyn FnMut() -> bool> = Box::new(move || {
        if cancelled.load(Ordering::Relaxed) != expected {
            return true;
        }
        let expired = Instant::now() > deadline;
        if expired {
            abort_timeout.store(true, Ordering::Relaxed);
        }
        expired
    });
    params.set_abort_callback_safe::<_, Box<dyn FnMut() -> bool>>(Some(abort));
    if let Err(e) = state.full(params, &audio) {
        return if epoch.load(Ordering::Relaxed) != expected {
            Ok(false)
        } else if timed_out.load(Ordering::Relaxed) {
            Err("Local wake recognition exceeded its time limit".into())
        } else {
            Err(format!("Local wake recognition could not finish: {e}"))
        };
    }
    let mut text = String::new();
    for segment in state.as_iter() {
        #[cfg(feature = "synthetic-diagnostics")]
        if synthetic_diagnostics_enabled() {
            let bounded = segment
                .to_str()
                .unwrap_or("<invalid UTF-8>")
                .chars()
                .take(256)
                .collect::<String>();
            eprintln!(
                "synthetic_wake_segment pid={} no_speech={} text={bounded:?} addressed={} rejected_no_speech={}",
                std::process::id(),
                segment.no_speech_probability(),
                addressed(&bounded, phrase),
                segment.no_speech_probability() > 0.6
            );
        }
        if segment.no_speech_probability() > 0.6 {
            return Ok(false);
        }
        text.push_str(segment.to_str().map_err(|e| e.to_string())?);
    }
    #[cfg(feature = "synthetic-diagnostics")]
    if synthetic_diagnostics_enabled() {
        eprintln!(
            "synthetic_wake_result pid={} addressed={}",
            std::process::id(),
            addressed(&text, phrase)
        );
    }
    Ok(addressed(&text, phrase))
}
// Give the recognizer a proper-name hint even when the configured name begins
// lowercase. Preserve deliberate internal casing and punctuation; matching below
// continues to use the original phrase and its existing strict address guard.
fn name_prompt(phrase: &str) -> String {
    phrase
        .strip_prefix("Hey ")
        .unwrap_or(phrase)
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                format!("{}{}", first.to_ascii_uppercase(), chars.as_str())
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn addressed(text: &str, phrase: &str) -> bool {
    let words = |s: &str| {
        s.split(|c: char| !c.is_ascii_alphabetic())
            .filter(|s| !s.is_empty())
            .map(str::to_ascii_lowercase)
            .collect::<Vec<_>>()
    };
    let expected = words(phrase);
    let actual = words(text);
    !expected.is_empty() && actual.starts_with(&expected)
}

pub fn normalize(samples: &[f32], rate: u32) -> Result<Vec<f32>, String> {
    if !(8000..=192000).contains(&rate)
        || samples.is_empty()
        || samples.len() > rate as usize * 6 + 512
        || samples.iter().any(|s| !s.is_finite())
    {
        return Err("Invalid wake audio".into());
    }
    let audio = if rate == 16000 {
        samples.to_vec()
    } else {
        let params = SincInterpolationParameters {
            sinc_len: 128,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 128,
            window: WindowFunction::BlackmanHarris2,
        };
        let mut resampler =
            SincFixedIn::<f32>::new(16000. / f64::from(rate), 1., params, samples.len(), 1)
                .map_err(|e| e.to_string())?;
        resampler
            .process(&[samples], None)
            .map_err(|e| e.to_string())?
            .remove(0)
    };
    Ok(audio)
}

#[cfg(test)]
mod prompt_tests {
    use super::*;
    #[test]
    fn lower_and_title_case_names_use_the_same_proper_name_prompt() {
        for phrase in ["ava", "Ava", "Hey ava", "Hey Ava"] {
            assert_eq!(name_prompt(phrase), "Ava");
        }
        assert_eq!(name_prompt("Hey ava marie"), name_prompt("Hey Ava Marie"));
    }
    #[test]
    fn prompt_preserves_hyphens_apostrophes_and_deliberate_internal_casing() {
        assert_eq!(name_prompt("Hey mary-jane o'brien"), "Mary-jane O'brien");
        assert_eq!(name_prompt("Hey Jean-Luc O'Neill"), "Jean-Luc O'Neill");
        assert_eq!(name_prompt("Hey McDonald NASA"), "McDonald NASA");
    }
    #[test]
    fn name_prompt_never_relaxes_the_address_guard() {
        assert!(!addressed("Ava", "Hey ava"));
        assert!(!addressed("Hey Nora", "Hey ava"));
        assert!(!addressed("I was talking about Hey Ava", "Hey ava"));
        assert!(addressed("HEY AVA", "Hey ava"));
    }
}
