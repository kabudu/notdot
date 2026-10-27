//! OS-protected persistence plus a native-only session cache. Background work
//! must never request authentication UI. Stored secrets are never returned through Tauri IPC.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use zeroize::Zeroizing;
// Legacy macOS keychain UI control is process-wide. Serialize all credential
// reads/writes across vault instances, so an explicit approval cannot race a
// scoped non-interactive read. The cache lock stays separate for UI snapshots.
static OPERATIONS: Mutex<()> = Mutex::new(());
#[derive(Default)]
pub struct Credentials {
    cache: Mutex<HashMap<String, Zeroizing<String>>>,
    read_only: bool,
}
impl Credentials {
    pub fn read_only() -> Self {
        Self {
            read_only: true,
            ..Self::default()
        }
    }
    pub fn cached(&self, provider: &str) -> bool {
        self.cache
            .lock()
            .expect("credential cache")
            .contains_key(provider)
    }
    fn read_with(
        &self,
        provider: &str,
        load: impl FnOnce() -> Result<String, String>,
    ) -> Result<String, String> {
        let _operation = OPERATIONS.lock().expect("credential operation");
        if let Some(key) = self.cache.lock().expect("credential cache").get(provider) {
            return Ok(key.to_string());
        }
        let key = Zeroizing::new(load()?);
        if key.is_empty() || key.len() > 512 {
            return Err("The saved voice key is invalid; update it in Settings".into());
        }
        self.cache
            .lock()
            .expect("credential cache")
            .insert(provider.into(), key.clone());
        Ok(key.to_string())
    }
    pub async fn read(
        self: &Arc<Self>,
        provider: &str,
        interactive: bool,
    ) -> Result<String, String> {
        let vault = self.clone();
        let provider = provider.to_string();
        tokio::task::spawn_blocking(move || {
            vault.read_with(&provider, || os_read(&provider, interactive))
        })
        .await
        .map_err(|e| e.to_string())?
    }
    fn write_with(
        &self,
        provider: &str,
        key: &str,
        save: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        let _operation = OPERATIONS.lock().expect("credential operation");
        save()?;
        self.cache
            .lock()
            .expect("credential cache")
            .insert(provider.into(), Zeroizing::new(key.to_string()));
        Ok(())
    }
    pub async fn save(self: &Arc<Self>, provider: &str, key: &str) -> Result<(), String> {
        if self.read_only {
            return Err("QA profile cannot change OS credentials. Use Authorize saved key for read-only access.".into());
        }
        let vault = self.clone();
        let provider = provider.to_string();
        let key = Zeroizing::new(key.trim().to_string());
        if key.is_empty() || key.len() > 512 {
            return Err("Enter a valid API key".into());
        }
        tokio::task::spawn_blocking(move || {
            vault.write_with(&provider, &key, || {
                super::credential_for(&provider)?
                    .set_password(&key)
                    .map_err(|_| "Could not save voice key to OS keychain".into())
            })
        })
        .await
        .map_err(|e| e.to_string())?
    }
    fn remove_with(
        &self,
        provider: &str,
        delete: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        let _operation = OPERATIONS.lock().expect("credential operation");
        delete()?;
        self.cache
            .lock()
            .expect("credential cache")
            .remove(provider);
        Ok(())
    }
    pub async fn remove(self: &Arc<Self>, provider: &str) -> Result<(), String> {
        if self.read_only {
            return Err("QA profile cannot remove OS credentials".into());
        }
        let vault = self.clone();
        let provider = provider.to_string();
        tokio::task::spawn_blocking(move || {
            vault.remove_with(&provider, || {
                match super::credential_for(&provider)?.delete_credential() {
                    Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                    Err(_) => Err("Could not remove voice key from OS keychain".into()),
                }
            })
        })
        .await
        .map_err(|e| e.to_string())?
    }
}
#[cfg(target_os = "macos")]
fn non_interactive_keychain()
-> Result<Option<security_framework::os::macos::keychain::KeychainUserInteractionLock>, String> {
    use security_framework::os::macos::keychain::SecKeychain;
    if SecKeychain::user_interaction_allowed().map_err(|_| "Could not verify keychain UI policy")? {
        Ok(Some(SecKeychain::disable_user_interaction().map_err(
            |_| "Could not prevent keychain authentication UI",
        )?))
    } else {
        Ok(None)
    }
}
fn os_read(provider: &str, interactive: bool) -> Result<String, String> {
    let entry = super::credential_for(provider)?;
    if interactive {
        return entry.get_password().map_err(|_|"Voice key access was not granted. Authorize the saved key in Settings, or enter a new key.".into());
    }
    #[cfg(target_os = "macos")]
    {
        use security_framework::item::{ItemClass, ItemSearchOptions, SearchResult};
        use security_framework::os::macos::keychain::{SecKeychain, SecPreferencesDomain};
        // kSecUseAuthenticationUISkip alone is ignored by some legacy reads.
        // Preserve the existing process UI state and restore it on every exit.
        let _no_ui = non_interactive_keychain()?;
        let keychain = SecKeychain::default_for_domain(SecPreferencesDomain::User)
            .map_err(|_| "The login keychain is unavailable".to_string())?;
        let values = ItemSearchOptions::new()
            .keychains(&[keychain])
            .class(ItemClass::generic_password())
            .service("com.kabudu.notdot")
            .account(provider)
            .load_data(true)
            .skip_authenticated_items(true)
            .search()
            .map_err(|_| "Saved voice key needs authorization in Settings".to_string())?;
        for value in values {
            if let SearchResult::Data(bytes) = value {
                return String::from_utf8(bytes)
                    .map_err(|_| "Saved voice key is not valid text".into());
            }
        }
        Err("Saved voice key needs authorization in Settings".into())
    }
    #[cfg(target_os = "windows")]
    {
        entry
            .get_password()
            .map_err(|_| "Saved voice key is unavailable in Windows Credential Manager".into())
    }
    #[cfg(target_os = "linux")]
    {
        // Reuse keyring's exact search attributes, but never invoke Unlock or a
        // Secret Service prompt. A lock racing GetSecret simply returns an error.
        let credential = entry
            .get_credential()
            .downcast_ref::<keyring::secret_service::SsCredential>()
            .ok_or("Unsupported Linux credential store")?;
        let service =
            dbus_secret_service::SecretService::connect(dbus_secret_service::EncryptionType::Plain)
                .map_err(|_| "Linux secret service is unavailable")?;
        let mut attributes = HashMap::new();
        for key in ["service", "username", "target"] {
            if let Some(value) = credential.attributes.get(key) {
                attributes.insert(key, value.as_str());
            }
        }
        let matches = service
            .search_items(attributes)
            .map_err(|_| "Saved voice key is unavailable")?;
        if !matches.locked.is_empty() || matches.unlocked.len() != 1 {
            return Err("Saved voice key needs authorization in Settings".into());
        }
        let bytes = matches.unlocked[0]
            .get_secret()
            .map_err(|_| "Saved voice key needs authorization in Settings")?;
        String::from_utf8(bytes).map_err(|_| "Saved voice key is not valid text".into())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = entry;
        Err("Authorize the saved voice key in Settings".into())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[tokio::test]
    async fn qa_profile_rejects_credential_mutation_before_any_os_access() {
        let vault = Arc::new(Credentials::read_only());
        assert!(
            vault
                .save("elevenlabs", "synthetic")
                .await
                .unwrap_err()
                .contains("QA profile")
        );
        assert!(
            vault
                .remove("elevenlabs")
                .await
                .unwrap_err()
                .contains("QA profile")
        );
        assert!(!vault.cached("elevenlabs"));
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn silent_keychain_scope_restores_prior_policy_and_preserves_an_existing_no_ui_scope() {
        use security_framework::os::macos::keychain::SecKeychain;
        let _operation = OPERATIONS.lock().unwrap();
        let original = SecKeychain::user_interaction_allowed().unwrap();
        {
            let _guard = non_interactive_keychain().unwrap();
            assert!(!SecKeychain::user_interaction_allowed().unwrap());
        }
        assert_eq!(SecKeychain::user_interaction_allowed().unwrap(), original);
        if original {
            let outer = SecKeychain::disable_user_interaction().unwrap();
            {
                let guard = non_interactive_keychain().unwrap();
                assert!(guard.is_none());
            }
            assert!(!SecKeychain::user_interaction_allowed().unwrap());
            drop(outer);
            assert!(SecKeychain::user_interaction_allowed().unwrap());
        }
    }
    #[test]
    fn concurrent_reads_authorize_once_and_replacements_and_deletion_invalidate_cache() {
        let vault = Arc::new(Credentials::default());
        let calls = Arc::new(AtomicUsize::new(0));
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let vault = vault.clone();
                let calls = calls.clone();
                scope.spawn(move || {
                    assert_eq!(
                        vault
                            .read_with("a", || {
                                calls.fetch_add(1, Ordering::SeqCst);
                                Ok("test-key".into())
                            })
                            .unwrap(),
                        "test-key"
                    )
                });
            }
        });
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(!vault.cached("b"));
        vault.write_with("a", "replacement", || Ok(())).unwrap();
        assert_eq!(
            vault
                .read_with("a", || panic!("must reuse authorized session"))
                .unwrap(),
            "replacement"
        );
        assert!(
            vault
                .write_with("a", "bad", || Err("denied".into()))
                .is_err()
        );
        assert_eq!(vault.read_with("a", || panic!()).unwrap(), "replacement");
        vault.remove_with("a", || Ok(())).unwrap();
        assert!(!vault.cached("a"));
        assert!(
            vault
                .read_with("a", || Err("authorization required".into()))
                .is_err()
        );
        assert!(!vault.cached("a"));
    }
    #[test]
    fn a_restart_does_not_persist_secrets_or_turn_a_denial_into_access() {
        let vault = Credentials::default();
        assert!(vault.read_with("a", || Err("denied".into())).is_err());
        assert!(!vault.cached("a"));
        vault.read_with("a", || Ok("session".into())).unwrap();
        assert!(vault.cached("a"));
        drop(vault);
        assert!(!Credentials::default().cached("a"));
    }
}
