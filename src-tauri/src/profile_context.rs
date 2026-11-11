//! Private state is selected once at admission, then retained by `Arc` for the
//! entire operation. Chat grants never select a different person's memory.
use crate::{initiative::Initiative, learning::Learning};
use notdot_core::{
    bridge::Bridge,
    memory::Memory,
    models::Selection,
    profiles::{MAX_PROFILES, ProfileId, Roster, TurnLock},
    session_runtime::SessionRuntime,
};
use std::{
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex as SyncMutex},
};
use tokio::sync::Mutex;

pub struct ProfileContext {
    pub id: ProfileId,
    pub memory: Arc<Memory>,
    pub learning: Learning,
    pub initiative: Initiative,
    /// User-authored notes and lists; never part of memory or chat context.
    pub notes: crate::notes::Notes,
    pub bridge: Mutex<Option<Arc<Bridge>>>,
    pub session_runtime: Mutex<Option<Arc<SessionRuntime>>>,
    pub session_outcomes: Mutex<VecDeque<String>>,
    pub session_consent: SyncMutex<notdot_core::session_control::Consent>,
    pub settings_consent: SyncMutex<notdot_core::voice_settings::Consent>,
    /// In-memory companion association; durable member metadata belongs to Roster.
    pub thread_id: Mutex<Option<String>>,
    pub turn_lock: Mutex<()>,
}

struct ContextPaths<'a> {
    memory: &'a Path,
    state: &'a Path,
    analysis: &'a Path,
    reminders: &'a Path,
    notes: &'a Path,
}
impl ProfileContext {
    fn open(
        id: ProfileId,
        paths: ContextPaths<'_>,
        runtime: &Path,
        enabled: bool,
        selection: Selection,
        thread: Option<String>,
    ) -> Result<Self, String> {
        for path in [paths.memory, paths.state, paths.analysis] {
            private_directory(path)?;
        }
        let memory = Arc::new(Memory::open_with_learning(paths.memory, enabled)?);
        let initiative = Initiative::open(paths.reminders.to_owned())?;
        let notes = crate::notes::Notes::open(paths.notes.to_owned());
        let learning = Learning::new(
            memory.clone(),
            paths.analysis.to_owned(),
            runtime.to_owned(),
        );
        learning.configure(selection);
        Ok(Self {
            id,
            memory,
            learning,
            initiative,
            notes,
            bridge: Mutex::new(None),
            session_runtime: Mutex::new(None),
            session_outcomes: Mutex::new(VecDeque::new()),
            session_consent: Default::default(),
            settings_consent: Default::default(),
            thread_id: Mutex::new(thread),
            turn_lock: Mutex::new(()),
        })
    }
}

/// At most eight open stores, matching the roster limit. No eviction: a turn
/// holding an old Arc must never race a second opening of the same database.
/// Call `get_or_open` only during gated profile admission, off the UI thread.
pub struct ProfileContexts {
    base: PathBuf,
    runtime: PathBuf,
    owner_id: ProfileId,
    owner: Arc<ProfileContext>,
    contexts: SyncMutex<BTreeMap<ProfileId, Arc<ProfileContext>>>,
}
impl ProfileContexts {
    pub fn open(
        base: PathBuf,
        runtime: PathBuf,
        owner_id: ProfileId,
        enabled: bool,
        selection: Selection,
    ) -> Result<Self, String> {
        cleanup_removed(&base, &owner_id)?;
        let owner = Arc::new(ProfileContext::open(
            owner_id.clone(),
            ContextPaths {
                memory: &base.join("memory"),
                state: &base.join("owner"),
                analysis: &base.join("memory-analysis"),
                reminders: &base.join("followups.json"),
                notes: &base.join("owner").join("notes.json"),
            },
            &runtime,
            enabled,
            selection,
            None,
        )?);
        let contexts = SyncMutex::new(BTreeMap::from([(owner_id.clone(), owner.clone())]));
        Ok(Self {
            base,
            runtime,
            owner_id,
            owner,
            contexts,
        })
    }
    /// Close a removed member's live stores and delete their private folder.
    /// Callers record the durable marker with [`record_removal`] *before*
    /// revoking the member in the roster, so a crash at any point is finished
    /// at the next startup. Recording again here keeps direct callers safe.
    pub async fn remove_member(&self, id: &ProfileId) -> Result<(), String> {
        if id == &self.owner_id {
            return Err("The owner cannot be removed".into());
        }
        record_removal(&self.base, id)?;
        let marker = self.base.join("profile-deletions").join(id.as_str());
        let context = self
            .contexts
            .lock()
            .map_err(|_| "Profile contexts unavailable")?
            .get(id)
            .cloned();
        if let Some(context) = context {
            context.initiative.cancel();
            context.learning.shutdown().await?;
            if let Some(bridge) = context.bridge.lock().await.take() {
                bridge.shutdown().await;
            }
            if let Some(runtime) = context.session_runtime.lock().await.take() {
                runtime.shutdown().await;
            }
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
            while Arc::strong_count(&context) > 2 || Arc::strong_count(&context.memory) > 2 {
                if tokio::time::Instant::now() >= deadline {
                    return Err("The person was removed. Private data will be deleted when notdot restarts; an operation is still closing.".into());
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
            self.contexts
                .lock()
                .map_err(|_| "Profile contexts unavailable")?
                .remove(id);
            drop(context);
        }
        delete_member_directory(&self.base, id)?;
        std::fs::remove_file(marker).map_err(|e| format!("Finish profile removal: {e}"))
    }
    pub fn owner(&self) -> Arc<ProfileContext> {
        self.owner.clone()
    }
    pub fn all(&self) -> Result<Vec<Arc<ProfileContext>>, String> {
        Ok(self
            .contexts
            .lock()
            .map_err(|_| "Profile contexts unavailable")?
            .values()
            .cloned()
            .collect())
    }
    pub fn get_or_open(
        &self,
        roster: &Roster,
        id: &ProfileId,
        enabled: bool,
        selection: Selection,
    ) -> Result<Arc<ProfileContext>, String> {
        if roster.owner() != &self.owner_id {
            return Err("Profile owner changed; restart notdot".into());
        }
        let namespace = roster.namespace(&self.base, id).ok_or("Unknown profile")?;
        let mut contexts = self
            .contexts
            .lock()
            .map_err(|_| "Profile contexts unavailable")?;
        if let Some(context) = contexts.get(id) {
            return Ok(context.clone());
        }
        if contexts.len() >= MAX_PROFILES {
            return Err("Restart notdot before opening more profiles".into());
        }
        private_directory(&self.base.join("profiles"))?;
        private_directory(&self.base.join("profiles").join(id.as_str()))?;
        let context = Arc::new(ProfileContext::open(
            id.clone(),
            ContextPaths {
                memory: &namespace.memory_dir,
                state: &namespace.state_dir,
                analysis: &namespace.state_dir.join("memory-analysis"),
                reminders: &namespace.state_dir.join("followups.json"),
                notes: &namespace.state_dir.join("notes.json"),
            },
            &self.runtime,
            enabled,
            selection,
            namespace.companion_thread,
        )?);
        contexts.insert(id.clone(), context.clone());
        Ok(context)
    }
    /// Validate before choosing storage, and again before committing side effects
    /// after an await. The returned context cannot silently change profiles.
    pub fn bind(
        &self,
        roster: &Roster,
        admission: &TurnLock,
        enabled: bool,
        selection: Selection,
    ) -> Result<Arc<ProfileContext>, String> {
        roster
            .validate_lock(admission)
            .map_err(|_| "Speaker permission changed; please try again")?;
        self.get_or_open(roster, &admission.profile, enabled, selection)
    }
}
/// Durably record that a member's removal was authorised. Written before the
/// roster changes; startup completes every recorded removal.
pub fn record_removal(base: &Path, id: &ProfileId) -> Result<(), String> {
    let markers = base.join("profile-deletions");
    private_directory(&markers)?;
    let marker = markers.join(id.as_str());
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(&marker)
        .and_then(|f| f.sync_all())
        .map_err(|e| format!("Record profile removal: {e}"))?;
    // Make the new directory entry itself durable.
    #[cfg(unix)]
    std::fs::File::open(&markers)
        .and_then(|d| d.sync_all())
        .map_err(|e| format!("Record profile removal: {e}"))?;
    Ok(())
}

/// Profiles whose removal was recorded but not finished.
pub fn pending_removals(base: &Path) -> Result<Vec<ProfileId>, String> {
    let markers = base.join("profile-deletions");
    let entries = match std::fs::read_dir(&markers) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("Read profile removals: {e}")),
    };
    Ok(entries
        .flatten()
        .take(64)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            e.file_name()
                .to_str()
                .and_then(|n| ProfileId::parse(n).ok())
        })
        .collect())
}

fn delete_member_directory(base: &Path, id: &ProfileId) -> Result<(), String> {
    private_directory(&base.join("profiles"))?;
    let path = base.join("profiles").join(id.as_str());
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Inspect removed profile: {e}")),
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {
            std::fs::remove_dir_all(path).map_err(|e| format!("Delete removed profile: {e}"))
        }
        Ok(_) => Err("Removed profile directory is not a regular directory".into()),
    }
}
fn cleanup_removed(base: &Path, owner: &ProfileId) -> Result<(), String> {
    let markers = base.join("profile-deletions");
    if !markers.exists() {
        return Ok(());
    }
    private_directory(&markers)?;
    for (index, entry) in std::fs::read_dir(&markers)
        .map_err(|e| e.to_string())?
        .enumerate()
    {
        if index >= 64 {
            return Err("Too many pending profile removals".into());
        }
        let entry = entry.map_err(|e| e.to_string())?;
        // Only removal markers name a profile. Ignore unrelated files, such as
        // Finder metadata, rather than refusing to start; nothing is deleted.
        let Some(id) = entry
            .file_name()
            .to_str()
            .and_then(|name| ProfileId::parse(name).ok())
        else {
            continue;
        };
        if &id == owner {
            return Err("Refusing to remove owner data".into());
        }
        // Delete only once the roster no longer has this person. If finishing
        // the roster change failed, keep their data and the marker for the
        // next attempt instead of leaving an active person without data.
        if let notdot_core::profiles::LoadOutcome::Loaded(roster) =
            notdot_core::profiles::Roster::load(&base.join("profiles.json"), "Owner")
            && roster.profile(&id).is_some()
        {
            continue;
        }
        if !entry.file_type().map_err(|e| e.to_string())?.is_file() {
            return Err("Invalid profile removal marker".into());
        }
        delete_member_directory(base, &id)?;
        std::fs::remove_file(entry.path()).map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn private_directory(path: &Path) -> Result<(), String> {
    // Refuse an existing symlink instead of following it into another profile.
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("Profile directory must not be a symbolic link".into());
    }
    std::fs::create_dir_all(path).map_err(|e| format!("Create profile directory: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("Protect profile directory: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use notdot_core::profiles::{Actor, ChatAccess};
    static STORES: Mutex<()> = Mutex::const_new(());

    #[tokio::test]
    async fn three_person_ledgers_and_chat_grants_remain_isolated_after_restart() {
        use notdot_core::{
            chat_tools::ChatPolicy,
            profiles::{Admission, ChatOp, SpeakerOutcome},
        };
        let _stores = STORES.lock().await;
        let directory = tempfile::tempdir().unwrap();
        let roster_path = directory.path().join("profiles.json");
        let settings_path = directory.path().join("settings.json");
        let mut roster = Roster::new("Synthetic owner").unwrap();
        let first = roster
            .add_member(&Actor::LocalDevice, "First member")
            .unwrap();
        let second = roster
            .add_member(&Actor::LocalDevice, "Second member")
            .unwrap();
        for id in [roster.owner().clone(), first.clone(), second.clone()] {
            roster
                .set_voice_enrolled(&Actor::LocalDevice, &id, true)
                .unwrap();
        }
        roster
            .set_companion_thread(&first, "first-private", Some("owner-private"))
            .unwrap();
        roster
            .set_companion_thread(&second, "second-private", Some("owner-private"))
            .unwrap();
        let settings = notdot_core::settings::Settings {
            thread_id: Some("owner-private".into()),
            ..Default::default()
        };
        settings.save(&settings_path).unwrap();
        let admission = |roster: &Roster, id: &ProfileId| {
            let Admission::Admit(lock) = roster.admit(
                "synthetic-capture",
                &SpeakerOutcome::Recognised {
                    profile: id.clone(),
                    score: 0.99,
                },
            ) else {
                panic!("admission")
            };
            lock
        };
        let first_lock = admission(&roster, &first);
        let policy = ChatPolicy {
            roster_path: roster_path.clone(),
            turn: first_lock.clone(),
            global_mode: notdot_core::session_control::Mode::WhenAsked,
            companion_thread: Some("first-private".into()),
        };
        roster.save(&roster_path).unwrap();
        for chat in ["owner-private", "owner-selected-project", "second-private"] {
            assert!(policy.check(chat, ChatOp::Read).is_err());
        }
        roster
            .grant_chat(
                &Actor::LocalDevice,
                "owner-selected-project",
                &first,
                ChatAccess::Read,
            )
            .unwrap();
        roster.save(&roster_path).unwrap();
        assert!(policy.check("owner-selected-project", ChatOp::Read).is_ok());
        for op in [ChatOp::Send, ChatOp::Steer, ChatOp::Stop] {
            assert!(policy.check("owner-selected-project", op).is_err());
        }
        assert!(policy.check("owner-private", ChatOp::Read).is_err());
        assert!(policy.check("second-private", ChatOp::Read).is_err());
        let expected = [
            "My synthetic canary is owner amber umbrella.",
            "My synthetic canary is first green teapot.",
            "My synthetic canary is second silver kite.",
        ];
        for restarting in [false, true] {
            let contexts = ProfileContexts::open(
                directory.path().into(),
                "unused-runtime".into(),
                roster.owner().clone(),
                true,
                Selection::default(),
            )
            .unwrap();
            let opened = [
                contexts.owner(),
                contexts
                    .bind(
                        &roster,
                        &admission(&roster, &first),
                        true,
                        Selection::default(),
                    )
                    .unwrap(),
                contexts
                    .bind(
                        &roster,
                        &admission(&roster, &second),
                        true,
                        Selection::default(),
                    )
                    .unwrap(),
            ];
            *opened[0].thread_id.lock().await =
                notdot_core::settings::Settings::load(&settings_path)
                    .unwrap()
                    .thread_id;
            let threads = ["owner-private", "first-private", "second-private"];
            for (index, context) in opened.iter().enumerate() {
                assert_eq!(
                    context.thread_id.lock().await.as_deref(),
                    Some(threads[index])
                );
                if !restarting {
                    context
                        .memory
                        .remember(expected[index], threads[index])
                        .await
                        .unwrap();
                }
                let recalled = context
                    .memory
                    .recall_when_enabled(
                        context.memory.learning_ticket().unwrap(),
                        "synthetic canary",
                        6,
                    )
                    .await
                    .unwrap();
                assert_eq!(recalled.len(), 1);
                assert_eq!(recalled[0].content, expected[index]);
                for (other, text) in expected.iter().enumerate() {
                    if other != index {
                        assert!(!serde_json::to_string(&recalled).unwrap().contains(text));
                    }
                }
                for other in opened.iter() {
                    if context.id != other.id {
                        assert!(!Arc::ptr_eq(&context.memory, &other.memory));
                    }
                }
            }
            if restarting {
                roster
                    .revoke_chat(&Actor::LocalDevice, "owner-selected-project", &first)
                    .unwrap();
                roster.save(&roster_path).unwrap();
                assert!(
                    policy
                        .check("owner-selected-project", ChatOp::Read)
                        .is_err()
                );
                assert!(
                    contexts
                        .bind(&roster, &first_lock, true, Selection::default())
                        .is_err(),
                    "revoked epoch cannot bind cached context"
                );
                let fresh = contexts
                    .bind(
                        &roster,
                        &admission(&roster, &first),
                        true,
                        Selection::default(),
                    )
                    .unwrap();
                assert!(Arc::ptr_eq(&fresh, &opened[1]));
                let recalled = fresh
                    .memory
                    .recall_when_enabled(
                        fresh.memory.learning_ticket().unwrap(),
                        "synthetic canary",
                        6,
                    )
                    .await
                    .unwrap();
                assert_eq!(recalled.len(), 1);
                assert_eq!(recalled[0].content, expected[1]);
            }
            // Stop every worker and drop every store Arc before reopening sled.
            for context in &opened {
                context.learning.shutdown().await.unwrap();
            }
            drop(opened);
            drop(contexts);
            let notdot_core::profiles::LoadOutcome::Loaded(reloaded) =
                Roster::load(&roster_path, "unused")
            else {
                panic!("reload roster")
            };
            roster = reloaded;
        }
    }

    #[tokio::test]
    async fn removal_waits_for_captured_turn_then_retry_deletes_private_store() {
        let _stores = STORES.lock().await;
        let directory = tempfile::tempdir().unwrap();
        let mut roster = Roster::new("Owner").unwrap();
        let member = roster.add_member(&Actor::LocalDevice, "Member").unwrap();
        let contexts = ProfileContexts::open(
            directory.path().into(),
            "unused-runtime".into(),
            roster.owner().clone(),
            true,
            Selection::default(),
        )
        .unwrap();
        let held = contexts
            .get_or_open(&roster, &member, true, Selection::default())
            .unwrap();
        held.memory
            .remember("Private synthetic note", "member-thread")
            .await
            .unwrap();
        roster
            .remove_member(&Actor::LocalDevice, directory.path(), &member)
            .unwrap();
        assert!(contexts.remove_member(&member).await.is_err());
        let member_dir = directory.path().join("profiles").join(member.as_str());
        assert!(member_dir.is_dir());
        assert!(
            directory
                .path()
                .join("profile-deletions")
                .join(member.as_str())
                .is_file()
        );
        drop(held);
        contexts.remove_member(&member).await.unwrap();
        assert!(!member_dir.exists());
        assert!(
            !directory
                .path()
                .join("profile-deletions")
                .join(member.as_str())
                .exists()
        );
        assert!(contexts.remove_member(roster.owner()).await.is_err());
        contexts.owner().learning.shutdown().await.unwrap();
    }

    /// A removal recorded before a crash is finished at the next start,
    /// whichever step the crash interrupted, and the owner's data is kept.
    #[tokio::test]
    async fn interrupted_removal_is_finished_at_restart_at_every_crash_point() {
        use crate::speakers::{RemovalRoute, Speakers};
        use notdot_speakers::store::{Key, KeyStore, MemoryKeyStore};
        #[derive(Clone, Default)]
        struct Keys(Arc<MemoryKeyStore>);
        impl KeyStore for Keys {
            fn load(&self) -> Result<Option<Key>, String> {
                self.0.load()
            }
            fn store(&self, key: &Key) -> Result<(), String> {
                self.0.store(key)
            }
            fn delete(&self) -> Result<(), String> {
                self.0.delete()
            }
        }
        let _stores = STORES.lock().await;
        for crash_point in ["after_marker", "after_roster", "after_folder"] {
            let directory = tempfile::tempdir().unwrap();
            let base = directory.path();
            let keys = Keys::default();
            let open = || {
                Speakers::with_keys(
                    base.into(),
                    base.join("m.onnx"),
                    Box::new(keys.clone()),
                    "Owner",
                )
            };
            let speakers = open();
            let mut roster = speakers.roster().unwrap();
            let ada = roster.add_member(&Actor::LocalDevice, "Ada").unwrap();
            roster
                .set_companion_thread(&ada, "ada-private", Some("owner-thread"))
                .unwrap();
            roster
                .grant_chat(&Actor::LocalDevice, "owner-shared", &ada, ChatAccess::Read)
                .unwrap();
            speakers.save_roster(roster).unwrap();
            let ada_dir = base.join("profiles").join(ada.as_str());
            std::fs::create_dir_all(ada_dir.join("memory")).unwrap();
            std::fs::write(ada_dir.join("memory/notes"), "Ada's private note").unwrap();
            std::fs::create_dir_all(base.join("memory")).unwrap();
            std::fs::write(base.join("memory/owner-notes"), "owner note").unwrap();
            // The removal was authorised and recorded, then the app stopped.
            let ticket = speakers
                .authorise_removal(&Actor::LocalDevice, &ada, "Ada", RemovalRoute::Owner)
                .unwrap();
            record_removal(base, &ada).unwrap();
            if crash_point != "after_marker" {
                speakers.remove_member(ticket).unwrap();
            }
            if crash_point == "after_folder" {
                delete_member_directory(base, &ada).unwrap();
            }
            drop(speakers);
            // Restart.
            let speakers = open();
            crate::speakers::resume_offboarding(&speakers).unwrap();
            let roster = speakers.roster().unwrap();
            let contexts = ProfileContexts::open(
                base.into(),
                "unused-runtime".into(),
                roster.owner().clone(),
                true,
                Selection::default(),
            )
            .unwrap();
            assert!(roster.profile(&ada).is_none(), "{crash_point}");
            assert!(roster.grants().is_empty(), "{crash_point}");
            assert!(roster.is_retired("ada-private"), "{crash_point}");
            assert!(!ada_dir.exists(), "{crash_point}");
            assert!(pending_removals(base).unwrap().is_empty(), "{crash_point}");
            assert_eq!(
                std::fs::read_to_string(base.join("memory/owner-notes")).unwrap(),
                "owner note"
            );
            // Re-adding the same name starts fresh.
            let again = speakers.add_member(&Actor::LocalDevice, "Ada").unwrap();
            assert_ne!(again, ada);
            assert!(!base.join("profiles").join(again.as_str()).exists());
            contexts.owner().learning.shutdown().await.unwrap();
        }
    }

    #[test]
    fn cleanup_keeps_data_while_the_roster_still_has_the_person() {
        let directory = tempfile::tempdir().unwrap();
        let base = directory.path();
        let mut roster = Roster::new("Owner").unwrap();
        let ada = roster.add_member(&Actor::LocalDevice, "Ada").unwrap();
        roster.save(&base.join("profiles.json")).unwrap();
        std::fs::create_dir_all(base.join("profiles").join(ada.as_str())).unwrap();
        record_removal(base, &ada).unwrap();
        // Finishing the roster change failed: do not leave an active person
        // without their data; keep the marker for the next attempt.
        cleanup_removed(base, roster.owner()).unwrap();
        assert!(base.join("profiles").join(ada.as_str()).is_dir());
        assert_eq!(pending_removals(base).unwrap(), vec![ada.clone()]);
        // A marker naming the owner is refused outright.
        record_removal(base, roster.owner()).unwrap();
        assert!(cleanup_removed(base, roster.owner()).is_err());
    }

    #[test]
    fn startup_cleanup_removes_only_marked_member_data() {
        let directory = tempfile::tempdir().unwrap();
        let owner = ProfileId::random();
        let removed = ProfileId::random();
        let orphan = ProfileId::random();
        let marker_dir = directory.path().join("profile-deletions");
        std::fs::create_dir_all(&marker_dir).unwrap();
        for id in [&removed, &orphan] {
            std::fs::create_dir_all(directory.path().join("profiles").join(id.as_str())).unwrap();
        }
        std::fs::write(marker_dir.join(removed.as_str()), "").unwrap();
        std::fs::write(marker_dir.join(".DS_Store"), "").unwrap();
        std::fs::create_dir_all(marker_dir.join("not-a-profile")).unwrap();
        cleanup_removed(directory.path(), &owner).unwrap();
        assert!(marker_dir.join(".DS_Store").is_file());
        assert!(!marker_dir.join(removed.as_str()).exists());
        assert!(
            !directory
                .path()
                .join("profiles")
                .join(removed.as_str())
                .exists()
        );
        assert!(
            directory
                .path()
                .join("profiles")
                .join(orphan.as_str())
                .exists()
        );
    }

    #[tokio::test]
    async fn chat_grant_never_shares_private_memory_or_companion_thread() {
        let _stores = STORES.lock().await;
        let directory = tempfile::tempdir().unwrap();
        let mut roster = Roster::new("Owner").unwrap();
        let member = roster.add_member(&Actor::LocalDevice, "Member").unwrap();
        roster
            .set_companion_thread(&member, "member-private-chat", Some("owner-chat"))
            .unwrap();
        roster
            .grant_chat(&Actor::LocalDevice, "owner-chat", &member, ChatAccess::Read)
            .unwrap();
        let contexts = ProfileContexts::open(
            directory.path().into(),
            PathBuf::from("unused-runtime"),
            roster.owner().clone(),
            true,
            Selection::default(),
        )
        .unwrap();
        let owner = contexts.owner();
        let member_context = contexts
            .get_or_open(&roster, &member, true, Selection::default())
            .unwrap();
        owner
            .memory
            .remember("Owner-only secret: violet umbrella", "owner-chat")
            .await
            .unwrap();
        member_context
            .memory
            .remember("Member-only preference: green tea", "member-private-chat")
            .await
            .unwrap();
        assert_eq!(owner.memory.review("").await.unwrap().len(), 1);
        let member_notes = member_context.memory.review("").await.unwrap();
        assert_eq!(member_notes.len(), 1);
        assert!(
            !serde_json::to_string(&member_notes)
                .unwrap()
                .contains("violet umbrella")
        );
        assert_eq!(
            member_context.thread_id.lock().await.as_deref(),
            Some("member-private-chat")
        );
        *owner.thread_id.lock().await = Some("owner-chat".into());
        assert_ne!(
            *owner.thread_id.lock().await,
            *member_context.thread_id.lock().await
        );
        assert!(directory.path().join("memory").is_dir());
        assert!(
            directory
                .path()
                .join("profiles")
                .join(member.as_str())
                .join("memory")
                .is_dir()
        );
        let repeated = contexts
            .get_or_open(&roster, &member, true, Selection::default())
            .unwrap();
        assert!(Arc::ptr_eq(&member_context, &repeated));
        assert!(!Arc::ptr_eq(&owner.memory, &member_context.memory));
        roster
            .remove_member(&Actor::LocalDevice, directory.path(), &member)
            .unwrap();
        assert!(
            contexts
                .get_or_open(&roster, &member, true, Selection::default())
                .is_err()
        );
        // Captured turns retain their own store; removal must be checked again
        // by the host before committing an operation after await.
        assert_eq!(member_context.memory.review("").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn removed_or_unknown_profiles_cannot_reuse_cached_context() {
        let _stores = STORES.lock().await;
        let directory = tempfile::tempdir().unwrap();
        let roster = Roster::new("Owner").unwrap();
        let contexts = ProfileContexts::open(
            directory.path().into(),
            PathBuf::from("unused-runtime"),
            roster.owner().clone(),
            false,
            Selection::default(),
        )
        .unwrap();
        assert!(
            contexts
                .get_or_open(&roster, &ProfileId::random(), false, Selection::default())
                .is_err()
        );
        let changed_owner = Roster::new("New owner").unwrap();
        assert!(
            contexts
                .get_or_open(
                    &changed_owner,
                    changed_owner.owner(),
                    false,
                    Selection::default()
                )
                .is_err()
        );
    }
}
