//! Owner step-up for sensitive actions from a phone.
//!
//! The phone's step-up key is a P-256 key created in its secure hardware
//! (Secure Enclave / StrongBox) behind a biometric access policy, so a
//! signature means "the owner just authenticated on that phone". The desktop
//! issues a single-use challenge bound to the exact operation; the phone signs
//! it only when the user initiates that action, so no prompt can appear while
//! the user is absent. Phone biometrics never authenticate to the desktop on
//! their own: the device must also hold an owner grant made at the desktop.
use crate::{DeviceId, LinkError, random_bytes};
use p256::ecdsa::{Signature, VerifyingKey, signature::Verifier};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

const LABEL: &[u8] = b"notdot/step-up/v1";
/// How long a challenge stays valid.
pub const CHALLENGE_TTL_MS: u64 = 60_000;
/// Outstanding challenges per desktop.
pub const MAX_CHALLENGES: usize = 32;

/// SHA-256 of the operation as serde_json encodes it. Both sides use this
/// Rust crate today, so the bytes agree. Before `link/1` is frozen, define
/// the digest over explicitly canonical bytes so a non-Rust client could
/// reproduce it (review finding, tracked in mobile/README.md).
pub fn op_digest(op: &crate::protocol::Op) -> [u8; 32] {
    Sha256::digest(serde_json::to_vec(op).expect("ops serialise")).into()
}

/// The bytes a phone signs.
pub fn message(desktop: DeviceId, phone: DeviceId, nonce: &[u8; 32], digest: &[u8; 32]) -> Vec<u8> {
    [LABEL, &desktop.0, &phone.0, nonce, digest].concat()
}

struct Challenge {
    phone: DeviceId,
    digest: [u8; 32],
    expires_ms: u64,
}

/// Desktop-side challenge book.
pub struct StepUpDesk {
    desktop: DeviceId,
    challenges: HashMap<[u8; 32], Challenge>,
}
impl StepUpDesk {
    pub fn new(desktop: DeviceId) -> Self {
        Self {
            desktop,
            challenges: HashMap::new(),
        }
    }

    pub fn challenge(
        &mut self,
        phone: DeviceId,
        digest: [u8; 32],
        now_ms: u64,
    ) -> Result<[u8; 32], LinkError> {
        self.challenges.retain(|_, c| now_ms < c.expires_ms);
        if self.challenges.len() >= MAX_CHALLENGES {
            return Err(LinkError::Busy);
        }
        let nonce = random_bytes();
        self.challenges.insert(
            nonce,
            Challenge {
                phone,
                digest,
                expires_ms: now_ms + CHALLENGE_TTL_MS,
            },
        );
        Ok(nonce)
    }

    /// Consume a challenge and check the signature. Any failure consumes it.
    pub fn verify(
        &mut self,
        phone: DeviceId,
        public_key: &[u8],
        nonce: &[u8; 32],
        digest: &[u8; 32],
        signature: &[u8],
        now_ms: u64,
    ) -> Result<(), LinkError> {
        let challenge = self
            .challenges
            .remove(nonce)
            .ok_or(LinkError::StepUpRejected)?;
        if challenge.phone != phone || &challenge.digest != digest || now_ms >= challenge.expires_ms
        {
            return Err(LinkError::StepUpRejected);
        }
        let key =
            VerifyingKey::from_sec1_bytes(public_key).map_err(|_| LinkError::StepUpRejected)?;
        let signature = Signature::from_der(signature)
            .or_else(|_| Signature::from_slice(signature))
            .map_err(|_| LinkError::StepUpRejected)?;
        key.verify(&message(self.desktop, phone, nonce, digest), &signature)
            .map_err(|_| LinkError::StepUpRejected)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use p256::ecdsa::{SigningKey, signature::Signer};

    pub fn key() -> SigningKey {
        SigningKey::from_slice(&[7; 32]).unwrap()
    }
    pub fn public(key: &SigningKey) -> Vec<u8> {
        key.verifying_key()
            .to_encoded_point(false)
            .as_bytes()
            .to_vec()
    }

    #[test]
    fn signatures_are_bound_to_phone_operation_and_single_use() {
        let desktop = DeviceId([1; 32]);
        let phone = DeviceId([2; 32]);
        let mut desk = StepUpDesk::new(desktop);
        let key = key();
        let digest = [9; 32];
        let nonce = desk.challenge(phone, digest, 0).unwrap();
        let sig: Signature = key.sign(&message(desktop, phone, &nonce, &digest));
        let der = sig.to_der();
        desk.verify(phone, &public(&key), &nonce, &digest, der.as_bytes(), 1)
            .unwrap();
        // Replay.
        assert!(
            desk.verify(phone, &public(&key), &nonce, &digest, der.as_bytes(), 2)
                .is_err()
        );
        // Different operation, phone, expiry or key.
        for (phone_used, digest_used, now, signer) in [
            (phone, [8; 32], 1, key.clone()),
            (DeviceId([3; 32]), digest, 1, key.clone()),
            (phone, digest, CHALLENGE_TTL_MS, key.clone()),
            (phone, digest, 1, SigningKey::from_slice(&[6; 32]).unwrap()),
        ] {
            let nonce = desk.challenge(phone, digest, 0).unwrap();
            let sig: Signature = signer.sign(&message(desktop, phone_used, &nonce, &digest_used));
            assert!(
                desk.verify(
                    phone_used,
                    &public(&key),
                    &nonce,
                    &digest_used,
                    sig.to_der().as_bytes(),
                    now
                )
                .is_err()
            );
        }
    }
}
