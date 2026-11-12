//! Shared Rust core for the notdot iOS (SwiftUI) and Android (Compose)
//! apps, exported with UniFFI. Native code owns OS lifecycles, audio I/O
//! and drawing; this crate owns the rules both platforms must share.
//!
//! - [`orb`]: the canonical orb contract and generated shaders.
//! - [`session`]: the hands-free session lifecycle.
//! - [`wake`]: the desktop's on-device keyword spotter.
//! - [`link`]: pairing and the encrypted link to the desktop.
uniffi::setup_scaffolding!();

pub mod link;
pub mod orb;
pub mod session;
#[cfg(feature = "wake")]
pub mod wake;

/// Errors reported to native code in plain language.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum MobileError {
    #[error("{message}")]
    Invalid { message: String },
    #[error("{message}")]
    Link { message: String },
    #[error("{message}")]
    Wake { message: String },
}
