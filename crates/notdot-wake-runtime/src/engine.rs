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
        if segment.no_speech_probability() > 0.6 {
            return Ok(false);
        }
        text.push_str(segment.to_str().map_err(|e| e.to_string())?);
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
    let mut audio = if rate == 16000 {
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
    // Long digital-silence edges can change short-name transcription. Keep
    // every nonzero sample and 100 ms of context on each side, without an
    // energy threshold, gain change or removal of pauses inside the phrase.
    // Test bits so even subnormal speech samples are never classified as zero.
    let nonzero = |sample: &f32| sample.to_bits() & 0x7fff_ffff != 0;
    if let (Some(first), Some(last)) = (
        audio.iter().position(nonzero),
        audio.iter().rposition(nonzero),
    ) {
        let begin = first.saturating_sub(1600);
        let end = (last + 1601).min(audio.len());
        audio.drain(..begin);
        audio.truncate(end - begin);
    }
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

#[cfg(test)]
mod normalization_tests {
    use super::normalize;

    fn bits(samples: &[f32]) -> Vec<u32> {
        samples.iter().map(|sample| sample.to_bits()).collect()
    }

    #[test]
    fn exact_zero_edges_keep_all_nonzero_bits_and_one_hundred_ms_context() {
        let mut samples = vec![0.; 12000];
        samples[5000] = f32::from_bits(1); // Quietest nonzero prefix is retained.
        samples[6000] = -0.;
        samples[7000] = -0.1;
        let normalized = normalize(&samples, 16000).unwrap();
        assert_eq!(bits(&normalized), bits(&samples[3400..8601]));
        // App and optional helper both normalize; a second pass must not crop more.
        assert_eq!(
            bits(&normalize(&normalized, 16000).unwrap()),
            bits(&normalized)
        );
    }

    #[test]
    fn resampling_then_helper_normalization_is_bitwise_idempotent() {
        let mut samples = vec![0.; 48000];
        samples[18000..30000].fill(0.01);
        let first = normalize(&samples, 48000).unwrap();
        assert!(first.len() < 16000);
        assert_eq!(bits(&normalize(&first, 16000).unwrap()), bits(&first));
    }

    #[test]
    fn nonzero_edges_and_internal_pauses_are_preserved() {
        let mut samples = vec![0.; 10000];
        samples[0] = 0.01;
        samples[9999] = -0.01;
        assert_eq!(bits(&normalize(&samples, 16000).unwrap()), bits(&samples));
    }

    #[test]
    fn zero_only_and_short_inputs_are_not_erased_or_padded() {
        for samples in [
            vec![0.; 16000],
            vec![-0.; 100],
            vec![0.01],
            vec![0.01, 0., -0.02],
        ] {
            assert_eq!(bits(&normalize(&samples, 16000).unwrap()), bits(&samples));
        }
        assert!(normalize(&[], 16000).is_err());
    }

    #[test]
    fn nonfinite_and_oversized_input_stays_invalid_before_padding_removal() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            for rate in [16000, 48000] {
                assert!(normalize(&[0., value, 0.], rate).is_err());
            }
        }
        assert!(normalize(&vec![0.; 16000 * 6 + 513], 16000).is_err());
    }
}
