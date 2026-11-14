//! Owner step-up for sensitive actions from a phone.
//!
//! The phone's step-up key is a P-256 key behind a biometric access policy,
//! so a signature means "the owner just authenticated on that phone". On
//! Android, Google's key attestation shows the policy is enforced in
//! hardware; on iPhones it is the genuine app's behaviour, vouched for by
//! App Attest, not an attested property of the key. The desktop accepts the
//! key only through
//! enrolment with platform evidence ([`crate::attest`]), records what that
//! evidence proved, and issues single-use challenges bound to the exact
//! operation; the phone signs one only when the user starts that action, so
//! no prompt can appear while the user is absent. Phone biometrics never
//! authenticate to the desktop on their own: the owner must also grant
//! sensitive access at the desktop, and replacing the key ends the grant.
use crate::{DeviceId, LinkError, random_bytes};
use p256::ecdsa::{Signature, VerifyingKey, signature::Verifier};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

const LABEL: &[u8] = b"notdot/step-up/v1";
/// How long a challenge stays valid.
pub const CHALLENGE_TTL_MS: u64 = 60_000;
/// Outstanding action challenges per desktop.
pub const MAX_CHALLENGES: usize = 32;
/// Outstanding enrolment challenges per desktop, and per phone. Counted
/// apart from action challenges, so slow enrolments never crowd them out.
pub const MAX_ENROLL_CHALLENGES: usize = 8;
pub const MAX_ENROLL_PER_PHONE: usize = 2;

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
        Op::StepUpEnrollChallenge {} => out.push(8),
        Op::StepUpEnroll { nonce, evidence } => {
            out.push(9);
            out.extend_from_slice(nonce);
            evidence.canonical(&mut out);
        }
        Op::StepUpStatus {} => out.push(10),
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

/// How long an enrolment challenge stays valid: key generation and an
/// App Attest round trip to Apple can take a while.
pub const ENROLL_TTL_MS: u64 = 5 * 60_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Purpose {
    Action([u8; 32]),
    Enroll,
}

struct Challenge {
    phone: DeviceId,
    purpose: Purpose,
    expires_ms: u64,
}

/// A saved enrolment and grant (see [`StepUpDesk::snapshot`]).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registration {
    pub phone: DeviceId,
    pub enrollment: crate::attest::Enrollment,
    pub granted_until_ms: Option<u64>,
}

/// A phone's enrolled key and the owner's grant for it.
struct Registered {
    enrollment: crate::attest::Enrollment,
    granted_until_ms: Option<u64>,
}

/// What a phone may learn about its own step-up state.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StepUpState {
    pub protection: crate::attest::Protection,
    pub test_only: bool,
    /// App Attest's development environment (a debuggable build).
    pub development: bool,
    /// SHA-256 of the enrolled step-up public key, so the phone can tell
    /// which of its keys the desktop holds.
    pub key_sha256: [u8; 32],
    pub granted_until_ms: Option<u64>,
}

/// Desktop-side challenge book and enrolled keys.
pub struct StepUpDesk {
    desktop: DeviceId,
    challenges: HashMap<[u8; 32], Challenge>,
    registered: HashMap<DeviceId, Registered>,
}
impl StepUpDesk {
    pub fn new(desktop: DeviceId) -> Self {
        Self {
            desktop,
            challenges: HashMap::new(),
            registered: HashMap::new(),
        }
    }

    fn issue(
        &mut self,
        phone: DeviceId,
        purpose: Purpose,
        ttl_ms: u64,
        now_ms: u64,
    ) -> Result<[u8; 32], LinkError> {
        self.challenges.retain(|_, c| now_ms < c.expires_ms);
        let enrolling = |c: &&Challenge| c.purpose == Purpose::Enroll;
        let actions = |c: &&Challenge| c.purpose != Purpose::Enroll;
        let (total, per_phone) = if purpose == Purpose::Enroll {
            (MAX_ENROLL_CHALLENGES, MAX_ENROLL_PER_PHONE)
        } else {
            (MAX_CHALLENGES, MAX_CHALLENGES / 4)
        };
        let same: &dyn Fn(&&Challenge) -> bool = if purpose == Purpose::Enroll {
            &enrolling
        } else {
            &actions
        };
        if self.challenges.values().filter(same).count() >= total
            || self
                .challenges
                .values()
                .filter(same)
                .filter(|c| c.phone == phone)
                .count()
                >= per_phone
        {
            return Err(LinkError::Busy);
        }
        let nonce = random_bytes();
        self.challenges.insert(
            nonce,
            Challenge {
                phone,
                purpose,
                expires_ms: now_ms + ttl_ms,
            },
        );
        Ok(nonce)
    }

    /// Take a challenge: any use consumes it, whatever the outcome.
    fn take(
        &mut self,
        phone: DeviceId,
        nonce: &[u8; 32],
        purpose: Purpose,
        now_ms: u64,
    ) -> Result<(), LinkError> {
        let challenge = self
            .challenges
            .remove(nonce)
            .ok_or(LinkError::StepUpRejected)?;
        if challenge.phone != phone || challenge.purpose != purpose {
            return Err(LinkError::StepUpRejected);
        }
        if now_ms >= challenge.expires_ms {
            return Err(LinkError::StepUpExpired);
        }
        Ok(())
    }

    /// A challenge for one sensitive operation, for an enrolled phone whose
    /// grant is live.
    pub fn challenge(
        &mut self,
        phone: DeviceId,
        digest: [u8; 32],
        policy: &crate::attest::AttestationPolicy,
        now_ms: u64,
    ) -> Result<[u8; 32], LinkError> {
        self.recheck(phone, policy, now_ms)?;
        if !self.granted(phone, now_ms) {
            return Err(LinkError::Forbidden);
        }
        self.issue(phone, Purpose::Action(digest), CHALLENGE_TTL_MS, now_ms)
    }

    /// Recheck an enrolment against the current policy, and forget it if
    /// it no longer passes (a certificate revoked since, the app removed,
    /// development builds no longer allowed).
    fn recheck(
        &mut self,
        phone: DeviceId,
        policy: &crate::attest::AttestationPolicy,
        now_ms: u64,
    ) -> Result<(), LinkError> {
        let Some(registered) = self.registered.get(&phone) else {
            return Ok(());
        };
        if let Err(rejected) = policy.still_accepts(&registered.enrollment, now_ms) {
            self.forget(phone);
            return Err(LinkError::StepUpWithdrawn(rejected.to_string()));
        }
        Ok(())
    }

    /// A challenge for enrolling a key.
    pub fn enroll_challenge(
        &mut self,
        phone: DeviceId,
        now_ms: u64,
    ) -> Result<[u8; 32], LinkError> {
        self.issue(phone, Purpose::Enroll, ENROLL_TTL_MS, now_ms)
    }

    /// Consume an enrolment challenge and verify the evidence. On success
    /// the key replaces any earlier one and the owner's grant ends.
    pub fn enroll(
        &mut self,
        phone: DeviceId,
        nonce: &[u8; 32],
        evidence: &crate::attest::Evidence,
        policy: &crate::attest::AttestationPolicy,
        now_ms: u64,
    ) -> Result<Result<StepUpState, crate::attest::Rejected>, LinkError> {
        self.take(phone, nonce, Purpose::Enroll, now_ms)?;
        let enrollment = match policy.verify(evidence, self.desktop, phone, nonce, now_ms) {
            Ok(enrollment) => enrollment,
            Err(rejected) => return Ok(Err(rejected)),
        };
        // One key per phone; another phone may not reuse it.
        if self
            .registered
            .iter()
            .any(|(other, r)| *other != phone && r.enrollment.step_up_key == enrollment.step_up_key)
        {
            return Ok(Err(crate::attest::Rejected::WrongKey));
        }
        self.registered.insert(
            phone,
            Registered {
                enrollment,
                granted_until_ms: None,
            },
        );
        Ok(Ok(self.state(phone).expect("just registered")))
    }

    pub fn state(&self, phone: DeviceId) -> Option<StepUpState> {
        self.registered.get(&phone).map(|r| StepUpState {
            protection: r.enrollment.protection,
            test_only: r.enrollment.test_only,
            development: r
                .enrollment
                .app_attest
                .as_ref()
                .is_some_and(|a| a.environment == crate::attest::apple::Environment::Development),
            key_sha256: Sha256::digest(&r.enrollment.step_up_key).into(),
            granted_until_ms: r.granted_until_ms,
        })
    }

    pub fn enrollment(&self, phone: DeviceId) -> Option<&crate::attest::Enrollment> {
        self.registered.get(&phone).map(|r| &r.enrollment)
    }

    fn granted(&self, phone: DeviceId, now_ms: u64) -> bool {
        self.registered
            .get(&phone)
            .and_then(|r| r.granted_until_ms)
            .is_some_and(|until| now_ms < until)
    }

    /// Owner action on the desktop: allow sensitive requests (each still
    /// needing a step-up) until `until_ms`. Needs an enrolled key.
    pub fn grant(&mut self, phone: DeviceId, until_ms: u64) -> Result<(), LinkError> {
        let registered = self
            .registered
            .get_mut(&phone)
            .ok_or(LinkError::Forbidden)?;
        registered.granted_until_ms = Some(until_ms);
        Ok(())
    }

    /// The durable part: enrolled keys and grants. Challenges are never
    /// saved, so nothing signed before a restart can be used after it.
    pub fn snapshot(&self) -> Vec<Registration> {
        let mut out: Vec<_> = self
            .registered
            .iter()
            .map(|(phone, r)| Registration {
                phone: *phone,
                enrollment: r.enrollment.clone(),
                granted_until_ms: r.granted_until_ms,
            })
            .collect();
        out.sort_by_key(|r| r.phone.0);
        out
    }

    /// Restore keys and grants saved by [`Self::snapshot`].
    pub fn restore(&mut self, registrations: Vec<Registration>) {
        self.registered = registrations
            .into_iter()
            .map(|r| {
                (
                    r.phone,
                    Registered {
                        enrollment: r.enrollment,
                        granted_until_ms: r.granted_until_ms,
                    },
                )
            })
            .collect();
    }

    /// Forget a phone's key, grant and challenges (removal, re-pairing, or
    /// the owner withdrawing sensitive access entirely).
    /// End a phone's grant now, keeping its key.
    pub fn withdraw(&mut self, phone: DeviceId) {
        if let Some(registered) = self.registered.get_mut(&phone) {
            registered.granted_until_ms = None;
        }
    }

    pub fn forget(&mut self, phone: DeviceId) {
        self.registered.remove(&phone);
        self.challenges.retain(|_, c| c.phone != phone);
    }

    /// Consume an action challenge and check the proof. Any failure
    /// consumes it. For App Attest enrolments the assertion's counter is
    /// advanced only when everything verifies.
    pub fn verify(
        &mut self,
        phone: DeviceId,
        nonce: &[u8; 32],
        digest: &[u8; 32],
        proof: &crate::protocol::StepUpProof,
        policy: &crate::attest::AttestationPolicy,
        now_ms: u64,
    ) -> Result<(), LinkError> {
        self.take(phone, nonce, Purpose::Action(*digest), now_ms)?;
        self.recheck(phone, policy, now_ms)?;
        if !self.granted(phone, now_ms) {
            return Err(LinkError::Forbidden);
        }
        let desktop = self.desktop;
        let registered = self
            .registered
            .get_mut(&phone)
            .ok_or(LinkError::Forbidden)?;
        let message = message(desktop, phone, nonce, digest);
        let key = VerifyingKey::from_sec1_bytes(&registered.enrollment.step_up_key)
            .map_err(|_| LinkError::StepUpRejected)?;
        let signature = Signature::from_der(&proof.signature)
            .or_else(|_| Signature::from_slice(&proof.signature))
            .map_err(|_| LinkError::StepUpRejected)?;
        key.verify(&message, &signature)
            .map_err(|_| LinkError::StepUpRejected)?;
        match (&mut registered.enrollment.app_attest, &proof.assertion) {
            (None, None) => Ok(()),
            (Some(attested), Some(assertion)) => {
                let counter = crate::attest::apple::verify_assertion(
                    assertion,
                    &crate::attest::assertion_client_data(&message),
                    &attested.key,
                    &attested.rp_id_hash,
                    attested.counter,
                    &[attested.facts.validation_category],
                )
                .map_err(|_| LinkError::StepUpRejected)?;
                attested.counter = counter;
                Ok(())
            }
            // An iPhone must assert; nothing else may.
            _ => Err(LinkError::StepUpRejected),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use p256::ecdsa::{SigningKey, signature::Signer};

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
            Op::StepUpEnrollChallenge {},
            Op::StepUpEnroll {
                nonce: [0; 32],
                evidence: crate::attest::Evidence::AndroidKeystore { chain: Vec::new() },
            },
            Op::StepUpEnroll {
                nonce: [0; 32],
                evidence: crate::attest::Evidence::AppleAppAttest {
                    step_up_key: Vec::new(),
                    key_id: [0; 32],
                    attestation: Vec::new(),
                },
            },
            Op::StepUpStatus {},
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

    use crate::attest::testing::{TestRoots, point, random_key};
    use crate::protocol::StepUpProof;

    const NOW: u64 = 1_790_000_000_000;

    fn proof(
        key: &SigningKey,
        desktop: DeviceId,
        phone: DeviceId,
        nonce: [u8; 32],
        digest: [u8; 32],
    ) -> StepUpProof {
        let sig: Signature = key.sign(&message(desktop, phone, &nonce, &digest));
        StepUpProof {
            nonce,
            signature: sig.to_der().as_bytes().to_vec(),
            assertion: None,
        }
    }

    /// Enrol an Android test key for `phone` and return it.
    pub fn enrol_android(desk: &mut StepUpDesk, roots: &TestRoots, phone: DeviceId) -> SigningKey {
        let policy = roots.policy(NOW);
        let nonce = desk.enroll_challenge(phone, NOW).unwrap();
        let challenge = crate::attest::attestation_challenge(desk.desktop, phone, &nonce);
        let (key, evidence) = roots.android_evidence(challenge);
        desk.enroll(phone, &nonce, &evidence, &policy, NOW)
            .unwrap()
            .unwrap();
        key
    }

    #[test]
    fn signatures_are_bound_to_phone_operation_grant_and_single_use() {
        let desktop = DeviceId([1; 32]);
        let phone = DeviceId([2; 32]);
        let mut desk = StepUpDesk::new(desktop);
        let roots = TestRoots::new(NOW);
        let policy = roots.policy(NOW);
        let digest = [9; 32];
        // Not enrolled, then enrolled but not granted: no challenges.
        assert!(matches!(
            desk.challenge(phone, digest, &policy, NOW),
            Err(LinkError::Forbidden)
        ));
        let key = enrol_android(&mut desk, &roots, phone);
        assert!(matches!(
            desk.challenge(phone, digest, &policy, NOW),
            Err(LinkError::Forbidden)
        ));
        desk.grant(phone, NOW + 600_000).unwrap();
        let nonce = desk.challenge(phone, digest, &policy, NOW).unwrap();
        let good = proof(&key, desktop, phone, nonce, digest);
        desk.verify(phone, &nonce, &digest, &good, &policy, NOW + 1)
            .unwrap();
        // Replay.
        assert!(
            desk.verify(phone, &nonce, &digest, &good, &policy, NOW + 2)
                .is_err()
        );
        // Different operation, phone or key; an unexpected assertion.
        for (phone_used, digest_used, signer, assertion) in [
            (phone, [8; 32], key.clone(), None),
            (DeviceId([3; 32]), digest, key.clone(), None),
            (phone, digest, random_key(), None),
            (phone, digest, key.clone(), Some(vec![0xa0])),
        ] {
            let nonce = desk.challenge(phone, digest, &policy, NOW).unwrap();
            let mut attempt = proof(&signer, desktop, phone_used, nonce, digest_used);
            attempt.assertion = assertion;
            assert!(
                desk.verify(phone_used, &nonce, &digest_used, &attempt, &policy, NOW + 1)
                    .is_err()
            );
        }
        // Expiry is reported as such, and still consumes the challenge.
        let nonce = desk.challenge(phone, digest, &policy, NOW).unwrap();
        let late = proof(&key, desktop, phone, nonce, digest);
        assert!(matches!(
            desk.verify(
                phone,
                &nonce,
                &digest,
                &late,
                &policy,
                NOW + CHALLENGE_TTL_MS
            ),
            Err(LinkError::StepUpExpired)
        ));
        assert!(matches!(
            desk.verify(phone, &nonce, &digest, &late, &policy, NOW + 1),
            Err(LinkError::StepUpRejected)
        ));
        // An expired grant refuses even a valid proof.
        let nonce = desk.challenge(phone, digest, &policy, NOW).unwrap();
        let ok = proof(&key, desktop, phone, nonce, digest);
        desk.grant(phone, NOW + 10).unwrap();
        assert!(matches!(
            desk.verify(phone, &nonce, &digest, &ok, &policy, NOW + 10),
            Err(LinkError::Forbidden)
        ));
        // An enrolment challenge cannot authorise an action, nor the reverse.
        desk.grant(phone, NOW + 600_000).unwrap();
        let enrol_nonce = desk.enroll_challenge(phone, NOW).unwrap();
        let misuse = proof(&key, desktop, phone, enrol_nonce, digest);
        assert!(
            desk.verify(phone, &enrol_nonce, &digest, &misuse, &policy, NOW)
                .is_err()
        );
        let action_nonce = desk.challenge(phone, digest, &policy, NOW).unwrap();
        let challenge = crate::attest::attestation_challenge(desktop, phone, &action_nonce);
        let (_, evidence) = roots.android_evidence(challenge);
        assert!(
            desk.enroll(phone, &action_nonce, &evidence, &roots.policy(NOW), NOW)
                .is_err()
        );
    }

    #[test]
    fn re_enrolment_ends_the_grant_and_keys_are_not_shared() {
        let desktop = DeviceId([1; 32]);
        let (phone, other) = (DeviceId([2; 32]), DeviceId([3; 32]));
        let mut desk = StepUpDesk::new(desktop);
        let roots = TestRoots::new(NOW);
        let policy = roots.policy(NOW);
        enrol_android(&mut desk, &roots, phone);
        desk.grant(phone, NOW + 600_000).unwrap();
        assert!(desk.state(phone).unwrap().granted_until_ms.is_some());
        let key = enrol_android(&mut desk, &roots, phone);
        assert_eq!(desk.state(phone).unwrap().granted_until_ms, None);
        // Another phone presenting the same key is refused.
        let nonce = desk.enroll_challenge(other, NOW).unwrap();
        let challenge = crate::attest::attestation_challenge(desktop, other, &nonce);
        let chain = roots.android_chain(
            &key,
            &crate::attest::testing::KeySpec::good(challenge, &roots.android_app),
        );
        let evidence = crate::attest::Evidence::AndroidKeystore { chain };
        assert!(matches!(
            desk.enroll(other, &nonce, &evidence, &policy, NOW),
            Ok(Err(crate::attest::Rejected::WrongKey))
        ));
        // Evidence is single-use: the same challenge cannot enrol twice.
        assert!(matches!(
            desk.enroll(other, &nonce, &evidence, &policy, NOW),
            Err(LinkError::StepUpRejected)
        ));
        // An expired enrolment challenge.
        let nonce = desk.enroll_challenge(other, NOW).unwrap();
        assert!(matches!(
            desk.enroll(other, &nonce, &evidence, &policy, NOW + ENROLL_TTL_MS),
            Err(LinkError::StepUpExpired)
        ));
        // Forgetting removes key, grant and challenges.
        desk.forget(phone);
        assert!(desk.state(phone).is_none());
        assert!(matches!(
            desk.grant(phone, NOW + 1),
            Err(LinkError::Forbidden)
        ));
        // Challenges per phone are bounded.
        let mut issued = 0;
        while desk.enroll_challenge(other, NOW).is_ok() {
            issued += 1;
        }
        assert_eq!(issued, MAX_ENROLL_PER_PHONE);
        assert!(desk.enroll_challenge(phone, NOW).is_ok());
    }

    #[test]
    fn iphones_must_assert_with_a_rising_counter_alongside_the_signature() {
        let desktop = DeviceId([1; 32]);
        let phone = DeviceId([2; 32]);
        let mut desk = StepUpDesk::new(desktop);
        let roots = TestRoots::new(NOW);
        let policy = roots.policy(NOW);
        let mut device = roots.app_attest_device();
        let step_up = random_key();
        let nonce = desk.enroll_challenge(phone, NOW).unwrap();
        let evidence = device.evidence(desktop, phone, &nonce, &point(&step_up));
        let state = desk
            .enroll(phone, &nonce, &evidence, &roots.policy(NOW), NOW)
            .unwrap()
            .unwrap();
        assert_eq!(
            state.protection,
            crate::attest::Protection::AppleAppAttested
        );
        desk.grant(phone, NOW + 600_000).unwrap();
        let digest = [9; 32];
        let attempt = |desk: &mut StepUpDesk,
                       device: &mut crate::attest::testing::AppAttestDevice,
                       assert: bool| {
            let nonce = desk.challenge(phone, digest, &policy, NOW).unwrap();
            let mut p = proof(&step_up, desktop, phone, nonce, digest);
            if assert {
                let data =
                    crate::attest::assertion_client_data(&message(desktop, phone, &nonce, &digest));
                p.assertion = Some(device.assert(&data));
            }
            (nonce, p)
        };
        // Signature alone is not enough on an iPhone.
        let (nonce, bare) = attempt(&mut desk, &mut device, false);
        assert!(
            desk.verify(phone, &nonce, &digest, &bare, &policy, NOW)
                .is_err()
        );
        let (nonce, full) = attempt(&mut desk, &mut device, true);
        desk.verify(phone, &nonce, &digest, &full, &policy, NOW)
            .unwrap();
        assert_eq!(
            desk.enrollment(phone)
                .unwrap()
                .app_attest
                .as_ref()
                .unwrap()
                .counter,
            1
        );
        // A captured assertion replayed with a new challenge fails: it is
        // bound to the old message, and its counter is spent.
        let nonce = desk.challenge(phone, digest, &policy, NOW).unwrap();
        let mut replay = proof(&step_up, desktop, phone, nonce, digest);
        replay.assertion = full.assertion.clone();
        assert!(
            desk.verify(phone, &nonce, &digest, &replay, &policy, NOW)
                .is_err()
        );
        // A failed attempt does not advance the counter.
        assert_eq!(
            desk.enrollment(phone)
                .unwrap()
                .app_attest
                .as_ref()
                .unwrap()
                .counter,
            1
        );
    }

    #[test]
    fn a_key_revoked_or_disallowed_after_enrolment_is_withdrawn() {
        let desktop = DeviceId([1; 32]);
        let phone = DeviceId([2; 32]);
        let roots = TestRoots::new(NOW);
        let digest = [9; 32];
        for change in 0..4 {
            let mut desk = StepUpDesk::new(desktop);
            let key = enrol_android(&mut desk, &roots, phone);
            desk.grant(phone, NOW + 600_000).unwrap();
            let mut policy = roots.policy(NOW);
            let nonce = desk.challenge(phone, digest, &policy, NOW).unwrap();
            match change {
                // Google revokes the batch certificate after enrolment.
                0 => {
                    let serial =
                        crate::attest::der::Certificate::parse(&roots.android_intermediate.der)
                            .unwrap()
                            .serial
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect::<String>();
                    let json = format!(
                        r#"{{"entries":{{"{}":{{"status":"REVOKED"}}}}}}"#,
                        serial.trim_start_matches('0')
                    );
                    policy.set_status_list(
                        crate::attest::android::StatusList::parse(json.as_bytes(), NOW).unwrap(),
                    );
                }
                // The owner stops trusting the app or its signer.
                1 => policy.android_apps.clear(),
                2 => policy.android_apps[0].signers = vec![[0x11; 32]],
                // The revocation list goes stale.
                _ => {}
            }
            let now = if change == 3 {
                NOW + crate::attest::android::MAX_STATUS_AGE_MS + 1
            } else {
                NOW
            };
            let p = proof(&key, desktop, phone, nonce, digest);
            let outcome = if change == 3 {
                // Long after the action challenge: the next request is refused.
                desk.challenge(phone, digest, &policy, now).map(|_| ())
            } else {
                desk.verify(phone, &nonce, &digest, &p, &policy, now)
            };
            assert!(
                matches!(outcome, Err(LinkError::StepUpWithdrawn(_))),
                "change {change}: {outcome:?}"
            );
            // Withdrawn means forgotten: no key, no grant, no challenges.
            assert!(desk.state(phone).is_none());
            assert!(
                desk.challenge(phone, digest, &roots.policy(NOW), NOW)
                    .is_err()
            );
        }
    }

    #[test]
    fn enrolment_challenges_cannot_crowd_out_actions() {
        let desktop = DeviceId([1; 32]);
        let roots = TestRoots::new(NOW);
        let policy = roots.policy(NOW);
        let mut desk = StepUpDesk::new(desktop);
        let owner = DeviceId([2; 32]);
        enrol_android(&mut desk, &roots, owner);
        desk.grant(owner, NOW + 600_000).unwrap();
        for p in 3..20u8 {
            let _ = desk.enroll_challenge(DeviceId([p; 32]), NOW);
            let _ = desk.enroll_challenge(DeviceId([p; 32]), NOW);
            let _ = desk.enroll_challenge(DeviceId([p; 32]), NOW);
        }
        assert!(desk.enroll_challenge(DeviceId([30; 32]), NOW).is_err());
        assert!(desk.challenge(owner, [9; 32], &policy, NOW).is_ok());
        let mut one = StepUpDesk::new(desktop);
        assert!(one.enroll_challenge(owner, NOW).is_ok());
        assert!(one.enroll_challenge(owner, NOW).is_ok());
        assert!(one.enroll_challenge(owner, NOW).is_err());
    }
}
