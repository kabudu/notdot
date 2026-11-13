//! Owner step-up from the phone: a P-256 key in the phone's secure
//! hardware signs a single-use desktop challenge bound to one sensitive
//! operation, after the owner authenticates on the phone.
//!
//! Native adapters own the key (iOS: Secure Enclave; Android: Keystore,
//! StrongBox first, then the trusted execution environment). They report
//! where the key actually lives, and this module refuses a software key:
//! there is no silent downgrade. A phone without secure hardware pairs
//! without step-up, and sensitive actions stay unavailable on it.
//!
//! The protection level is the phone's own report. The desktop does not
//! verify it (that needs platform key attestation on a real device), so it
//! is shown to the owner on the phone, not trusted by the desktop.
use crate::MobileError;

/// Where a step-up key's private half lives, as the platform reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum KeyProtection {
    /// iOS Secure Enclave.
    SecureEnclave,
    /// Android StrongBox (a separate secure element).
    StrongBox,
    /// Android trusted execution environment.
    TrustedEnvironment,
    /// Ordinary software storage. Never accepted for step-up.
    Software,
}

impl KeyProtection {
    pub fn is_hardware(self) -> bool {
        !matches!(self, Self::Software)
    }
}

/// A step-up public key and where its private half lives.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct StepUpKey {
    /// SEC1 P-256 public key (65 bytes uncompressed, or 33 compressed).
    pub public_key: Vec<u8>,
    pub protection: KeyProtection,
}

/// Signs step-up challenges with the hardware key. Implemented natively;
/// `sign` is where the phone asks the owner to authenticate (Face ID,
/// Touch ID, fingerprint or passcode), so it is only ever called for an
/// action the owner just started. It runs off the main thread and may
/// block until the owner answers.
#[uniffi::export(with_foreign)]
pub trait StepUpSigner: Send + Sync {
    /// ECDSA P-256 with SHA-256 over `message`; DER (X9.62) or raw r||s.
    fn sign(&self, message: Vec<u8>) -> Result<Vec<u8>, MobileError>;
}

/// Check a key before pairing with it: well formed and hardware-backed.
#[uniffi::export]
pub fn check_step_up_key(key: StepUpKey) -> Result<(), MobileError> {
    if !key.protection.is_hardware() {
        return Err(MobileError::Invalid {
            detail:
                "Step-up needs a key in this phone's secure hardware; a software key is not used."
                    .into(),
        });
    }
    if !notdot_link::stepup::valid_key(&key.public_key) {
        return Err(MobileError::Invalid {
            detail: "The step-up key is not a P-256 public key.".into(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn software_or_malformed_keys_are_refused() {
        let public = p256::ecdsa::SigningKey::from_slice(&[7; 32])
            .unwrap()
            .verifying_key()
            .to_encoded_point(false)
            .as_bytes()
            .to_vec();
        for protection in [
            KeyProtection::SecureEnclave,
            KeyProtection::StrongBox,
            KeyProtection::TrustedEnvironment,
        ] {
            check_step_up_key(StepUpKey {
                public_key: public.clone(),
                protection,
            })
            .unwrap();
        }
        assert!(
            check_step_up_key(StepUpKey {
                public_key: public.clone(),
                protection: KeyProtection::Software,
            })
            .is_err()
        );
        for bad in [Vec::new(), vec![4; 65], public[..64].to_vec()] {
            assert!(
                check_step_up_key(StepUpKey {
                    public_key: bad,
                    protection: KeyProtection::SecureEnclave,
                })
                .is_err()
            );
        }
    }
}
