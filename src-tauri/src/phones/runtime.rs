use super::{GRANT_MINUTES, PhoneSettings, status_list};
use crate::AppState;
use notdot_core::profiles::{ProfileId, Role};
use notdot_link::{
    DeviceId,
    attest::{AttestationPolicy, android::AndroidApp, apple::AppleApp},
    pairing::{DeviceProfile, ProfileRef},
    protocol::OpError,
    service::{Backend, BoxFuture, Settings},
    store::JsonFile,
    transport::{Desktop, DesktopOptions, NetConfig, SecretKey},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Weak},
};
use tokio::sync::{Mutex, watch};

pub struct Phones {
    dir: PathBuf,
    settings_file: Result<JsonFile, String>,
    inner: Mutex<Inner>,
    refresh_gate: Mutex<()>,
    transitions: Mutex<()>,
}
struct Inner {
    settings: PhoneSettings,
    desktop: Option<Desktop>,
    list: Option<notdot_link::attest::android::StatusList>,
    list_state: status_list::ListState,
    error: Option<String>,
    worker: Option<tokio::task::JoinHandle<()>>,
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn bytes<const N: usize>(text: &str) -> Result<[u8; N], String> {
    if text.len() != 2 * N || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Invalid identifier".into());
    }
    let mut out = [0; N];
    for (i, value) in out.iter_mut().enumerate() {
        *value =
            u8::from_str_radix(&text[2 * i..2 * i + 2], 16).map_err(|_| "Invalid identifier")?;
    }
    Ok(out)
}
fn policy(inner: &Inner) -> AttestationPolicy {
    let mut policy = AttestationPolicy::default();
    policy.android_apps = inner
        .settings
        .android_apps
        .iter()
        .map(|app| AndroidApp {
            package: app.package.clone(),
            signers: app.signers.iter().filter_map(|s| bytes(s).ok()).collect(),
        })
        .collect();
    policy.apple.apps = inner
        .settings
        .apple_apps
        .iter()
        .map(|app| AppleApp {
            team_id: app.team_id.clone(),
            bundle_id: app.bundle_id.clone(),
        })
        .collect();
    policy.apple.allow_development = inner.settings.apple_development;
    policy.apple.validation_categories = inner.settings.apple_categories.clone();
    if let Some(list) = &inner.list {
        policy.set_status_list(list.clone());
    }
    policy
}
impl Phones {
    pub fn open(base: &Path) -> Result<Self, String> {
        let dir = base.join("phone-link");
        notdot_link::store::private_dir(&dir).map_err(|e| e.to_string())?;
        let opened = JsonFile::open::<PhoneSettings>(&dir, "policy.json");
        let (settings_file, settings, error) = match opened {
            Ok((file, settings)) => {
                let settings = settings.unwrap_or_default();
                match settings.validate() {
                    Ok(()) => (Ok(file), settings, None),
                    Err(e) => (Err(e.clone()), PhoneSettings::default(), Some(e)),
                }
            }
            Err(e) => {
                let message = e.to_string();
                (
                    Err(message.clone()),
                    PhoneSettings::default(),
                    Some(message),
                )
            }
        };
        let cached = status_list::load_cached(&dir);
        let mut list_state = status_list::ListState::default();
        if let Some((_, fetched)) = &cached {
            list_state.restore(*fetched);
        }
        Ok(Self {
            dir,
            settings_file,
            refresh_gate: Mutex::new(()),
            transitions: Mutex::new(()),
            inner: Mutex::new(Inner {
                settings,
                desktop: None,
                list: cached.map(|(list, _)| list),
                list_state,
                error,
                worker: None,
            }),
        })
    }
    pub async fn start_if_enabled(&self, state: &Arc<AppState>) {
        let _transition = self.transitions.lock().await;
        let enabled = self.inner.lock().await.settings.enabled;
        if enabled && let Err(e) = self.start(state, None, false).await {
            self.inner.lock().await.error = Some(e);
        }
    }
    async fn start(
        &self,
        state: &Arc<AppState>,
        secret: Option<SecretKey>,
        interactive: bool,
    ) -> Result<(), String> {
        let mut inner = self.inner.lock().await;
        if inner.desktop.is_some() {
            return Ok(());
        }
        let secret = match secret {
            Some(key) => key,
            None => {
                let dir = self.dir.clone();
                tokio::task::spawn_blocking(move || identity(&dir, interactive))
                    .await
                    .map_err(|e| e.to_string())??
            }
        };
        let weak = Arc::downgrade(state);
        let epochs = weak.clone();
        let desktop = Desktop::bind_with(
            Some(secret),
            &NetConfig {
                relays: None,
                direct: true,
                insecure_test_relay: false,
                relay_roots: vec![],
            },
            notdot_link::service::system_clock(),
            DesktopOptions {
                backend: Some(Arc::new(AppBackend(weak))),
                state_dir: Some(self.dir.clone()),
                epochs: Some(Arc::new(move |id| {
                    let s = epochs.upgrade()?;
                    let roster = s.speakers.roster().ok()?;
                    if roster.recovery_required() {
                        return None;
                    }
                    Some(roster.profile(&ProfileId::parse(id).ok()?)?.epoch)
                })),
            },
        )
        .await
        .map_err(|e| e.to_string())?;
        desktop.set_attestation_policy(policy(&inner));
        inner.desktop = Some(desktop);
        inner.error = None;
        let weak = Arc::downgrade(state);
        inner.worker = Some(tokio::spawn(async move {
            loop {
                let Some(s) = weak.upgrade() else {
                    break;
                };
                if s.phones
                    .inner
                    .lock()
                    .await
                    .list_state
                    .due(status_list::now_ms())
                {
                    s.phones.refresh_now().await;
                }
                drop(s);
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            }
        }));
        Ok(())
    }
    pub async fn stop(&self) {
        let _transition = self.transitions.lock().await;
        self.stop_inner().await;
    }
    async fn stop_inner(&self) {
        let mut inner = self.inner.lock().await;
        if let Some(worker) = inner.worker.take() {
            worker.abort();
        }
        if let Some(desktop) = inner.desktop.take() {
            desktop.close().await;
        }
    }
    pub async fn set_enabled(&self, state: &Arc<AppState>, enabled: bool) -> Result<(), String> {
        let _transition = self.transitions.lock().await;
        let mut settings = self.inner.lock().await.settings.clone();
        settings.enabled = enabled;
        self.apply_settings(state, settings, enabled).await
    }
    pub async fn set_settings(
        &self,
        state: &Arc<AppState>,
        mut settings: PhoneSettings,
    ) -> Result<(), String> {
        let _transition = self.transitions.lock().await;
        // Trust edits must not resurrect an earlier connection toggle.
        settings.enabled = self.inner.lock().await.settings.enabled;
        self.apply_settings(state, settings, false).await
    }
    async fn apply_settings(
        &self,
        state: &Arc<AppState>,
        settings: PhoneSettings,
        interactive: bool,
    ) -> Result<(), String> {
        settings.validate()?;
        {
            let mut inner = self.inner.lock().await;
            // Persist before expanding trust. A write failure keeps the old policy.
            self.settings_file
                .as_ref()
                .map_err(Clone::clone)?
                .save(&settings)
                .map_err(|e| e.to_string())?;
            inner.settings = settings;
            if let Some(desktop) = &inner.desktop {
                desktop.set_attestation_policy(policy(&inner));
            }
        }
        let enabled = self.inner.lock().await.settings.enabled;
        if enabled {
            let result = self.start(state, None, interactive).await;
            if let Err(error) = &result {
                self.inner.lock().await.error = Some(error.clone());
            }
            result
        } else {
            self.stop_inner().await;
            Ok(())
        }
    }
    pub async fn refresh_now(&self) {
        let Ok(_refresh) = self.refresh_gate.try_lock() else {
            return;
        };
        let Ok(client) = status_list::client() else {
            return;
        };
        let mut list_state = self.inner.lock().await.list_state.clone();
        if list_state
            .last_attempt_ms
            .is_some_and(|last| status_list::now_ms().saturating_sub(last) < 60_000)
        {
            return;
        }
        let list = status_list::refresh(&client, &self.dir, &mut list_state).await;
        let mut inner = self.inner.lock().await;
        inner.list_state = list_state;
        if let Some(list) = list {
            inner.list = Some(list);
        }
        if let Some(desktop) = &inner.desktop {
            desktop.set_attestation_policy(policy(&inner));
        }
    }
    pub async fn status(&self, state: &AppState) -> Value {
        let inner = self.inner.lock().await;
        let profiles: Vec<_> = state
            .speakers
            .roster()
            .map(|r| {
                r.profiles()
                    .iter()
                    .map(|p| json!({"id":p.id,"name":p.name}))
                    .collect()
            })
            .unwrap_or_default();
        let mut pending = Vec::new();
        let mut devices = Vec::new();
        if let Some(desktop) = &inner.desktop {
            pending = desktop
                .pending()
                .iter()
                .map(|p| json!({"offer":hex(&p.offer),"name":p.name,"code":p.sas.to_string()}))
                .collect();
            devices = desktop.devices().iter().map(|d| json!({"id":hex(&d.device.0),"name":d.name,"profile":d.profile.id,"owner":d.device_profile == DeviceProfile::Owner,"step_up":desktop.step_up_state(d.device)})).collect();
        }
        json!({"available":true,"running":inner.desktop.is_some(),"settings":inner.settings,"error":inner.error,"profiles":profiles,"pending":pending,"devices":devices,"revocations":inner.list_state,"revocations_fresh":inner.list_state.fresh(status_list::now_ms())})
    }
    pub async fn create_offer(&self, state: &AppState, profile: &str) -> Result<Value, String> {
        let id = ProfileId::parse(profile)?;
        let roster = state.speakers.roster()?;
        let person = roster
            .profile(&id)
            .ok_or("That person is no longer in People")?;
        let inner = self.inner.lock().await;
        let desktop = inner
            .desktop
            .as_ref()
            .ok_or("Turn on phone connections first")?;
        let qr = desktop
            .create_offer(
                ProfileRef {
                    id: profile.into(),
                    epoch: person.epoch,
                },
                if person.role == Role::Owner {
                    DeviceProfile::Owner
                } else {
                    DeviceProfile::Member
                },
                notdot_link::pairing::OFFER_TTL_MS,
            )
            .map_err(|e| e.to_string())?;
        Ok(json!({"code":qr.encode(),"expires_ms":qr.expires_ms}))
    }
    pub async fn decide(&self, offer: &str, allow: bool) -> Result<(), String> {
        let offer = bytes::<16>(offer)?;
        let inner = self.inner.lock().await;
        let desktop = inner.desktop.as_ref().ok_or("Phones are disconnected")?;
        let pending = desktop
            .pending()
            .into_iter()
            .find(|p| p.offer == offer)
            .ok_or("This pairing request has expired")?;
        if allow {
            desktop.approve(&pending).map_err(|e| e.to_string())?;
        } else {
            desktop.deny(&pending);
        }
        Ok(())
    }
    pub async fn remove(&self, device: &str) -> Result<(), String> {
        self.inner
            .lock()
            .await
            .desktop
            .as_ref()
            .ok_or("Turn on phone connections to remove a phone")?
            .revoke(DeviceId(bytes(device)?))
            .map_err(|e| e.to_string())
    }
    pub async fn sensitive(&self, device: &str, allow: bool) -> Result<(), String> {
        let inner = self.inner.lock().await;
        let desktop = inner.desktop.as_ref().ok_or("Phones are disconnected")?;
        let id = DeviceId(bytes(device)?);
        if allow {
            desktop.grant_sensitive(id, status_list::now_ms() + GRANT_MINUTES * 60_000)
        } else {
            desktop.withdraw_sensitive(id)
        }
        .map_err(|e| e.to_string())
    }
}

/// Separate OS item, never a voice provider entry. Background restoration
/// cannot show Keychain UI; failure is visible in Phones, never a new identity.
fn identity(dir: &Path, _interactive: bool) -> Result<SecretKey, String> {
    use base64::Engine;
    let _serial = crate::credentials::OPERATIONS
        .lock()
        .map_err(|_| "Credential store is busy")?;
    #[cfg(target_os = "macos")]
    let _no_ui = if _interactive {
        None
    } else {
        crate::credentials::non_interactive_keychain()?
    };
    let account =
        hex(ring::digest::digest(&ring::digest::SHA256, dir.to_string_lossy().as_bytes()).as_ref());
    let entry = keyring::Entry::new("dev.notdot.phone-link", &account)
        .map_err(|_| "Phone identity store is unavailable")?;
    match entry.get_password() {
        Ok(text) => {
            let raw = zeroize::Zeroizing::new(
                base64::engine::general_purpose::STANDARD
                    .decode(text)
                    .map_err(|_| "Phone identity could not be read")?,
            );
            let raw: [u8; 32] = raw
                .as_slice()
                .try_into()
                .map_err(|_| "Phone identity could not be read")?;
            Ok(SecretKey::from_bytes(&raw))
        }
        Err(keyring::Error::NoEntry) => {
            // Existing pairings must never silently switch desktop identity.
            if dir.join(notdot_link::store::REGISTRY_FILE).exists() {
                return Err(
                    "Allow access to the saved phone identity before reconnecting phones".into(),
                );
            }
            let mut raw = zeroize::Zeroizing::new([0u8; 32]);
            ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), raw.as_mut())
                .map_err(|_| "Phone identity random source unavailable")?;
            let key = SecretKey::from_bytes(&raw);
            let encoded = zeroize::Zeroizing::new(
                base64::engine::general_purpose::STANDARD.encode(key.to_bytes()),
            );
            entry.set_password(&encoded).map_err(
                |_| "Allow access to the phone identity in your computer's credential store",
            )?;
            Ok(key)
        }
        Err(_) => {
            Err("The saved phone identity needs access in your computer's credential store".into())
        }
    }
}

/// Uses the pairing-bound profile, never the active UI/voice profile. The
/// roster is checked again under the submission gate before every write.
struct AppBackend(Weak<AppState>);
fn unavailable(reason: impl ToString) -> OpError {
    OpError::NotAvailable {
        reason: reason.to_string(),
    }
}
impl AppBackend {
    fn state(&self) -> Result<Arc<AppState>, OpError> {
        self.0
            .upgrade()
            .ok_or_else(|| unavailable("notdot is closing"))
    }
    async fn context(
        state: &AppState,
        profile: &str,
    ) -> Result<Arc<crate::profile_context::ProfileContext>, OpError> {
        let id = ProfileId::parse(profile).map_err(|_| OpError::Forbidden)?;
        let settings = state.snapshot.lock().await.settings.clone();
        let roster = state.speakers.roster().map_err(unavailable)?;
        if roster.recovery_required() || roster.profile(&id).is_none() {
            return Err(OpError::Forbidden);
        }
        state
            .profiles
            .get_or_open(&roster, &id, settings.memory_enabled, settings.selection())
            .map_err(unavailable)
    }
}
impl Backend for AppBackend {
    fn add_note(&self, profile: String, text: String) -> BoxFuture<Result<u64, OpError>> {
        let state = self.state();
        Box::pin(async move {
            let state = state?;
            let _gate = state.send_gate.lock().await;
            let context = Self::context(&state, &profile).await?;
            let _turn = context.turn_lock.lock().await;
            context
                .notes
                .add_from_phone(&text)
                .await
                .map_err(unavailable)
        })
    }
    fn list_notes(&self, profile: String) -> BoxFuture<Result<Vec<(u64, String)>, OpError>> {
        let state = self.state();
        Box::pin(async move {
            let state = state?;
            let _gate = state.send_gate.lock().await;
            let context = Self::context(&state, &profile).await?;
            let book = context.notes.snapshot().await.map_err(unavailable)?;
            Ok(book
                .notes
                .iter()
                .enumerate()
                .map(|(i, n)| (i as u64 + 1, notdot_core::notes::export_text(&[n])))
                .collect())
        })
    }
    fn read_settings(&self, profile: String) -> BoxFuture<Result<Settings, OpError>> {
        let state = self.state();
        Box::pin(async move {
            let state = state?;
            Self::context(&state, &profile).await?;
            let snap = state.snapshot.lock().await;
            // Explicit public fields. Never serialise Settings wholesale: it
            // contains chat associations and machine-specific runtime paths.
            Ok((
                0,
                BTreeMap::from([
                    ("name".into(), snap.settings.name.clone()),
                    ("hue".into(), snap.settings.hue.to_string()),
                    (
                        "followup_enabled".into(),
                        snap.settings.followup_enabled.to_string(),
                    ),
                ]),
            ))
        })
    }
    fn stage_setting(
        &self,
        _: String,
        _: String,
        _: String,
        _: u64,
    ) -> BoxFuture<Result<u64, OpError>> {
        Box::pin(async {
            Err(unavailable(
                "Change desktop settings on this computer; remote settings proposals are not connected yet",
            ))
        })
    }
    fn run_task(
        &self,
        _: String,
        _: String,
        _: u32,
        _: watch::Receiver<bool>,
    ) -> BoxFuture<Result<Option<Value>, OpError>> {
        Box::pin(async {
            Err(unavailable(
                "Remote Codex tasks are not connected in this mobile prototype",
            ))
        })
    }
    fn reset_memory(&self, profile: String) -> BoxFuture<Result<u64, OpError>> {
        let state = self.state();
        Box::pin(async move {
            let state = state?;
            let _gate = state.send_gate.lock().await;
            let context = Self::context(&state, &profile).await?;
            // Defense in depth: members can never erase owner memories.
            if context.id != state.profiles.owner().id {
                return Err(OpError::Forbidden);
            }
            let _turn = context.turn_lock.lock().await;
            if state.snapshot.lock().await.active_turn.is_some()
                || context.notes.waiting().await
                || context.memory.restore_held()
            {
                return Err(unavailable(
                    "Finish the current turn or restore before erasing memories",
                ));
            }
            context.learning.cancel();
            context
                .memory
                .clear_all()
                .await
                .map_err(|_| OpError::Interrupted)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notdot_core::profiles::Actor;
    use notdot_link::{
        pairing::QrPayload,
        protocol::{Op, PairFrame, RequestId, ServerFrame},
        transport::{Phone, desktop_addr},
    };
    use std::time::Duration;

    async fn start(state: &Arc<AppState>, key: [u8; 32]) {
        state
            .phones
            .inner
            .lock()
            .await
            .list_state
            .restore(status_list::now_ms());
        state
            .phones
            .start(state, Some(SecretKey::from_bytes(&key)), false)
            .await
            .unwrap();
    }
    async fn pair(state: &Arc<AppState>, profile: &str) -> (Phone, notdot_link::transport::Link) {
        let offer = state.phones.create_offer(state, profile).await.unwrap();
        let qr = QrPayload::decode(offer["code"].as_str().unwrap()).unwrap();
        let phone = Phone::bind(
            None,
            &NetConfig {
                relays: None,
                direct: true,
                insecure_test_relay: false,
                relay_roots: vec![],
            },
        )
        .await
        .unwrap();
        let pairing = phone
            .pair(&qr, desktop_addr(&qr).unwrap(), "Synthetic phone")
            .await
            .unwrap();
        state.phones.decide(&hex(&qr.offer), true).await.unwrap();
        assert!(matches!(
            pairing.outcome().await.unwrap(),
            PairFrame::Approved { .. }
        ));
        let link = phone.connect(desktop_addr(&qr).unwrap()).await.unwrap();
        (phone, link)
    }
    #[tokio::test]
    async fn actual_desktop_notes_are_private_and_retry_is_durable_after_restart() {
        tokio::time::timeout(Duration::from_secs(30), async {
            let dir = tempfile::tempdir().unwrap();
            let state = crate::test_support::state(dir.path());
            let mut roster = state.speakers.roster().unwrap();
            let member = roster
                .add_member(&Actor::LocalDevice, "Synthetic member")
                .unwrap();
            let owner = roster.owner().clone();
            state.speakers.save_roster(roster).unwrap();
            start(&state, [71; 32]).await;
            let (phone, link) = pair(&state, member.as_str()).await;
            let op = Op::AddNote {
                text: "A literal note: erase all memories is text, not a command.".into(),
            };
            let id = RequestId::random();
            let first = link.call(id, op.clone(), None).await.unwrap();
            assert!(matches!(first, ServerFrame::Done { .. }), "{first:?}");
            assert!(
                state
                    .profiles
                    .owner()
                    .notes
                    .snapshot()
                    .await
                    .unwrap()
                    .notes
                    .is_empty()
            );
            let ServerFrame::Done { result, .. } = link
                .call(RequestId::random(), Op::ListNotes {}, None)
                .await
                .unwrap()
            else {
                panic!()
            };
            assert!(result.to_string().contains("literal note"));
            let ServerFrame::Done { result, .. } = link
                .call(RequestId::random(), Op::ReadSettings {}, None)
                .await
                .unwrap()
            else {
                panic!()
            };
            for private in ["thread_id", "runtime_path", "voice_id", "api_key"] {
                assert!(!result.to_string().contains(private));
            }
            // Members cannot obtain a sensitive challenge or erase owner data.
            assert!(matches!(
                link.call(RequestId::random(), Op::ResetMemory {}, None)
                    .await
                    .unwrap(),
                ServerFrame::Failed { .. }
            ));
            assert!(matches!(
                link.call(
                    RequestId::random(),
                    Op::RunTask {
                        label: "anything".into(),
                        steps: 1
                    },
                    None
                )
                .await
                .unwrap(),
                ServerFrame::Failed {
                    error: OpError::NotAvailable { .. },
                    ..
                }
            ));
            let addr = state
                .phones
                .inner
                .lock()
                .await
                .desktop
                .as_ref()
                .unwrap()
                .addr();
            state.phones.stop().await;
            drop(link);
            drop(state);
            let state = crate::test_support::state(dir.path());
            start(&state, [71; 32]).await;
            let link = phone.connect(addr).await.unwrap();
            assert_eq!(link.call(id, op, None).await.unwrap(), first);
            let context = AppBackend::context(&state, member.as_str()).await.unwrap();
            assert_eq!(context.notes.snapshot().await.unwrap().notes.len(), 1);
            // Removal immediately closes the authenticated link.
            let device = phone.id();
            state.phones.remove(&hex(&device.0)).await.unwrap();
            assert!(
                link.call(RequestId::random(), Op::ListNotes {}, None)
                    .await
                    .is_err()
            );
            assert!(
                state
                    .profiles
                    .owner()
                    .notes
                    .snapshot()
                    .await
                    .unwrap()
                    .notes
                    .is_empty()
            );
            assert_eq!(state.profiles.owner().id, owner);
            state.phones.stop().await;
            phone.close().await;
        })
        .await
        .expect("bounded desktop link lifecycle");
    }
    #[tokio::test]
    async fn policy_survives_restart_but_invalid_or_newer_policy_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let state = crate::test_support::state(dir.path());
        let settings = PhoneSettings {
            android_apps: vec![super::super::AndroidAppSetting {
                package: "dev.example.phone".into(),
                signers: vec!["ab".repeat(32)],
            }],
            ..Default::default()
        };
        state
            .phones
            .set_settings(&state, settings.clone())
            .await
            .unwrap();
        assert_eq!(
            Phones::open(dir.path())
                .unwrap()
                .inner
                .lock()
                .await
                .settings,
            settings
        );
        let mut stale = settings.clone();
        stale.enabled = true;
        state.phones.set_settings(&state, stale).await.unwrap();
        assert!(
            !Phones::open(dir.path())
                .unwrap()
                .inner
                .lock()
                .await
                .settings
                .enabled,
            "a stale policy form must never re-enable phone connections"
        );
        let path = dir.path().join("phone-link/policy.json");
        let original = std::fs::read(&path).unwrap();
        let mut invalid = settings;
        invalid.android_apps[0].signers.clear();
        assert!(state.phones.set_settings(&state, invalid).await.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        std::fs::write(&path, br#"{"version":999,"generation":1,"data":{}}"#).unwrap();
        assert!(
            Phones::open(dir.path())
                .unwrap()
                .inner
                .lock()
                .await
                .error
                .is_some()
        );
        assert!(std::fs::read_to_string(path).unwrap().contains("999"));
    }
    #[tokio::test]
    async fn sensitive_backend_erases_only_memories_and_invalidates_learning() {
        let dir = tempfile::tempdir().unwrap();
        let state = crate::test_support::state(dir.path());
        let owner = state.profiles.owner();
        owner.memory.set_learning_enabled(true).await;
        owner
            .memory
            .remember("Synthetic durable fact", "fixture")
            .await
            .unwrap();
        owner
            .notes
            .add_from_phone("Keep this literal note")
            .await
            .unwrap();
        let ticket = owner.memory.learning_ticket();
        let backend = AppBackend(Arc::downgrade(&state));
        assert_eq!(backend.reset_memory(owner.id.to_string()).await.unwrap(), 1);
        assert!(owner.memory.review("").await.unwrap().is_empty());
        assert_ne!(ticket, owner.memory.learning_ticket());
        assert_eq!(owner.notes.snapshot().await.unwrap().notes.len(), 1);
        assert_eq!(backend.reset_memory(owner.id.to_string()).await.unwrap(), 0);
    }
    #[tokio::test]
    async fn real_memory_handler_needs_attestation_and_grant_and_executes_once_over_quic() {
        use notdot_link::{
            attest::{attestation_challenge, testing::TestRoots},
            protocol::StepUpProof,
            stepup::{message, op_digest},
        };
        use p256::ecdsa::{Signature, signature::Signer};
        tokio::time::timeout(Duration::from_secs(30), async {
            let dir = tempfile::tempdir().unwrap();
            let state = crate::test_support::state(dir.path());
            let owner = state.profiles.owner();
            owner.memory.set_learning_enabled(true).await;
            owner
                .memory
                .remember("Synthetic private preference", "fixture")
                .await
                .unwrap();
            start(&state, [77; 32]).await;
            let (phone, link) = pair(&state, owner.id.as_str()).await;
            assert!(matches!(
                link.call(RequestId::random(), Op::ResetMemory {}, None)
                    .await
                    .unwrap(),
                ServerFrame::Failed { .. }
            ));
            assert_eq!(owner.memory.review("").await.unwrap().len(), 1);
            let now = status_list::now_ms();
            let roots = TestRoots::new(now);
            let desktop_id = {
                let inner = state.phones.inner.lock().await;
                let d = inner.desktop.as_ref().unwrap();
                d.set_attestation_policy(roots.policy(now));
                d.id()
            };
            let nonce = |frame: ServerFrame| {
                let ServerFrame::Done { result, .. } = frame else {
                    panic!("{frame:?}")
                };
                let v = notdot_link::b64_decode(result["nonce"].as_str().unwrap()).unwrap();
                <[u8; 32]>::try_from(v).unwrap()
            };
            let enrol_nonce = nonce(
                link.call(RequestId::random(), Op::StepUpEnrollChallenge {}, None)
                    .await
                    .unwrap(),
            );
            let (key, evidence) =
                roots.android_evidence(attestation_challenge(desktop_id, phone.id(), &enrol_nonce));
            assert!(matches!(
                link.call(
                    RequestId::random(),
                    Op::StepUpEnroll {
                        nonce: enrol_nonce,
                        evidence
                    },
                    None
                )
                .await
                .unwrap(),
                ServerFrame::Done { .. }
            ));
            let digest = op_digest(&Op::ResetMemory {});
            assert!(matches!(
                link.call(RequestId::random(), Op::StepUpChallenge { digest }, None)
                    .await
                    .unwrap(),
                ServerFrame::Failed {
                    error: OpError::Forbidden,
                    ..
                }
            ));
            state
                .phones
                .sensitive(&hex(&phone.id().0), true)
                .await
                .unwrap();
            let action_nonce = nonce(
                link.call(RequestId::random(), Op::StepUpChallenge { digest }, None)
                    .await
                    .unwrap(),
            );
            let signature: Signature =
                key.sign(&message(desktop_id, phone.id(), &action_nonce, &digest));
            let proof = StepUpProof {
                nonce: action_nonce,
                signature: signature.to_der().as_bytes().to_vec(),
                assertion: None,
            };
            let id = RequestId::random();
            let done = link
                .call(id, Op::ResetMemory {}, Some(proof.clone()))
                .await
                .unwrap();
            assert!(matches!(done, ServerFrame::Done { .. }), "{done:?}");
            assert!(owner.memory.review("").await.unwrap().is_empty());
            owner
                .memory
                .remember("New fact after the erase", "fixture")
                .await
                .unwrap();
            assert_eq!(
                link.call(id, Op::ResetMemory {}, Some(proof.clone()))
                    .await
                    .unwrap(),
                done
            );
            assert_eq!(
                owner.memory.review("").await.unwrap().len(),
                1,
                "retry must not erase new data"
            );
            assert!(matches!(
                link.call(RequestId::random(), Op::ResetMemory {}, Some(proof))
                    .await
                    .unwrap(),
                ServerFrame::Failed { .. }
            ));
            state.phones.stop().await;
            phone.close().await;
        })
        .await
        .expect("bounded sensitive-action lifecycle");
    }
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "explicit silent OS-vault smoke test with a disposable identity"]
    fn silent_os_identity_round_trip_never_uses_voice_keys() {
        let dir = tempfile::tempdir().unwrap();
        let account = hex(ring::digest::digest(
            &ring::digest::SHA256,
            dir.path().to_string_lossy().as_bytes(),
        )
        .as_ref());
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _serial = crate::credentials::OPERATIONS.lock().unwrap();
                let _quiet = crate::credentials::non_interactive_keychain().unwrap();
                let _ = keyring::Entry::new("dev.notdot.phone-link", &self.0)
                    .unwrap()
                    .delete_credential();
            }
        }
        let _cleanup = Cleanup(account);
        let first = identity(dir.path(), false).expect("silent disposable identity creation");
        let second = identity(dir.path(), false).expect("silent identity restoration");
        assert!(
            first.to_bytes() == second.to_bytes(),
            "identity changed across reads"
        );
    }
}
