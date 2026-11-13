//! Owner step-up from the phone.
//!
//! A P-256 key in the phone's secure hardware signs a single-use desktop
//! challenge bound to one sensitive operation, after the owner authenticates
//! on the phone. Native adapters own the keys; this module carries the
//! protocol so both apps behave the same.
//!
//! Setting up ([`crate::link::PhoneLink::set_up_step_up`]): the desktop
//! issues an enrolment challenge; the native [`StepUpAttester`] creates the
//! key and returns platform evidence bound to it. The desktop verifies that
//! evidence against Google's or Apple's roots and decides the protection
//! level itself; nothing the phone says about its own hardware is trusted.
//!
//! - Android: a Keystore key created with the attestation challenge; its
//!   certificate chain is the evidence.
//! - iOS: a Secure Enclave key, plus an App Attest attestation whose client
//!   data binds that key's public half. Apple attests the app, not the
//!   Secure Enclave key; the desktop records exactly that.
//!
//! A phone without the needed hardware or service gets
//! [`MobileError::Unavailable`]: there is no software fallback.
use crate::MobileError;
use notdot_link::DeviceId;
use sha2::{Digest, Sha256};

/// What the desktop verified about this phone's step-up key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum StepUpProtection {
    /// Google attests the key is in StrongBox behind a strong biometric.
    AndroidStrongBox,
    /// Google attests the key is in the TEE behind a strong biometric.
    AndroidTrustedEnvironment,
    /// Apple attests the genuine app on a real iPhone vouched for the key.
    AppleAppAttested,
}
impl StepUpProtection {
    pub(crate) fn from_wire(value: &serde_json::Value) -> Option<Self> {
        match value.as_str()? {
            "android_strong_box" => Some(Self::AndroidStrongBox),
            "android_trusted_environment" => Some(Self::AndroidTrustedEnvironment),
            "apple_app_attested" => Some(Self::AppleAppAttested),
            _ => None,
        }
    }
}

/// Plain words for a protection level, for settings screens.
#[uniffi::export]
pub fn describe_protection(protection: StepUpProtection) -> String {
    match protection {
        StepUpProtection::AndroidStrongBox => {
            "Google confirmed the key is in this phone's security chip."
        }
        StepUpProtection::AndroidTrustedEnvironment => {
            "Google confirmed the key is in this phone's secure processor."
        }
        StepUpProtection::AppleAppAttested => {
            "Apple confirmed this is the genuine notdot app on a real iPhone."
        }
    }
    .into()
}

/// This phone's sensitive-action state on the linked computer.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct StepUpStatus {
    /// A key is set up and verified by the computer.
    pub enrolled: bool,
    pub protection: Option<StepUpProtection>,
    /// The owner has allowed sensitive actions from this phone, for now.
    pub granted: bool,
    /// Verified against test roots only: never genuine hardware evidence.
    pub test_only: bool,
    /// An App Attest development-environment key (a debuggable build).
    pub development: bool,
    /// SHA-256 of the step-up public key the computer holds, so native
    /// code can tell which of its keys that is (see
    /// [`crate::link::PhoneLink::set_up_step_up`]).
    pub key_sha256: Option<Vec<u8>>,
}

/// SHA-256 of a step-up public key, to compare with
/// [`StepUpStatus::key_sha256`].
#[uniffi::export]
pub fn step_up_key_sha256(public_key: Vec<u8>) -> Vec<u8> {
    Sha256::digest(public_key).to_vec()
}

/// The values a native attester binds into its evidence.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct AttestRequest {
    /// For Android: pass to `setAttestationChallenge`.
    pub android_challenge: Vec<u8>,
    pub desktop: Vec<u8>,
    pub phone: Vec<u8>,
    pub nonce: Vec<u8>,
}

/// Platform evidence for a new step-up key.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum StepUpEvidence {
    /// The new key's attestation certificate chain, leaf first.
    AndroidKeystore { chain: Vec<Vec<u8>> },
    /// The new Secure Enclave key, and an App Attest attestation made with
    /// [`app_attest_client_data_hash`] for it.
    AppleAppAttest {
        step_up_key: Vec<u8>,
        key_id: Vec<u8>,
        attestation: Vec<u8>,
    },
}

/// Creates a step-up key and its evidence. Implemented natively; runs off
/// the main thread and may block (key generation, a request to Apple).
/// Replace any previous step-up key only after the computer accepts the new
/// one (see [`crate::link::PhoneLink::set_up_step_up`]).
#[uniffi::export(with_foreign)]
pub trait StepUpAttester: Send + Sync {
    fn attest(&self, request: AttestRequest) -> Result<StepUpEvidence, MobileError>;
}

/// The `clientDataHash` an iPhone passes to `attestKey` for `step_up_key`.
#[uniffi::export]
pub fn app_attest_client_data_hash(
    request: AttestRequest,
    step_up_key: Vec<u8>,
) -> Result<Vec<u8>, MobileError> {
    let (desktop, phone, nonce) = ids(&request)?;
    Ok(
        notdot_link::attest::app_attest_client_data_hash(desktop, phone, &nonce, &step_up_key)
            .to_vec(),
    )
}

fn ids(request: &AttestRequest) -> Result<(DeviceId, DeviceId, [u8; 32]), MobileError> {
    let bad = || MobileError::Invalid {
        detail: "malformed attestation request".into(),
    };
    Ok((
        DeviceId(request.desktop.as_slice().try_into().map_err(|_| bad())?),
        DeviceId(request.phone.as_slice().try_into().map_err(|_| bad())?),
        request.nonce.as_slice().try_into().map_err(|_| bad())?,
    ))
}

pub(crate) fn attest_request(desktop: DeviceId, phone: DeviceId, nonce: [u8; 32]) -> AttestRequest {
    AttestRequest {
        android_challenge: notdot_link::attest::attestation_challenge(desktop, phone, &nonce)
            .to_vec(),
        desktop: desktop.0.to_vec(),
        phone: phone.0.to_vec(),
        nonce: nonce.to_vec(),
    }
}

impl StepUpEvidence {
    pub(crate) fn into_wire(self) -> Result<notdot_link::attest::Evidence, MobileError> {
        use notdot_link::attest::Evidence;
        Ok(match self {
            StepUpEvidence::AndroidKeystore { chain } => Evidence::AndroidKeystore { chain },
            StepUpEvidence::AppleAppAttest {
                step_up_key,
                key_id,
                attestation,
            } => Evidence::AppleAppAttest {
                step_up_key,
                key_id: key_id
                    .as_slice()
                    .try_into()
                    .map_err(|_| MobileError::Invalid {
                        detail: "App Attest key id must be 32 bytes".into(),
                    })?,
                attestation,
            },
        })
    }
}

/// What a native signer is asked to sign for one action.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SignRequest {
    /// Sign with the step-up key: ECDSA P-256 with SHA-256.
    pub message: Vec<u8>,
    /// iPhone only: pass to `generateAssertion` with the App Attest key.
    pub app_attest_client_data_hash: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct StepUpSignature {
    /// DER (X9.62) or raw r||s.
    pub signature: Vec<u8>,
    /// The App Attest assertion, on iPhones.
    pub assertion: Option<Vec<u8>>,
}

/// Signs step-up challenges with the hardware key. Implemented natively;
/// `sign` is where the phone asks the owner to authenticate, so it is only
/// ever called for an action the owner just confirmed. It runs off the
/// main thread and may block until the owner answers. Return
/// [`MobileError::Cancelled`] when the owner cancels.
#[uniffi::export(with_foreign)]
pub trait StepUpSigner: Send + Sync {
    fn sign(&self, request: SignRequest) -> Result<StepUpSignature, MobileError>;
}

pub(crate) fn sign_request(message: Vec<u8>) -> SignRequest {
    let data = notdot_link::attest::assertion_client_data(&message);
    SignRequest {
        app_attest_client_data_hash: Sha256::digest(data).to_vec(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_carry_the_desktops_binding() {
        let request = attest_request(DeviceId([1; 32]), DeviceId([2; 32]), [3; 32]);
        assert_eq!(
            request.android_challenge,
            notdot_link::attest::attestation_challenge(
                DeviceId([1; 32]),
                DeviceId([2; 32]),
                &[3; 32]
            )
        );
        let hash = app_attest_client_data_hash(request.clone(), vec![4; 65]).unwrap();
        assert_eq!(
            hash,
            notdot_link::attest::app_attest_client_data_hash(
                DeviceId([1; 32]),
                DeviceId([2; 32]),
                &[3; 32],
                &[4; 65]
            )
        );
        let mut short = request;
        short.nonce.pop();
        assert!(app_attest_client_data_hash(short, vec![4; 65]).is_err());
        assert!(
            StepUpEvidence::AppleAppAttest {
                step_up_key: vec![4; 65],
                key_id: vec![0; 31],
                attestation: vec![]
            }
            .into_wire()
            .is_err()
        );
    }
}
