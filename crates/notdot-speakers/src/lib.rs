//! On-device speaker recognition for notdot.
//!
//! - [`audio`]: PCM decoding, voiced-frame selection and sample quality checks.
//! - [`embedding`]: checksum-verified CAM++ speaker embeddings via sherpa-onnx.
//! - [`matcher`]: cosine scoring with accept/reject bands, margin and replay flag.
//! - [`enrol`]: guided, consented, retryable enrolment.
//! - [`store`]: ChaCha20-Poly1305 encrypted voiceprints with an OS-held key.
//! - [`gate`]: bounded local PCM hold so audio never leaves the device before
//!   the speaker decision in locked modes.
//! - [`engine`]: bounded, serialised identification.
//!
//! Voice matching is convenience identity, not authentication: recordings,
//! clones and noise can defeat it. The replay flag only catches verbatim reuse
//! of an enrolment clip.
#![forbid(unsafe_code)]
pub mod audio;
pub mod embedding;
pub mod engine;
pub mod enrol;
pub mod gate;
pub mod matcher;
pub mod store;

/// Pinned model: 3D-Speaker CAM++ "common advanced" (Chinese and English,
/// 16 kHz), Apache-2.0, from the k2-fsa/sherpa-onnx
/// `speaker-recongition-models` release. Chosen over the English VoxCeleb CAM++
/// export, which did not separate speakers in notdot's evaluation (see
/// docs/validation/2026-11-04-speaker-identity.md).
pub const MODEL_FILE: &str = "3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx";
pub const MODEL_SHA256: &str = "aa3cfc16963a10586a9393f5035d6d6b57e98d358b347f80c2a30bf4f00ceba2";
pub const MODEL_BYTES: u64 = 28_281_164;
pub const EMBEDDING_DIM: usize = 192;
