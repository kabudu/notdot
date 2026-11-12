#![forbid(unsafe_code)]
//! notdot's orb. The platform-neutral contract (`visual_state`, `theme`,
//! `contract` and the canonical `orb.frag`) always builds. The desktop web
//! UI is behind the default `web` feature; mobile cores disable it.
pub mod contract;
pub mod theme;
pub mod visual_state;

#[cfg(feature = "web")]
mod app;
