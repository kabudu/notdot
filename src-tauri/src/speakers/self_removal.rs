//! Self-service offboarding for a trusted member.
//!
//! Voice matching alone is not strong enough to delete someone's data: a
//! recording of their voice could be replayed. Removing your own profile
//! therefore needs three things, each bound to the same profile and its
//! current epoch:
//!
//! 1. a spoken request ("remove my profile") in a capture admitted as that
//!    member (never the owner and never an open-mode fallback);
//! 2. a fresh one-time code, spoken back in a *different* capture admitted as
//!    the same member within two minutes. A pre-made recording cannot contain
//!    a code that did not exist when it was made;
//! 3. a click in notdot's own visible, focused window on this computer, with
//!    the member's name typed to confirm, within two minutes of step 2.
//!
//! A real-time voice clone of the member plus physical access to the
//! computer would still pass. That is the documented limit.
use notdot_core::profiles::{ProfileId, TurnLock};
use std::time::{Duration, Instant};

pub const CHALLENGE_TTL: Duration = Duration::from_secs(120);
pub const MAX_CODE_ATTEMPTS: u8 = 3;
const CODE_DIGITS: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stage {
    AwaitingCode { attempts: u8 },
    AwaitingLocalConfirmation,
}

#[derive(Clone, Debug)]
pub struct Challenge {
    pub profile: ProfileId,
    pub epoch: u64,
    pub request_token: String,
    code: String,
    pub stage: Stage,
    pub since: Instant,
}

/// Result of offering a transcript to a pending challenge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// Not part of the removal flow; handle the request normally.
    NotForUs,
    /// Code accepted; the local confirmation step is now open.
    Confirmed,
    WrongCode {
        remaining: u8,
    },
    /// Too many wrong codes, or the code expired. The challenge is gone.
    Ended,
}

impl Challenge {
    pub fn new(lock: &TurnLock, code: String, now: Instant) -> Self {
        Self {
            profile: lock.profile.clone(),
            epoch: lock.epoch,
            request_token: lock.capture_token.clone(),
            code,
            stage: Stage::AwaitingCode { attempts: 0 },
            since: now,
        }
    }
    pub fn expired(&self, now: Instant) -> bool {
        now.duration_since(self.since) >= CHALLENGE_TTL
    }
    /// The code read aloud with pauses, for example "4, 7, 2, 9".
    pub fn spoken_code(&self) -> String {
        self.code
            .chars()
            .map(String::from)
            .collect::<Vec<_>>()
            .join(", ")
    }
    pub fn expires_in(&self, now: Instant) -> Duration {
        CHALLENGE_TTL.saturating_sub(now.duration_since(self.since))
    }

    /// Offer the transcript of a new capture admitted as `lock`.
    pub fn answer(&mut self, lock: &TurnLock, transcript: &str, now: Instant) -> Answer {
        if lock.profile != self.profile
            || lock.capture_token == self.request_token
            || lock.open_fallback
            || !matches!(self.stage, Stage::AwaitingCode { .. })
        {
            return Answer::NotForUs;
        }
        // Ordinary speech ("I need to go for a walk") must not count as a
        // wrong code: only an explicit "code"/"remove me" or a run of at least
        // four unambiguous digits is treated as an answer.
        let about_removal =
            mentions_code(transcript) || spoken_digits(transcript, false).len() >= CODE_DIGITS;
        if !about_removal {
            return Answer::NotForUs;
        }
        let digits = spoken_digits(transcript, true);
        if self.expired(now) || lock.epoch != self.epoch {
            return Answer::Ended;
        }
        if digits.contains(&self.code) {
            self.stage = Stage::AwaitingLocalConfirmation;
            self.since = now;
            return Answer::Confirmed;
        }
        let attempts = self.attempts() + 1;
        if attempts >= MAX_CODE_ATTEMPTS {
            return Answer::Ended;
        }
        self.stage = Stage::AwaitingCode { attempts };
        Answer::WrongCode {
            remaining: MAX_CODE_ATTEMPTS - attempts,
        }
    }
    fn attempts(&self) -> u8 {
        match self.stage {
            Stage::AwaitingCode { attempts } => attempts,
            Stage::AwaitingLocalConfirmation => MAX_CODE_ATTEMPTS,
        }
    }
}

/// A fresh random code. UUIDv4 bytes come from the OS generator. The two
/// bytes carrying the version and variant bits are skipped, and bytes of 250
/// or more are rejected, so every digit is equally likely.
pub fn random_code() -> String {
    let mut out = String::new();
    while out.len() < CODE_DIGITS {
        for (i, b) in uuid::Uuid::new_v4().into_bytes().into_iter().enumerate() {
            if i != 6 && i != 8 && b < 250 && out.len() < CODE_DIGITS {
                out.push(char::from(b'0' + b % 10));
            }
        }
    }
    out
}

fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|w| !w.is_empty())
        .map(|w| w.trim_matches('\'').to_owned())
        .collect()
}

/// Digits spoken in a transcript, whether recognised as numerals ("4729",
/// "4 7 2 9") or words ("four seven two nine", "oh" for zero). `lenient`
/// also accepts common recognition homophones ("to", "for").
pub fn spoken_digits(text: &str, lenient: bool) -> String {
    let mut out = String::new();
    for w in words(text) {
        let digit = match w.as_str() {
            "zero" | "oh" => Some('0'),
            "one" => Some('1'),
            "two" => Some('2'),
            "to" | "too" if lenient => Some('2'),
            "three" => Some('3'),
            "four" => Some('4'),
            "for" if lenient => Some('4'),
            "five" => Some('5'),
            "six" => Some('6'),
            "seven" => Some('7'),
            "eight" => Some('8'),
            "nine" => Some('9'),
            _ => None,
        };
        match digit {
            Some(d) => out.push(d),
            None if w.chars().all(|c| c.is_ascii_digit()) => out.push_str(&w),
            None => {}
        }
    }
    out
}

fn mentions_code(text: &str) -> bool {
    let w = words(text);
    w.iter().any(|w| w == "code") || w.windows(2).any(|p| p[0] == "remove" && p[1] == "me")
}

/// Is this a member asking to remove their own profile? Deliberately narrow:
/// it must name the speaker's own profile, account or data, or say "forget
/// me". Mentions of other people never match.
pub fn is_request(text: &str) -> bool {
    let w = words(text);
    let joined = format!(" {} ", w.join(" "));
    let verbs = ["remove", "delete", "erase"];
    let objects = [
        "my profile",
        "my account",
        "my data",
        "my voice profile",
        "me from notdot",
    ];
    let asks = verbs.iter().any(|v| {
        objects.iter().any(|o| {
            joined.contains(&format!(" {v} {o} ")) || joined.contains(&format!(" {v} all {o} "))
        })
    }) || joined.contains(" forget me ")
        || joined.contains(" forget about me ");
    let negated = [" don't ", " do not ", " never ", " not "]
        .iter()
        .any(|n| joined.contains(n));
    asks && !negated
}

#[cfg(test)]
mod tests {
    use super::*;
    use notdot_core::profiles::Role;

    fn lock(profile: &ProfileId, token: &str, epoch: u64) -> TurnLock {
        TurnLock {
            capture_token: token.into(),
            profile: profile.clone(),
            role: Role::Member,
            epoch,
            open_fallback: false,
        }
    }

    #[test]
    fn requests_are_narrow() {
        for yes in [
            "Remove my profile",
            "please delete my data from notdot",
            "notdot, forget me.",
            "Erase my account",
            "remove me from notdot please",
        ] {
            assert!(is_request(yes), "{yes}");
        }
        for no in [
            "remove Ada's profile",
            "don't delete my profile",
            "remove my reminder",
            "what does forget mean",
            "delete my last message",
            "remind me to remove the profile picture",
        ] {
            assert!(!is_request(no), "{no}");
        }
    }

    #[test]
    fn digits_from_words_or_numerals() {
        assert_eq!(
            spoken_digits("Remove me, code four seven two nine.", false),
            "4729"
        );
        assert_eq!(spoken_digits("code 4729", false), "4729");
        assert_eq!(spoken_digits("4, 7, 2, 9", false), "4729");
        assert_eq!(spoken_digits("oh one two three", false), "0123");
        assert_eq!(spoken_digits("hello there", false), "");
        assert_eq!(spoken_digits("code for seven to nine", true), "4729");
        assert_eq!(spoken_digits("I need to go for a walk", false), "");
        let c = random_code();
        assert_eq!(c.len(), CODE_DIGITS);
        assert!(c.chars().all(|d| d.is_ascii_digit()));
    }

    #[test]
    fn code_must_come_from_a_new_capture_by_the_same_member_in_time() {
        let ada = ProfileId::random();
        let grace = ProfileId::random();
        let t0 = Instant::now();
        let mut c = Challenge::new(&lock(&ada, "req", 3), "4729".into(), t0);
        assert_eq!(c.spoken_code(), "4, 7, 2, 9");
        // The request capture itself (a replay of the same recording) never counts.
        assert_eq!(
            c.answer(&lock(&ada, "req", 3), "4729", t0),
            Answer::NotForUs
        );
        // Another member, or an open-mode fallback, is not part of this flow.
        assert_eq!(
            c.answer(&lock(&grace, "g", 0), "code 4729", t0),
            Answer::NotForUs
        );
        let mut fallback = lock(&ada, "f", 3);
        fallback.open_fallback = true;
        assert_eq!(c.answer(&fallback, "4729", t0), Answer::NotForUs);
        // Unrelated speech passes through untouched.
        assert_eq!(
            c.answer(&lock(&ada, "a1", 3), "what's the weather", t0),
            Answer::NotForUs
        );
        assert_eq!(
            c.answer(&lock(&ada, "a1b", 3), "I need to go for a walk at 5", t0),
            Answer::NotForUs
        );
        assert_eq!(
            c.answer(&lock(&ada, "a2", 3), "code 1111", t0),
            Answer::WrongCode { remaining: 2 }
        );
        assert_eq!(
            c.answer(
                &lock(&ada, "a3", 3),
                "remove me code four seven two nine",
                t0
            ),
            Answer::Confirmed
        );
        assert_eq!(c.stage, Stage::AwaitingLocalConfirmation);
        // Further speech cannot re-trigger or alter a confirmed challenge.
        assert_eq!(c.answer(&lock(&ada, "a4", 3), "4729", t0), Answer::NotForUs);
    }

    #[test]
    fn expiry_epoch_change_and_too_many_attempts_end_the_challenge() {
        let ada = ProfileId::random();
        let t0 = Instant::now();
        let mut c = Challenge::new(&lock(&ada, "req", 3), "4729".into(), t0);
        assert_eq!(
            c.answer(&lock(&ada, "a", 3), "4729", t0 + CHALLENGE_TTL),
            Answer::Ended
        );
        let mut c = Challenge::new(&lock(&ada, "req", 3), "4729".into(), t0);
        assert_eq!(c.answer(&lock(&ada, "a", 4), "4729", t0), Answer::Ended);
        let mut c = Challenge::new(&lock(&ada, "req", 3), "4729".into(), t0);
        assert!(matches!(
            c.answer(&lock(&ada, "a", 3), "0000", t0),
            Answer::WrongCode { .. }
        ));
        assert!(matches!(
            c.answer(&lock(&ada, "b", 3), "0000", t0),
            Answer::WrongCode { .. }
        ));
        assert_eq!(c.answer(&lock(&ada, "c", 3), "0000", t0), Answer::Ended);
    }
}
