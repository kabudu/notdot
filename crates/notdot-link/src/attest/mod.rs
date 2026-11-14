//! Desktop-verified attestation of a phone's step-up key.
//!
//! A phone enrols its step-up key with evidence the desktop can check
//! against platform roots, bound to a fresh single-use challenge from the
//! desktop, the desktop and phone identities, and the key itself. The
//! client's own report of its hardware is never trusted.
//!
//! The platforms prove different things, and the desktop records which:
//!
//! - **Android** key attestation covers the step-up key itself: a chain to
//!   Google's root, with no certificate on Google's revocation list, says it
//!   was generated in a TEE or StrongBox, needs a strong biometric for every
//!   signature, belongs to an allowed app, and lives on a locked,
//!   verified-boot device ([`Protection::AndroidStrongBox`],
//!   [`Protection::AndroidTrustedEnvironment`]). That holds only while the
//!   device's attestation key has not leaked unnoticed, so enrolments are
//!   rechecked against every new revocation list and policy
//!   ([`AttestationPolicy::still_accepts`]).
//! - **iOS** has no attestation for arbitrary Secure Enclave keys. App Attest
//!   proves that a genuine, unmodified instance of the app on Apple hardware
//!   holds a separate attested key. The app binds the step-up public key into
//!   that attestation and signs every sensitive action with both keys, so the
//!   desktop knows the genuine app vouched for the step-up key; that the
//!   step-up key is in the Secure Enclave behind biometrics is the app's
//!   behaviour, not something Apple attests ([`Protection::AppleAppAttested`]).
//!
//! Everything fails closed: an unconfigured policy, a stale revocation list,
//! an unknown root, or any unexpected field refuses enrolment.
pub mod android;
pub mod apple;
pub mod der;
#[cfg(any(test, feature = "test-attestation"))]
pub mod testing;

use crate::DeviceId;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const ANDROID_LABEL: &[u8] = b"notdot/step-up/attest/android/v1";
const APPLE_LABEL: &[u8] = b"notdot/step-up/attest/apple/v1";
const ASSERT_LABEL: &[u8] = b"notdot/step-up/assert/apple/v1";

/// Why evidence was refused. Messages are for the owner; none of them
/// reveal more than which check failed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Rejected {
    #[error("the evidence is malformed ({0})")]
    Malformed(&'static str),
    #[error("the evidence uses something this computer does not support ({0})")]
    Unsupported(&'static str),
    #[error("the certificate chain is invalid ({0})")]
    Chain(&'static str),
    #[error("the evidence is not signed by Google or Apple")]
    UntrustedRoot,
    #[error("a certificate in the evidence has expired or is not yet valid")]
    Expired,
    #[error("Google has revoked a certificate in the evidence")]
    Revoked,
    #[error("Google's revocation list is not available or out of date")]
    RevocationUnavailable,
    #[error("the key is not protected by secure hardware")]
    NotHardware,
    #[error("the evidence was made for a different request")]
    Challenge,
    #[error("the evidence is for a different key")]
    WrongKey,
    #[error("the key does not have the required protections ({0})")]
    KeyProperties(&'static str),
    #[error("the phone's software has been modified or its bootloader is unlocked")]
    DeviceIntegrity,
    #[error("the evidence comes from an app this computer does not trust")]
    App,
    #[error("the evidence comes from a development build")]
    Environment,
    #[error("the evidence was already used")]
    Counter,
    #[error("the signature does not verify")]
    Signature,
    #[error("this computer is not set up to accept this kind of phone")]
    NotConfigured,
}

/// The value an Android phone passes to `setAttestationChallenge`.
pub fn attestation_challenge(desktop: DeviceId, phone: DeviceId, nonce: &[u8; 32]) -> [u8; 32] {
    Sha256::new()
        .chain_update(ANDROID_LABEL)
        .chain_update(desktop.0)
        .chain_update(phone.0)
        .chain_update(nonce)
        .finalize()
        .into()
}

/// The `clientDataHash` an iPhone passes to `attestKey`: the challenge
/// inputs plus the step-up public key the attestation vouches for.
pub fn app_attest_client_data_hash(
    desktop: DeviceId,
    phone: DeviceId,
    nonce: &[u8; 32],
    step_up_key: &[u8],
) -> [u8; 32] {
    Sha256::new()
        .chain_update(APPLE_LABEL)
        .chain_update(desktop.0)
        .chain_update(phone.0)
        .chain_update(nonce)
        .chain_update((step_up_key.len() as u32).to_be_bytes())
        .chain_update(step_up_key)
        .finalize()
        .into()
}

/// The client data an iPhone asserts for a step-up `message`.
pub fn assertion_client_data(message: &[u8]) -> Vec<u8> {
    [ASSERT_LABEL, message].concat()
}

/// What a phone sends to enrol its step-up key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "platform", rename_all = "snake_case", deny_unknown_fields)]
pub enum Evidence {
    /// The step-up key's attestation chain, leaf first.
    AndroidKeystore {
        #[serde(with = "b64_list")]
        chain: Vec<Vec<u8>>,
    },
    /// An App Attest attestation whose client data binds `step_up_key`.
    AppleAppAttest {
        #[serde(with = "crate::b64")]
        step_up_key: Vec<u8>,
        #[serde(with = "crate::b64_32")]
        key_id: [u8; 32],
        #[serde(with = "b64_object")]
        attestation: Vec<u8>,
    },
}
impl Evidence {
    /// Canonical bytes for request-id binding.
    pub(crate) fn canonical(&self, out: &mut Vec<u8>) {
        let field = |out: &mut Vec<u8>, bytes: &[u8]| {
            out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
            out.extend_from_slice(bytes);
        };
        match self {
            Evidence::AndroidKeystore { chain } => {
                out.push(1);
                out.extend_from_slice(&(chain.len() as u32).to_be_bytes());
                for cert in chain {
                    field(out, cert);
                }
            }
            Evidence::AppleAppAttest {
                step_up_key,
                key_id,
                attestation,
            } => {
                out.push(2);
                field(out, step_up_key);
                out.extend_from_slice(key_id);
                field(out, attestation);
            }
        }
    }
}

/// Base64url for objects up to the App Attest size limit.
pub(crate) mod b64_object {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&crate::b64::encode(bytes))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        if text.len() > super::apple::MAX_OBJECT_BYTES * 4 / 3 + 4 {
            return Err(serde::de::Error::custom("too long"));
        }
        crate::b64::decode(&text).ok_or_else(|| serde::de::Error::custom("not base64url"))
    }
}

/// [`b64_object`] for an optional field.
pub(crate) mod b64_object_opt {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(bytes: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        match bytes {
            Some(bytes) => super::b64_object::serialize(bytes, s),
            None => s.serialize_none(),
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
        let text = Option::<String>::deserialize(d)?;
        text.map(|text| {
            if text.len() > super::apple::MAX_OBJECT_BYTES * 4 / 3 + 4 {
                return Err(serde::de::Error::custom("too long"));
            }
            crate::b64::decode(&text).ok_or_else(|| serde::de::Error::custom("not base64url"))
        })
        .transpose()
    }
}

mod b64_list {
    use serde::{Deserialize, Deserializer, Serializer, ser::SerializeSeq};
    pub fn serialize<S: Serializer>(list: &[Vec<u8>], s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(list.len()))?;
        for item in list {
            seq.serialize_element(&crate::b64::encode(item))?;
        }
        seq.end()
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Vec<u8>>, D::Error> {
        let texts = Vec::<String>::deserialize(d)?;
        if texts.len() > super::der::MAX_CHAIN {
            return Err(serde::de::Error::custom("chain too long"));
        }
        texts
            .iter()
            .map(|t| {
                if t.len() > super::der::MAX_CERT_BYTES * 4 / 3 + 4 {
                    return Err(serde::de::Error::custom("certificate too long"));
                }
                crate::b64::decode(t).ok_or_else(|| serde::de::Error::custom("base64"))
            })
            .collect()
    }
}

/// What the desktop verified about a step-up key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protection {
    /// Google attests the key is in StrongBox behind a strong biometric.
    AndroidStrongBox,
    /// Google attests the key is in the TEE behind a strong biometric.
    AndroidTrustedEnvironment,
    /// Apple attests the genuine app vouched for the key; the key's own
    /// hardware protection is not attested.
    AppleAppAttested,
}
impl Protection {
    /// Plain words for the owner.
    pub fn describe(self) -> &'static str {
        match self {
            Protection::AndroidStrongBox => {
                "Google's certificates (not revoked) say the key is in this phone's security chip and needs a fingerprint or face for each use"
            }
            Protection::AndroidTrustedEnvironment => {
                "Google's certificates (not revoked) say the key is in this phone's secure processor and needs a fingerprint or face for each use"
            }
            Protection::AppleAppAttested => {
                "Apple confirms the request comes from the genuine notdot app on a real iPhone"
            }
        }
    }
}

/// An enrolled step-up key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Enrollment {
    #[serde(with = "crate::b64_any")]
    pub step_up_key: Vec<u8>,
    pub protection: Protection,
    /// The App Attest key and state, for iPhones.
    pub app_attest: Option<AppAttestKey>,
    /// What the Android chain showed, rechecked against later policy and
    /// revocation lists.
    pub android: Option<android::Verified>,
    /// Verified against test roots: never genuine hardware evidence.
    pub test_only: bool,
    pub enrolled_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppAttestKey {
    #[serde(with = "crate::b64_any")]
    pub key: Vec<u8>,
    #[serde(with = "crate::b64_32")]
    pub rp_id_hash: [u8; 32],
    pub environment: apple::Environment,
    pub counter: u32,
    pub facts: apple::AppFacts,
}

/// Which apps and roots the desktop accepts. The default accepts nothing.
#[derive(Clone, Debug)]
pub struct AttestationPolicy {
    android_anchors: Vec<Vec<u8>>,
    apple_anchors: Vec<Vec<u8>>,
    apple_root: Vec<u8>,
    pub android_apps: Vec<android::AndroidApp>,
    pub apple: apple::ApplePolicy,
    status: Option<android::StatusList>,
    test_only: bool,
}
impl Default for AttestationPolicy {
    fn default() -> Self {
        let apple_root = apple::apple_root();
        Self {
            android_anchors: android::google_anchors(),
            apple_anchors: vec![der::spki_of(&apple_root).expect("bundled root")],
            apple_root,
            android_apps: Vec::new(),
            apple: apple::ApplePolicy {
                apps: Vec::new(),
                allow_development: false,
                validation_categories: Vec::new(),
            },
            status: None,
            test_only: false,
        }
    }
}
impl AttestationPolicy {
    /// Install a freshly fetched Google status list.
    pub fn set_status_list(&mut self, status: android::StatusList) {
        self.status = Some(status);
    }

    /// Replace the platform roots with test roots. Every enrolment this
    /// policy accepts is marked `test_only`.
    #[cfg(any(test, feature = "test-attestation"))]
    pub fn with_test_roots(mut self, android_root: &[u8], apple_root: &[u8]) -> Self {
        self.android_anchors = vec![der::spki_of(android_root).expect("test root")];
        self.apple_anchors = vec![der::spki_of(apple_root).expect("test root")];
        self.apple_root = apple_root.to_vec();
        self.test_only = true;
        self
    }

    /// Whether an earlier enrolment still passes this policy: its app is
    /// still allowed, and for Android no certificate in its chain is on a
    /// fresh revocation list. Checked before every sensitive action.
    pub fn still_accepts(&self, enrollment: &Enrollment, now_ms: u64) -> Result<(), Rejected> {
        if enrollment.test_only != self.test_only {
            return Err(Rejected::UntrustedRoot);
        }
        if let Some(verified) = &enrollment.android {
            android::still_valid(verified, &self.android_apps, self.status.as_ref(), now_ms)?;
        }
        if let Some(attested) = &enrollment.app_attest {
            if !self
                .apple
                .apps
                .iter()
                .any(|app| app.rp_id_hash() == attested.rp_id_hash)
                || !self
                    .apple
                    .validation_categories
                    .contains(&attested.facts.validation_category)
            {
                return Err(Rejected::App);
            }
            if attested.environment == apple::Environment::Development
                && !self.apple.allow_development
            {
                return Err(Rejected::Environment);
            }
        }
        Ok(())
    }

    /// Verify enrolment evidence bound to this desktop, phone and nonce.
    pub fn verify(
        &self,
        evidence: &Evidence,
        desktop: DeviceId,
        phone: DeviceId,
        nonce: &[u8; 32],
        now_ms: u64,
    ) -> Result<Enrollment, Rejected> {
        let (step_up_key, protection, app_attest, android) = match evidence {
            Evidence::AndroidKeystore { chain } => {
                let leaf_key = der::Certificate::parse(
                    chain
                        .first()
                        .ok_or(Rejected::Chain("certificate chain length"))?,
                )?
                .key
                .to_vec();
                let verified = android::verify(
                    chain,
                    &self.android_anchors,
                    &self.android_apps,
                    self.status.as_ref(),
                    &attestation_challenge(desktop, phone, nonce),
                    &leaf_key,
                    now_ms,
                )?;
                let protection = match verified.level {
                    android::Level::StrongBox => Protection::AndroidStrongBox,
                    android::Level::TrustedEnvironment => Protection::AndroidTrustedEnvironment,
                };
                (leaf_key, protection, None, Some(verified))
            }
            Evidence::AppleAppAttest {
                step_up_key,
                key_id,
                attestation,
            } => {
                if !crate::stepup::valid_key(step_up_key) {
                    return Err(Rejected::WrongKey);
                }
                let verified = apple::verify_attestation(
                    attestation,
                    key_id,
                    &app_attest_client_data_hash(desktop, phone, nonce, step_up_key),
                    &self.apple_anchors,
                    &self.apple_root,
                    &self.apple,
                    now_ms,
                )?;
                // The step-up key must not be the App Attest key itself.
                if verified.key == *step_up_key {
                    return Err(Rejected::WrongKey);
                }
                (
                    step_up_key.clone(),
                    Protection::AppleAppAttested,
                    Some(AppAttestKey {
                        key: verified.key,
                        rp_id_hash: verified.rp_id_hash,
                        environment: verified.environment,
                        counter: 0,
                        facts: verified.facts,
                    }),
                    None,
                )
            }
        };
        Ok(Enrollment {
            step_up_key,
            protection,
            app_attest,
            android,
            test_only: self.test_only,
            enrolled_ms: now_ms,
        })
    }
}

#[cfg(test)]
mod tests;
