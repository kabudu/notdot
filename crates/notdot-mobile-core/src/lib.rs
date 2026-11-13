//! Shared Rust core for the notdot iOS (SwiftUI) and Android (Compose)
//! apps, exported with UniFFI. Native code owns OS lifecycles, audio I/O
//! and drawing; this crate owns the rules both platforms must share.
//!
//! - [`orb`]: the canonical orb contract and generated shaders.
//! - [`session`]: the hands-free session lifecycle.
//! - [`wake`]: the desktop's on-device wake detector (keyword spotter, plus
//!   the Whisper backup with the `whisper` feature).
//! - [`link`]: pairing and the encrypted link to the desktop.
//! - [`stepup`]: owner step-up with a key in the phone's secure hardware.
uniffi::setup_scaffolding!();

#[cfg(feature = "whisper")]
mod backup;
#[cfg(feature = "wake")]
mod backup_types;
pub mod link;
pub mod orb;
pub mod session;
pub mod stepup;
#[cfg(feature = "wake")]
pub mod wake;

/// Errors reported to native code in plain language.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum MobileError {
    #[error("{detail}")]
    Invalid { detail: String },
    #[error("{detail}")]
    Link { detail: String },
    /// The desktop no longer accepts this phone (removed there, or its
    /// profile changed). The core has forgotten the pairing; native code
    /// should delete the stored identity, paired record and step-up key,
    /// and offer to pair again.
    #[error("{detail}")]
    Unpaired { detail: String },
    #[error("{detail}")]
    Wake { detail: String },
}
