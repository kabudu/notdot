//! notdot's own host-control drivers.
//!
//! OpenAI's Browser Use and Computer Use are not available to notdot's Codex
//! runtime (they accept only the ChatGPT desktop app as host). These drivers
//! give notdot a separate, scoped capability that it owns end to end:
//!
//! - [`browser`]: a Chromium-family browser that notdot launches with a fresh,
//!   disposable profile and controls over a private DevTools pipe (no port).
//! - [`desktop`]: the platform accessibility API (macOS AX today).
//!
//! Policy (who may use them, which site or app, and which actions need a
//! confirmation) lives in `notdot-core`; this crate only drives the host.
#![deny(unsafe_code)]

pub mod browser;
#[cfg(unix)]
mod cdp;
pub mod desktop;

use serde::{Deserialize, Serialize};

/// Whether a capability can run now. Shown separately from the owner's saved
/// preference so Settings never claims a capability notdot does not have.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum Status {
    Ready,
    /// A one-time step the person must take (for example an OS permission).
    SetupRequired(String),
    /// Not implemented on this platform.
    Unsupported(String),
}
impl Status {
    pub fn ready(&self) -> bool {
        matches!(self, Self::Ready)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Readiness {
    pub browser: Status,
    pub desktop: Status,
}

/// Current readiness of both capabilities. Cheap: a file-existence check and,
/// on macOS, one `AXIsProcessTrusted` call. It never prompts.
pub fn readiness() -> Readiness {
    Readiness {
        browser: browser::readiness(),
        desktop: desktop::readiness(),
    }
}

/// Labels whose action can spend money, send, publish, delete, sign in or
/// otherwise be hard to undo. A match always needs its own confirmation, even
/// inside a site or app the person already allowed.
pub fn sensitive_label(label: &str) -> bool {
    const WORDS: &[&str] = &[
        "buy",
        "pay",
        "purchase",
        "checkout",
        "check out",
        "order",
        "delete",
        "remove",
        "erase",
        "send",
        "submit",
        "transfer",
        "confirm",
        "sign in",
        "signin",
        "log in",
        "login",
        "sign up",
        "signup",
        "register",
        "subscribe",
        "publish",
        "post",
        "share",
        "install",
        "download",
        "upload",
        "accept",
        "agree",
        "allow",
        "authorize",
        "authorise",
        "password",
        "unlock",
        "quit",
        "close",
        "discard",
        "empty",
        "reset",
        "format",
    ];
    let lower = label.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let joined = words.join(" ");
    WORDS.iter().any(|w| {
        if w.contains(' ') {
            joined.contains(w)
        } else {
            words.iter().any(|x| x == w)
        }
    })
}

/// Make untrusted page or app text safe to quote: no control characters, one
/// line, bounded length (in characters, never splitting a code point).
pub fn clean(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut space = false;
    for c in text.chars() {
        let c = if c.is_control() || c.is_whitespace() {
            ' '
        } else {
            c
        };
        if c == ' ' {
            if space || out.is_empty() {
                continue;
            }
            space = true;
        } else {
            space = false;
        }
        if out.chars().count() >= max {
            out.push('…');
            break;
        }
        out.push(c);
    }
    out.trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sensitive_labels_match_whole_words_only() {
        for label in [
            "Pay now",
            "Delete account",
            "Send",
            "Sign in",
            "Place order",
            "SUBMIT",
            "Check out",
        ] {
            assert!(sensitive_label(label), "{label}");
        }
        for label in [
            "Increment",
            "Show details",
            "Next page",
            "Payload viewer",
            "Postcode",
            "Ordered list",
            "Resend-free counter",
        ] {
            assert!(!sensitive_label(label), "{label}");
        }
    }
    #[test]
    fn untrusted_text_is_one_bounded_line() {
        assert_eq!(clean("  a\n\nb\tc\u{7}d  ", 100), "a b c d");
        assert_eq!(clean("éééé", 2), "éé…");
        assert_eq!(clean("", 5), "");
    }
}
