//! notdot's paired-device link (prototype).
//!
//! - [`pairing`]: QR pairing with pinned keys, desktop approval, expiry,
//!   replay protection, revocation and device-profile scopes.
//! - [`stepup`]: owner step-up for sensitive actions.
//! - [`protocol`]: the typed `link/1` API and framing.
//! - [`service`]: idempotent, cancellable, bounded execution per device.
//! - [`transport`]: Iroh QUIC endpoints (direct or relayed; end-to-end
//!   encrypted either way) for the desktop and the phone.
//!
//! The desktop never opens a listener for its existing runtime commands; the
//! only inbound surface is this allowlisted protocol, after pairing.
pub mod pairing;
pub mod protocol;
pub mod service;
pub mod stepup;
pub mod transport;

use serde::{Deserialize, Serialize};

/// A device's public key: the Ed25519 key its transport authenticates with.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DeviceId(#[serde(with = "b64_32")] pub [u8; 32]);
impl std::fmt::Debug for DeviceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DeviceId(")?;
        for byte in &self.0[..4] {
            write!(f, "{byte:02x}")?;
        }
        write!(f, "…)")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LinkError {
    #[error("pairing code: {0}")]
    BadQr(&'static str),
    #[error("bad request: {0}")]
    BadRequest(&'static str),
    #[error("that pairing code has expired or does not exist")]
    OfferUnavailable,
    #[error("that pairing code was already used")]
    OfferUsed,
    #[error("pairing proof did not verify")]
    BadProof,
    #[error("too many open pairing codes")]
    TooManyOffers,
    #[error("too many paired phones")]
    TooManyDevices,
    #[error("not allowed")]
    Forbidden,
    #[error("busy")]
    Busy,
    #[error("step-up rejected")]
    StepUpRejected,
    #[error("frame too large")]
    FrameTooLarge,
    #[error("refused: {0}")]
    Refused(pairing::Refusal),
    #[error("pairing denied on the computer")]
    Denied,
    #[error("connection: {0}")]
    Connection(String),
    #[error("i/o: {0}")]
    Io(String),
}
impl LinkError {
    pub(crate) fn io(error: impl std::fmt::Display) -> Self {
        Self::Io(error.to_string())
    }
}

/// Bytes from the operating system's CSPRNG.
pub(crate) fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).expect("the OS random source is available");
    bytes
}

pub(crate) mod b64 {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn encode(bytes: &[u8]) -> String {
        URL_SAFE_NO_PAD.encode(bytes)
    }
    pub fn decode(text: &str) -> Option<Vec<u8>> {
        URL_SAFE_NO_PAD.decode(text).ok()
    }
    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&encode(bytes))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        if text.len() > 512 {
            return Err(serde::de::Error::custom("too long"));
        }
        decode(&text).ok_or_else(|| serde::de::Error::custom("not base64url"))
    }
}

pub(crate) mod b64_32 {
    use serde::{Deserializer, Serializer};
    pub fn serialize<S: Serializer>(bytes: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
        super::b64::serialize(bytes, s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
        super::b64::deserialize(d)?
            .try_into()
            .map_err(|_| serde::de::Error::custom("expected 32 bytes"))
    }
}
