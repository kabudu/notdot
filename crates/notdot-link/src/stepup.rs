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

/// Domain separation for operation digests.
const OP_LABEL: &[u8] = b"notdot/link/1/op";

fn field(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
}

/// The canonical byte encoding of an operation: a one-byte tag, then each
/// field in declaration order; strings and byte strings are length-prefixed
/// (u32, big-endian), integers are big-endian. Independent of JSON, so any
/// client can reproduce it. Adding an operation is a compile error here
/// until its encoding is defined; existing tags never change within link/1.
pub fn op_bytes(op: &crate::protocol::Op) -> Vec<u8> {
    use crate::protocol::Op;
    let mut out = Vec::new();
    match op {
        Op::Ping {} => out.push(0),
        Op::AddNote { text } => {
            out.push(1);
            field(&mut out, text.as_bytes());
        }
        Op::ListNotes {} => out.push(2),
        Op::ReadSettings {} => out.push(3),
        Op::StageSetting {
            key,
            value,
            base_revision,
        } => {
            out.push(4);
            field(&mut out, key.as_bytes());
            field(&mut out, value.as_bytes());
            out.extend_from_slice(&base_revision.to_be_bytes());
        }
        Op::RunTask { label, steps } => {
            out.push(5);
            field(&mut out, label.as_bytes());
            out.extend_from_slice(&steps.to_be_bytes());
        }
        Op::ResetMemory {} => out.push(6),
        Op::StepUpChallenge { digest } => {
            out.push(7);
            out.extend_from_slice(digest);
        }
    }
    out
}

/// SHA-256 over the label and the canonical encoding of an operation.
pub fn op_digest(op: &crate::protocol::Op) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(OP_LABEL);
    hash.update(op_bytes(op));
    hash.finalize().into()
}

/// Whether `bytes` is a SEC1-encoded P-256 public key (a step-up key).
pub fn valid_key(bytes: &[u8]) -> bool {
    VerifyingKey::from_sec1_bytes(bytes).is_ok()
}

/// The nonce in a `StepUpChallenge` result (`{"nonce": base64}`).
pub fn challenge_nonce(result: &serde_json::Value) -> Option<[u8; 32]> {
    crate::b64::decode(result["nonce"].as_str()?)?
        .try_into()
        .ok()
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
    fn operation_digests_are_canonical_pinned_and_unambiguous() {
        use crate::protocol::Op;
        let hex = |d: [u8; 32]| d.iter().map(|b| format!("{b:02x}")).collect::<String>();
        // Pinned vectors: a change here breaks every signing client.
        assert_eq!(op_bytes(&Op::ResetMemory {}), [6]);
        // sha256("notdot/link/1/op" || 0x06), computed independently.
        assert_eq!(
            hex(op_digest(&Op::ResetMemory {})),
            "2d3eeab6669b600456965f49c1113b8cac7374502959b66094a041e7b5a5b675"
        );
        let stage = Op::StageSetting {
            key: "hue".into(),
            value: "42".into(),
            base_revision: 7,
        };
        assert_eq!(
            op_bytes(&stage),
            [
                &[4u8][..],
                &[0, 0, 0, 3],
                b"hue",
                &[0, 0, 0, 2],
                b"42",
                &7u64.to_be_bytes()
            ]
            .concat()
        );
        // Shifting a boundary between fields changes the encoding.
        let shifted = Op::StageSetting {
            key: "hue4".into(),
            value: "2".into(),
            base_revision: 7,
        };
        assert_ne!(op_digest(&stage), op_digest(&shifted));
        // Every operation kind has a distinct encoding.
        let all = [
            Op::Ping {},
            Op::AddNote {
                text: String::new(),
            },
            Op::ListNotes {},
            Op::ReadSettings {},
            Op::StageSetting {
                key: String::new(),
                value: String::new(),
                base_revision: 0,
            },
            Op::RunTask {
                label: String::new(),
                steps: 0,
            },
            Op::ResetMemory {},
            Op::StepUpChallenge { digest: [0; 32] },
        ];
        let digests: std::collections::HashSet<_> = all.iter().map(op_digest).collect();
        assert_eq!(digests.len(), all.len());
        // Independent of JSON: the digest is not a hash of serde output.
        assert_ne!(
            op_digest(&Op::ResetMemory {}),
            <[u8; 32]>::from(Sha256::digest(
                serde_json::to_vec(&Op::ResetMemory {}).unwrap()
            ))
        );
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
