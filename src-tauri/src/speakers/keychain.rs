//! Device key for encrypted voiceprints, held by the OS credential store.
//! Background reads never show keychain UI; a locked or unavailable keychain
//! is reported as an error so locked identity modes fail closed.
use notdot_speakers::store::{Key, KeyStore};
use zeroize::Zeroizing;

const ACCOUNT: &str = "voiceprints";

pub fn service(qa: bool) -> &'static str {
    if qa {
        "notdot-qa-speaker-key"
    } else {
        "notdot-speaker-key"
    }
}

pub struct Keychain {
    service: &'static str,
}
impl Keychain {
    pub fn new(qa: bool) -> Self {
        Self {
            service: service(qa),
        }
    }
    fn entry(&self) -> Result<keyring::Entry, String> {
        keyring::Entry::new(self.service, ACCOUNT).map_err(|_| "Keychain is unavailable".into())
    }
}

#[cfg(target_os = "macos")]
fn quiet<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    use security_framework::os::macos::keychain::SecKeychain;
    let _lock = if SecKeychain::user_interaction_allowed().map_err(|_| "Keychain policy unknown")? {
        Some(SecKeychain::disable_user_interaction().map_err(|_| "Keychain policy unknown")?)
    } else {
        None
    };
    f()
}
#[cfg(not(target_os = "macos"))]
fn quiet<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    f()
}

// Keychain access shares the process-wide credential lock: legacy macOS
// keychain UI control is global, so reads and writes must not interleave.
fn serialised<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let _operation = crate::credentials::OPERATIONS
        .lock()
        .map_err(|_| "Keychain is unavailable".to_owned())?;
    f()
}

impl KeyStore for Keychain {
    fn load(&self) -> Result<Option<Key>, String> {
        serialised(|| {
            quiet(|| match self.entry()?.get_secret() {
                Ok(bytes) => {
                    let bytes = Zeroizing::new(bytes);
                    let key: [u8; 32] = bytes
                        .as_slice()
                        .try_into()
                        .map_err(|_| "Voice key has an unexpected size".to_owned())?;
                    Ok(Some(Zeroizing::new(key)))
                }
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(_) => Err("Keychain is locked or unavailable".into()),
            })
        })
    }
    fn store(&self, key: &Key) -> Result<(), String> {
        serialised(|| {
            self.entry()?
                .set_secret(key.as_ref())
                .map_err(|_| "Could not save the voice key".into())
        })
    }
    fn delete(&self) -> Result<(), String> {
        serialised(|| match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err("Could not remove the voice key".into()),
        })
    }
}
