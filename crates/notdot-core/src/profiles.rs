//! Household profiles: who may speak to notdot, whose context a turn runs in,
//! and which owner chats a trusted member may see.
//!
//! This module is pure policy. It holds no biometric data (voiceprints live in
//! the encrypted `notdot-speakers` store) and performs no I/O beyond the atomic
//! roster file. Every namespace is keyed by a random [`ProfileId`]; display names
//! never select storage, threads or grants.
//!
//! Security stance: a recognised voice is a *convenience identity*, not
//! authentication. Recordings, voice clones and noise can fool it. Therefore:
//! - device and security controls (members, mode, grants, keys, reset) require
//!   [`Actor::LocalDevice`], meaning a deliberate action in notdot's window on the
//!   logged-in computer, never a voice;
//! - a locked mode whose roster or voiceprints cannot be trusted fails closed;
//! - every identified turn carries an immutable [`TurnLock`] that sinks must
//!   re-validate before acting or speaking.
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
};

pub const MAX_PROFILES: usize = 8;
pub const MAX_GRANTS: usize = 256;
pub const MAX_NAME_CHARS: usize = 40;
/// Retired private chat IDs are kept forever (IDs only, no names) so a
/// removed person's chat can never become visible or shareable again.
pub const MAX_RETIRED_CHATS: usize = 256;
/// Name given to the owner before they say what notdot should call them.
pub const OWNER_PLACEHOLDER_NAME: &str = "Me";
const MAX_ROSTER_BYTES: u64 = 64 * 1024;
const ROSTER_VERSION: u32 = 1;

/// Random, stable identifier for one person. Only lowercase hyphenated UUIDv4
/// text is accepted, so an ID can never be a path fragment or a name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProfileId(String);
impl ProfileId {
    pub fn random() -> Self {
        Self(uuid::Uuid::new_v4().hyphenated().to_string())
    }
    pub fn parse(text: &str) -> Result<Self, String> {
        let parsed = uuid::Uuid::parse_str(text).map_err(|_| "Invalid profile ID")?;
        let canonical = parsed.hyphenated().to_string();
        if canonical != text || parsed.get_version_num() != 4 {
            return Err("Invalid profile ID".into());
        }
        Ok(Self(canonical))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for ProfileId {
    type Error = String;
    fn try_from(value: String) -> Result<Self, String> {
        Self::parse(&value)
    }
}
impl From<ProfileId> for String {
    fn from(value: ProfileId) -> String {
        value.0
    }
}
impl std::fmt::Display for ProfileId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The superuser who installed notdot. Exactly one.
    Owner,
    /// A trusted family member or friend with a private context.
    Member,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityMode {
    /// Legacy behaviour: anyone may speak; an unrecognised voice is served as
    /// the owner. A recognised member still gets their own private context.
    #[default]
    Open,
    /// Only the enrolled owner's voice is answered.
    OwnerOnly,
    /// The owner and enrolled members are answered, each in their own context.
    TrustedGroup,
}
impl IdentityMode {
    pub fn locked(self) -> bool {
        self != Self::Open
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub id: ProfileId,
    pub role: Role,
    pub name: String,
    pub created_unix_ms: u64,
    /// True once an encrypted voiceprint exists for this profile.
    #[serde(default)]
    pub voice_enrolled: bool,
    /// Private companion thread for a member. The owner keeps the legacy
    /// `settings.thread_id`, so this is always `None` for the owner.
    #[serde(default)]
    pub companion_thread: Option<String>,
    /// Bumped whenever this profile's authority shrinks (removal, revoked
    /// grant, mode change that excludes it). In-flight turns then fail.
    #[serde(default)]
    pub epoch: u64,
    /// When the owner confirmed, at this computer, that this person was
    /// present and agreed to voice recognition. Cleared with the voice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_consent_unix_ms: Option<u64>,
}
impl Profile {
    /// False while the owner still has the placeholder name. Callers must not
    /// address anyone by a placeholder.
    pub fn named(&self) -> bool {
        !(self.role == Role::Owner && self.name == OWNER_PLACEHOLDER_NAME)
    }
}

/// Archiving state of a removed member's private Codex chat.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum ArchiveState {
    Pending {
        attempts: u32,
        #[serde(default)]
        next_attempt_unix_ms: u64,
    },
    Archived {
        archived_unix_ms: u64,
    },
}

/// A removed member's private chat. Denied to everyone, forever.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetiredChat {
    pub chat_id: String,
    pub retired_unix_ms: u64,
    pub archive: ArchiveState,
}

/// Delay before the next archive attempt: 30 s doubling to at most 1 hour.
pub fn archive_backoff_ms(attempts: u32) -> u64 {
    (30_000u64 << attempts.saturating_sub(1).min(7)).min(3_600_000)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatAccess {
    /// See the chat in discovery, read its history and ask for its status.
    Read,
    /// Additionally send, steer and stop, still bounded by the global
    /// session-control mode and its confirmation rules.
    ReadWrite,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatGrant {
    pub chat_id: String,
    pub member: ProfileId,
    pub access: ChatAccess,
    pub granted_unix_ms: u64,
    /// The chat's title when it was shared, for display only. Never used for
    /// access decisions. Absent for grants made before titles were kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// Longest stored chat title, in characters.
const MAX_CHAT_TITLE: usize = 120;

/// A display title as stored: single-line, trimmed and bounded. Empty or
/// control-only input gives `None`.
fn clean_title(title: &str) -> Option<String> {
    let line: String = title
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let bounded: String = line.chars().take(MAX_CHAT_TITLE).collect();
    (!bounded.is_empty()).then_some(bounded)
}

/// Who is asking. `LocalDevice` is a deliberate click or key press in notdot's
/// own window on the logged-in computer. A voice is never `LocalDevice`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Actor {
    LocalDevice,
    Voice(TurnLock),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Enrol, re-enrol, rename or remove members and voiceprints.
    ManageMembers,
    ChangeIdentityMode,
    ManageChatGrants,
    ChangeCredentials,
    ResetData,
    /// Ordinary device preferences (voice, speed, models). Still subject to the
    /// existing spoken-confirmation flow.
    ChangeDeviceSettings,
    /// Read the device settings file (`read_notdot_settings`).
    ReadDeviceSettings,
    /// Read and write the owner's memory ledger.
    UseOwnerMemory,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "reason")]
pub enum Denied {
    NotPermitted { capability: Capability },
    Revoked,
    UnknownProfile,
}
impl std::fmt::Display for Denied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotPermitted { .. } => {
                f.write_str("That needs the owner, using notdot on this computer")
            }
            Self::Revoked => f.write_str("This request was cancelled because access changed"),
            Self::UnknownProfile => f.write_str("That person is no longer set up on this computer"),
        }
    }
}

/// Result of local speaker identification, as reported by the speaker engine.
#[derive(Clone, Debug, PartialEq)]
pub enum SpeakerOutcome {
    /// Identification is not configured (no model or no voiceprints) and the
    /// roster is in open mode.
    Disabled,
    Recognised {
        profile: ProfileId,
        score: f32,
    },
    /// Two profiles were too close, the score fell in the uncertain band, or the
    /// clip looked like a replay of an enrolment sample.
    Ambiguous,
    Unknown,
    /// Too short, too quiet, clipped or otherwise not assessable.
    Unusable,
    /// Model, key or voiceprint store could not be used (tampered, missing key,
    /// failed decryption, inference error or timeout).
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    /// Owner-only mode and a member was recognised.
    OwnerOnly,
    UnrecognisedVoice,
    UncertainVoice,
    UnclearAudio,
    /// Locked mode cannot verify anyone; use the local recovery controls.
    IdentityUnavailable,
    /// The roster file is damaged; local recovery is required.
    RecoveryRequired,
}
impl Refusal {
    /// Short spoken line. Deliberately does not name any enrolled person.
    pub fn spoken(self) -> &'static str {
        match self {
            Self::OwnerOnly => "Sorry, I'm only answering my owner right now.",
            Self::UnrecognisedVoice => "Sorry, I don't recognise your voice.",
            Self::UncertainVoice => "I couldn't quite tell who that was. Please try again.",
            Self::UnclearAudio => "I didn't catch enough of that. Please try again.",
            Self::IdentityUnavailable | Self::RecoveryRequired => {
                "Voice recognition needs attention on this computer."
            }
        }
    }
}

/// Immutable binding of one capture/request to one person's context. Created
/// once at identification and carried through STT, model, memory, learning,
/// task control, notifications and completion reports. Never rewritten; a
/// speaker switch is a new capture with a new lock.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnLock {
    pub capture_token: String,
    pub profile: ProfileId,
    pub role: Role,
    /// Profile epoch at admission.
    pub epoch: u64,
    /// True when admitted only because open mode serves unknown voices as the
    /// owner. Such turns get the owner's context, exactly as before profiles.
    pub open_fallback: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Admission {
    Admit(TurnLock),
    Refuse(Refusal),
}

/// Operations on another Codex chat. `Discover` covers list, search and title
/// metadata (`find_chats`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatOp {
    Discover,
    Read,
    Status,
    Send,
    Steer,
    Stop,
}
impl ChatOp {
    fn writes(self) -> bool {
        matches!(self, Self::Send | Self::Steer | Self::Stop)
    }
}

/// Global session-control ceiling, mirrored from
/// `session_control::Mode` so this module stays independent of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlobalChatMode {
    ReadOnly,
    Confirm,
    WhenAsked,
}
impl From<crate::session_control::Mode> for GlobalChatMode {
    fn from(mode: crate::session_control::Mode) -> Self {
        use crate::session_control::Mode;
        match mode {
            Mode::ReadOnly => Self::ReadOnly,
            Mode::Confirm => Self::Confirm,
            Mode::WhenAsked => Self::WhenAsked,
        }
    }
}

/// Storage and thread locations for one profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Namespace {
    pub profile: ProfileId,
    pub role: Role,
    /// Directory for this profile's own `Memory::open_with_learning` ledger.
    pub memory_dir: PathBuf,
    /// Directory for profile-private state (reminders, report outbox, etc.).
    pub state_dir: PathBuf,
    /// `None` for the owner (use `settings.thread_id`), or the member's own
    /// companion thread once created.
    pub companion_thread: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Roster {
    version: u32,
    mode: IdentityMode,
    owner: ProfileId,
    profiles: Vec<Profile>,
    #[serde(default)]
    grants: Vec<ChatGrant>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    retired_chats: Vec<RetiredChat>,
    /// Set when the stored roster could not be read; never persisted as true.
    #[serde(skip)]
    recovery_required: bool,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default()
}

pub fn validate_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    let count = name.chars().count();
    if count == 0 || count > MAX_NAME_CHARS {
        return Err(format!("Use a name of 1–{MAX_NAME_CHARS} characters"));
    }
    if name.chars().any(|c| c.is_control()) {
        return Err("Names cannot contain control characters".into());
    }
    Ok(name.to_owned())
}

fn validate_thread(thread: &str) -> Result<(), String> {
    if thread.is_empty()
        || thread.len() > 128
        || !thread
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("Invalid chat ID".into());
    }
    Ok(())
}

pub enum LoadOutcome {
    /// No roster file: identification has never been configured.
    Fresh(Roster),
    Loaded(Roster),
    /// The file exists but is unreadable, oversized or invalid. The returned
    /// roster fails closed until local recovery; the error is for local logs.
    Damaged(Roster, String),
}
impl LoadOutcome {
    pub fn roster(self) -> Roster {
        match self {
            Self::Fresh(r) | Self::Loaded(r) | Self::Damaged(r, _) => r,
        }
    }
}

impl Roster {
    /// A new roster with one owner and open mode.
    pub fn new(owner_name: &str) -> Result<Self, String> {
        let owner = ProfileId::random();
        Ok(Self {
            version: ROSTER_VERSION,
            mode: IdentityMode::Open,
            owner: owner.clone(),
            profiles: vec![Profile {
                id: owner,
                role: Role::Owner,
                name: validate_name(owner_name)?,
                created_unix_ms: now_ms(),
                voice_enrolled: false,
                companion_thread: None,
                epoch: 0,
                voice_consent_unix_ms: None,
            }],
            grants: Vec::new(),
            retired_chats: Vec::new(),
            recovery_required: false,
        })
    }

    pub fn mode(&self) -> IdentityMode {
        self.mode
    }
    pub fn owner(&self) -> &ProfileId {
        &self.owner
    }
    pub fn profiles(&self) -> &[Profile] {
        &self.profiles
    }
    pub fn grants(&self) -> &[ChatGrant] {
        &self.grants
    }
    pub fn retired_chats(&self) -> &[RetiredChat] {
        &self.retired_chats
    }
    /// A removed member's private chat: denied to everyone, forever.
    pub fn is_retired(&self, chat_id: &str) -> bool {
        self.retired_chats.iter().any(|r| r.chat_id == chat_id)
    }
    /// Retired chats whose Codex archive attempt is due at `now_unix_ms`.
    pub fn archives_due(&self, now_unix_ms: u64) -> Vec<String> {
        self.retired_chats
            .iter()
            .filter(|r| {
                matches!(r.archive, ArchiveState::Pending { next_attempt_unix_ms, .. } if next_attempt_unix_ms <= now_unix_ms)
            })
            .map(|r| r.chat_id.clone())
            .collect()
    }
    /// Record the outcome of an archive attempt. Retirement itself never
    /// changes: an archive failure only schedules another attempt.
    pub fn record_archive(&mut self, chat_id: &str, archived: bool, now_unix_ms: u64) {
        if let Some(r) = self.retired_chats.iter_mut().find(|r| r.chat_id == chat_id) {
            r.archive = match (&r.archive, archived) {
                (ArchiveState::Archived { .. }, _) => r.archive.clone(),
                (_, true) => ArchiveState::Archived {
                    archived_unix_ms: now_unix_ms,
                },
                (ArchiveState::Pending { attempts, .. }, false) => {
                    let attempts = attempts.saturating_add(1);
                    ArchiveState::Pending {
                        attempts,
                        next_attempt_unix_ms: now_unix_ms + archive_backoff_ms(attempts),
                    }
                }
            };
        }
    }
    pub fn recovery_required(&self) -> bool {
        self.recovery_required
    }
    pub fn profile(&self, id: &ProfileId) -> Option<&Profile> {
        self.profiles.iter().find(|p| &p.id == id)
    }
    fn profile_mut(&mut self, id: &ProfileId) -> Option<&mut Profile> {
        self.profiles.iter_mut().find(|p| &p.id == id)
    }
    /// Does any profile have a voiceprint? Locked modes require the owner's.
    pub fn owner_enrolled(&self) -> bool {
        self.profile(&self.owner).is_some_and(|p| p.voice_enrolled)
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != ROSTER_VERSION {
            return Err("Unsupported roster version".into());
        }
        if self.profiles.is_empty() || self.profiles.len() > MAX_PROFILES {
            return Err("Invalid profile count".into());
        }
        let owners: Vec<_> = self
            .profiles
            .iter()
            .filter(|p| p.role == Role::Owner)
            .collect();
        if owners.len() != 1 || owners[0].id != self.owner {
            return Err("Roster must have exactly one owner".into());
        }
        let mut seen = std::collections::BTreeSet::new();
        let mut threads = std::collections::BTreeSet::new();
        for p in &self.profiles {
            validate_name(&p.name)?;
            if !seen.insert(&p.id) {
                return Err("Duplicate profile ID".into());
            }
            if let Some(thread) = &p.companion_thread {
                validate_thread(thread)?;
                if p.role == Role::Owner || !threads.insert(thread) {
                    return Err("Companion threads must be private to one member".into());
                }
            }
        }
        if self.grants.len() > MAX_GRANTS {
            return Err("Too many chat grants".into());
        }
        let mut pairs = std::collections::BTreeSet::new();
        for g in &self.grants {
            validate_thread(&g.chat_id)?;
            if self
                .profile(&g.member)
                .is_none_or(|p| p.role != Role::Member)
            {
                return Err("Chat grants must name an existing member".into());
            }
            if threads.contains(&g.chat_id) {
                return Err("A member's private chat cannot be granted".into());
            }
            if !pairs.insert((&g.chat_id, &g.member)) {
                return Err("Duplicate chat grant".into());
            }
        }
        if self.retired_chats.len() > MAX_RETIRED_CHATS {
            return Err("Too many retired chats".into());
        }
        let mut retired = std::collections::BTreeSet::new();
        for r in &self.retired_chats {
            validate_thread(&r.chat_id)?;
            if !retired.insert(&r.chat_id) || threads.contains(&r.chat_id) {
                return Err("A retired chat cannot be reused".into());
            }
        }
        if self.grants.iter().any(|g| retired.contains(&g.chat_id)) {
            return Err("A retired chat cannot be shared".into());
        }
        if self.mode.locked() && !self.owner_enrolled() {
            return Err("A locked mode needs the owner's voice".into());
        }
        Ok(())
    }

    /// Load the roster. A missing file yields a fresh open-mode roster for
    /// `owner_name`. A damaged file yields a fail-closed roster: every voice is
    /// refused until the owner uses local recovery.
    pub fn load(path: &Path, owner_name: &str) -> LoadOutcome {
        match std::fs::symlink_metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return match Self::new(owner_name) {
                    Ok(r) => LoadOutcome::Fresh(r),
                    Err(e) => LoadOutcome::Damaged(Self::damaged(), e),
                };
            }
            Err(e) => return LoadOutcome::Damaged(Self::damaged(), e.to_string()),
            Ok(m) if !m.is_file() || m.len() > MAX_ROSTER_BYTES => {
                return LoadOutcome::Damaged(Self::damaged(), "Roster file is invalid".into());
            }
            Ok(_) => {}
        }
        let parsed = std::fs::read(path)
            .map_err(|e| e.to_string())
            .and_then(|b| serde_json::from_slice::<Self>(&b).map_err(|e| e.to_string()))
            .and_then(|r| r.validate().map(|()| r));
        match parsed {
            Ok(r) => LoadOutcome::Loaded(r),
            Err(e) => LoadOutcome::Damaged(Self::damaged(), e),
        }
    }

    fn damaged() -> Self {
        let owner = ProfileId::random();
        Self {
            version: ROSTER_VERSION,
            mode: IdentityMode::OwnerOnly,
            owner: owner.clone(),
            profiles: vec![Profile {
                id: owner,
                role: Role::Owner,
                name: "Owner".into(),
                created_unix_ms: now_ms(),
                voice_enrolled: false,
                companion_thread: None,
                epoch: 0,
                voice_consent_unix_ms: None,
            }],
            grants: Vec::new(),
            retired_chats: Vec::new(),
            recovery_required: true,
        }
    }

    /// Atomically persist the roster (write temporary file, sync, rename).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if self.recovery_required {
            return Err("Recover the roster before saving it".into());
        }
        self.validate()?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_ROSTER_BYTES {
            return Err("Roster is too large".into());
        }
        let parent = path.parent().ok_or("Invalid roster path")?;
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let tmp = parent.join(format!(".roster-{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)
                .map_err(|e| e.to_string())?;
            file.write_all(&bytes).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
            std::fs::rename(&tmp, path).map_err(|e| e.to_string())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }

    /// Local recovery after a damaged roster or lost voiceprint key: start a
    /// fresh open-mode roster that keeps the owner's existing ledger (the owner
    /// namespace is the legacy location). Member directories are not deleted
    /// here; [`Roster::orphaned_member_dirs`] lists them for an explicit,
    /// separately confirmed removal.
    pub fn recover(actor: &Actor, owner_name: &str) -> Result<Self, Denied> {
        if actor != &Actor::LocalDevice {
            return Err(Denied::NotPermitted {
                capability: Capability::ResetData,
            });
        }
        Self::new(owner_name).map_err(|_| Denied::NotPermitted {
            capability: Capability::ResetData,
        })
    }

    // ---------------------------------------------------------------- authority

    pub fn authorise(&self, actor: &Actor, capability: Capability) -> Result<(), Denied> {
        let lock = match actor {
            Actor::LocalDevice => return Ok(()),
            Actor::Voice(lock) => lock,
        };
        self.validate_lock(lock)?;
        let owner_voice = lock.role == Role::Owner;
        let permitted = match capability {
            Capability::ManageMembers
            | Capability::ChangeIdentityMode
            | Capability::ManageChatGrants
            | Capability::ChangeCredentials
            | Capability::ResetData => false,
            Capability::ChangeDeviceSettings
            | Capability::ReadDeviceSettings
            | Capability::UseOwnerMemory => owner_voice,
        };
        if permitted {
            Ok(())
        } else {
            Err(Denied::NotPermitted { capability })
        }
    }

    /// Re-check a lock before any sink acts on it (memory write, tool call,
    /// chat action, spoken reply, notification). Fails after removal, a revoked
    /// grant, a mode change that excludes the profile, or recovery.
    pub fn validate_lock(&self, lock: &TurnLock) -> Result<(), Denied> {
        if self.recovery_required {
            return Err(Denied::Revoked);
        }
        let profile = self.profile(&lock.profile).ok_or(Denied::UnknownProfile)?;
        if profile.epoch != lock.epoch || profile.role != lock.role {
            return Err(Denied::Revoked);
        }
        if lock.open_fallback && self.mode.locked() {
            return Err(Denied::Revoked);
        }
        Ok(())
    }

    // ---------------------------------------------------------------- admission

    /// Decide whether a capture may proceed and in whose context. Called before
    /// any audio leaves the device in locked modes.
    pub fn admit(&self, capture_token: &str, outcome: &SpeakerOutcome) -> Admission {
        if self.recovery_required {
            return Admission::Refuse(Refusal::RecoveryRequired);
        }
        let lock = |p: &Profile, open_fallback| {
            Admission::Admit(TurnLock {
                capture_token: capture_token.to_owned(),
                profile: p.id.clone(),
                role: p.role,
                epoch: p.epoch,
                open_fallback,
            })
        };
        let owner = self.profile(&self.owner).expect("validated owner");
        // A match only counts for a profile that currently has a voiceprint;
        // a stale result for a forgotten or never-enrolled voice is unknown.
        let recognised = match outcome {
            SpeakerOutcome::Recognised { profile, .. } => {
                self.profile(profile).filter(|p| p.voice_enrolled)
            }
            _ => None,
        };
        match self.mode {
            IdentityMode::Open => match recognised {
                Some(p) => lock(p, false),
                None => lock(owner, true),
            },
            IdentityMode::OwnerOnly | IdentityMode::TrustedGroup => match (recognised, outcome) {
                (Some(p), _) if p.role == Role::Owner => lock(p, false),
                (Some(p), _) if self.mode == IdentityMode::TrustedGroup => lock(p, false),
                (Some(_), _) => Admission::Refuse(Refusal::OwnerOnly),
                (None, SpeakerOutcome::Ambiguous) => Admission::Refuse(Refusal::UncertainVoice),
                (None, SpeakerOutcome::Unusable) => Admission::Refuse(Refusal::UnclearAudio),
                (None, SpeakerOutcome::Unavailable | SpeakerOutcome::Disabled) => {
                    Admission::Refuse(Refusal::IdentityUnavailable)
                }
                // Unknown, or recognised as an ID no longer in the roster.
                (None, _) => Admission::Refuse(Refusal::UnrecognisedVoice),
            },
        }
    }

    // ---------------------------------------------------------------- namespaces

    /// Where this profile's private data lives. The owner keeps the legacy
    /// `<root>/memory` ledger; members live under `<root>/profiles/<id>/`.
    pub fn namespace(&self, root: &Path, id: &ProfileId) -> Option<Namespace> {
        let p = self.profile(id)?;
        Some(match p.role {
            Role::Owner => Namespace {
                profile: p.id.clone(),
                role: p.role,
                memory_dir: root.join("memory"),
                state_dir: root.join("owner"),
                companion_thread: None,
            },
            Role::Member => {
                let base = root.join("profiles").join(p.id.as_str());
                Namespace {
                    profile: p.id.clone(),
                    role: p.role,
                    memory_dir: base.join("memory"),
                    state_dir: base.join("state"),
                    companion_thread: p.companion_thread.clone(),
                }
            }
        })
    }

    /// Record a member's newly created private companion thread. Refuses the
    /// owner's thread, any other member's thread and any granted owner chat.
    pub fn set_companion_thread(
        &mut self,
        member: &ProfileId,
        thread: &str,
        owner_thread: Option<&str>,
    ) -> Result<(), String> {
        validate_thread(thread)?;
        if owner_thread == Some(thread)
            || self.is_retired(thread)
            || self.grants.iter().any(|g| g.chat_id == thread)
            || self
                .profiles
                .iter()
                .any(|p| &p.id != member && p.companion_thread.as_deref() == Some(thread))
        {
            return Err("A member's companion chat must be new and private".into());
        }
        let p = self.profile_mut(member).ok_or("Unknown profile")?;
        if p.role != Role::Member {
            return Err("The owner keeps the existing companion chat".into());
        }
        p.companion_thread = Some(thread.to_owned());
        Ok(())
    }

    /// Member directories under `<root>/profiles` not in this roster. Listed for
    /// an explicit local clean-up; never deleted implicitly.
    pub fn orphaned_member_dirs(&self, root: &Path) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(root.join("profiles")) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let id = ProfileId::parse(&name).ok()?;
                self.profile(&id).is_none().then(|| e.path())
            })
            .collect()
    }

    // ---------------------------------------------------------------- mutation

    pub fn add_member(&mut self, actor: &Actor, name: &str) -> Result<ProfileId, String> {
        self.authorise(actor, Capability::ManageMembers)
            .map_err(|e| e.to_string())?;
        if self.recovery_required {
            return Err("Recover voice recognition first".into());
        }
        if self.profiles.len() >= MAX_PROFILES {
            return Err(format!("notdot supports up to {MAX_PROFILES} people"));
        }
        let name = validate_name(name)?;
        if self
            .profiles
            .iter()
            .any(|p| p.name.to_lowercase() == name.to_lowercase())
        {
            return Err("Someone already uses that name; choose another".into());
        }
        // Adding the first member changes the companion's tool boundary even
        // for the owner. Revoke any live legacy owner runtime immediately.
        if self.profiles.len() == 1 {
            for profile in &mut self.profiles {
                profile.epoch = profile.epoch.wrapping_add(1);
            }
        }
        let id = ProfileId::random();
        self.profiles.push(Profile {
            id: id.clone(),
            role: Role::Member,
            name,
            created_unix_ms: now_ms(),
            voice_enrolled: false,
            companion_thread: None,
            epoch: 0,
            voice_consent_unix_ms: None,
        });
        Ok(id)
    }

    pub fn rename(&mut self, actor: &Actor, id: &ProfileId, name: &str) -> Result<(), String> {
        self.authorise(actor, Capability::ManageMembers)
            .map_err(|e| e.to_string())?;
        let name = validate_name(name)?;
        if self
            .profiles
            .iter()
            .any(|p| &p.id != id && p.name.to_lowercase() == name.to_lowercase())
        {
            return Err("Someone already uses that name; choose another".into());
        }
        self.profile_mut(id).ok_or("Unknown profile")?.name = name;
        Ok(())
    }

    /// Mark whether a voiceprint exists. Re-enrolment bumps the epoch so turns
    /// admitted under the old voiceprint cannot finish.
    pub fn set_voice_enrolled(
        &mut self,
        actor: &Actor,
        id: &ProfileId,
        enrolled: bool,
    ) -> Result<(), String> {
        self.authorise(actor, Capability::ManageMembers)
            .map_err(|e| e.to_string())?;
        if !enrolled && id == &self.owner && self.mode.locked() {
            return Err("Switch to open mode before removing the owner's voice".into());
        }
        let p = self.profile_mut(id).ok_or("Unknown profile")?;
        p.voice_enrolled = enrolled;
        if !enrolled {
            p.voice_consent_unix_ms = None;
        }
        p.epoch = p.epoch.wrapping_add(1);
        Ok(())
    }

    /// Record that the owner confirmed at this computer that the person was
    /// present and agreed to voice recognition. Required to save a voice.
    pub fn record_voice_consent(&mut self, actor: &Actor, id: &ProfileId) -> Result<(), String> {
        self.authorise(actor, Capability::ManageMembers)
            .map_err(|e| e.to_string())?;
        self.profile_mut(id)
            .ok_or("Unknown profile")?
            .voice_consent_unix_ms = Some(now_ms());
        Ok(())
    }

    /// Remove a member, their grants and their voiceprint flag. Returns the
    /// member's namespace so the caller can delete exactly that directory and
    /// voiceprint, after the in-flight turns have been invalidated.
    pub fn remove_member(
        &mut self,
        actor: &Actor,
        root: &Path,
        id: &ProfileId,
    ) -> Result<Namespace, String> {
        self.authorise(actor, Capability::ManageMembers)
            .map_err(|e| e.to_string())?;
        self.remove_member_recorded(root, id)
    }

    /// Complete a removal whose authority was already checked natively and
    /// recorded durably (offboarding marker). Never callable for the owner.
    /// Revokes the profile, its grants and its pending turns, and retires its
    /// private chat so nobody (the owner included) can see or reuse it.
    pub fn remove_member_recorded(
        &mut self,
        root: &Path,
        id: &ProfileId,
    ) -> Result<Namespace, String> {
        let ns = self.namespace(root, id).ok_or("Unknown profile")?;
        if ns.role == Role::Owner || id == &self.owner {
            return Err("The owner cannot be removed".into());
        }
        if let Some(thread) = &ns.companion_thread {
            if self.retired_chats.len() >= MAX_RETIRED_CHATS {
                return Err("Too many removed people; contact support".into());
            }
            if !self.is_retired(thread) {
                self.retired_chats.push(RetiredChat {
                    chat_id: thread.clone(),
                    retired_unix_ms: now_ms(),
                    archive: ArchiveState::Pending {
                        attempts: 0,
                        next_attempt_unix_ms: 0,
                    },
                });
            }
        }
        self.profiles.retain(|p| &p.id != id);
        self.grants.retain(|g| &g.member != id);
        Ok(ns)
    }

    pub fn set_mode(&mut self, actor: &Actor, mode: IdentityMode) -> Result<(), String> {
        self.authorise(actor, Capability::ChangeIdentityMode)
            .map_err(|e| e.to_string())?;
        if mode.locked() && !self.owner_enrolled() {
            return Err("Teach notdot your voice before locking it".into());
        }
        if mode == self.mode {
            return Ok(());
        }
        // Switching identity mode changes the runtime/privacy contract. Every
        // admitted turn must reconnect under the new boundary, including owner.
        for profile in &mut self.profiles {
            profile.epoch = profile.epoch.wrapping_add(1);
        }
        self.mode = mode;
        Ok(())
    }

    /// Grant or change one member's access to one owner chat. A first grant is
    /// read-only; `ReadWrite` requires an existing `Read` grant, so control is
    /// always a second explicit step.
    pub fn grant_chat(
        &mut self,
        actor: &Actor,
        chat_id: &str,
        member: &ProfileId,
        access: ChatAccess,
    ) -> Result<(), String> {
        self.authorise(actor, Capability::ManageChatGrants)
            .map_err(|e| e.to_string())?;
        validate_thread(chat_id)?;
        if self.profile(member).is_none_or(|p| p.role != Role::Member) {
            return Err("Chats can only be shared with a member".into());
        }
        if self.is_retired(chat_id)
            || self
                .profiles
                .iter()
                .any(|p| p.companion_thread.as_deref() == Some(chat_id))
        {
            return Err("A member's private chat cannot be shared".into());
        }
        let existing = self
            .grants
            .iter_mut()
            .find(|g| g.chat_id == chat_id && &g.member == member);
        match (existing, access) {
            (Some(g), _) => {
                if access < g.access {
                    let p = self.profiles.iter_mut().find(|p| &p.id == member);
                    if let Some(p) = p {
                        p.epoch = p.epoch.wrapping_add(1);
                    }
                }
                // Re-borrow after the epoch bump.
                let g = self
                    .grants
                    .iter_mut()
                    .find(|g| g.chat_id == chat_id && &g.member == member)
                    .expect("grant exists");
                g.access = access;
                Ok(())
            }
            (None, ChatAccess::ReadWrite) => {
                Err("Share the chat read-only first, then allow control separately".into())
            }
            (None, ChatAccess::Read) => {
                if self.grants.len() >= MAX_GRANTS {
                    return Err("Too many shared chats".into());
                }
                self.grants.push(ChatGrant {
                    chat_id: chat_id.to_owned(),
                    member: member.clone(),
                    access,
                    granted_unix_ms: now_ms(),
                    title: None,
                });
                Ok(())
            }
        }
    }

    /// Remember the title of a shared chat for display. Every grant of that
    /// chat is updated; a chat that isn't shared is left alone.
    pub fn name_chat(&mut self, actor: &Actor, chat_id: &str, title: &str) -> Result<(), String> {
        self.authorise(actor, Capability::ManageChatGrants)
            .map_err(|e| e.to_string())?;
        validate_thread(chat_id)?;
        let title = clean_title(title);
        for g in self.grants.iter_mut().filter(|g| g.chat_id == chat_id) {
            if title.is_some() {
                g.title.clone_from(&title);
            }
        }
        Ok(())
    }

    pub fn revoke_chat(
        &mut self,
        actor: &Actor,
        chat_id: &str,
        member: &ProfileId,
    ) -> Result<(), String> {
        self.authorise(actor, Capability::ManageChatGrants)
            .map_err(|e| e.to_string())?;
        let before = self.grants.len();
        self.grants
            .retain(|g| !(g.chat_id == chat_id && &g.member == member));
        if before != self.grants.len()
            && let Some(p) = self.profile_mut(member)
        {
            p.epoch = p.epoch.wrapping_add(1);
        }
        Ok(())
    }

    // ---------------------------------------------------------------- chat ACL

    /// May `lock`'s profile perform `op` on `chat_id`? The owner may act on any
    /// chat except members' private companion chats. A member may use their own
    /// companion chat and granted owner chats only. Writes need `ReadWrite` and a
    /// global mode other than `ReadOnly`; existing confirmation still applies.
    pub fn chat_allowed(
        &self,
        lock: &TurnLock,
        chat_id: &str,
        op: ChatOp,
        global: GlobalChatMode,
    ) -> bool {
        if self.validate_lock(lock).is_err() {
            return false;
        }
        if op.writes() && global == GlobalChatMode::ReadOnly {
            return false;
        }
        if self.is_retired(chat_id) {
            return false;
        }
        let private_owner = self
            .profiles
            .iter()
            .find(|p| p.companion_thread.as_deref() == Some(chat_id))
            .map(|p| &p.id);
        if let Some(owner) = private_owner {
            return owner == &lock.profile;
        }
        match lock.role {
            Role::Owner => true,
            Role::Member => self
                .grants
                .iter()
                .find(|g| g.chat_id == chat_id && g.member == lock.profile)
                .is_some_and(|g| !op.writes() || g.access == ChatAccess::ReadWrite),
        }
    }

    /// Filter discovery results (IDs, titles, previews, counts) so ungranted
    /// chats leave no trace. Apply before results reach the model or the UI.
    pub fn filter_chats<T>(
        &self,
        lock: &TurnLock,
        items: Vec<T>,
        chat_id: impl Fn(&T) -> &str,
    ) -> Vec<T> {
        items
            .into_iter()
            .filter(|item| {
                self.chat_allowed(
                    lock,
                    chat_id(item),
                    ChatOp::Discover,
                    GlobalChatMode::ReadOnly,
                )
            })
            .collect()
    }

    /// Settings visible to `read_notdot_settings` for this turn: the device file
    /// for the owner, a minimal non-sensitive view for members.
    pub fn settings_scope(&self, lock: &TurnLock) -> SettingsScope {
        if self
            .authorise(&Actor::Voice(lock.clone()), Capability::ReadDeviceSettings)
            .is_ok()
        {
            SettingsScope::Device
        } else {
            SettingsScope::MemberSummary
        }
    }

    /// Plain summary for the settings panel; never contains voiceprints.
    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({
            "mode": self.mode,
            "recovery_required": self.recovery_required,
            "people": self.profiles.iter().map(|p| serde_json::json!({
                "id": p.id,
                "role": p.role,
                "name": p.name,
                "voice_enrolled": p.voice_enrolled,
                "voice_consent_unix_ms": p.voice_consent_unix_ms,
                "needs_name": !p.named(),
                "shared_chats": self.grants.iter().filter(|g| g.member == p.id).map(|g| serde_json::json!({
                    "chat_id": g.chat_id, "access": g.access, "title": g.title,
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "removed_chats": {
                "archived": self.retired_chats.iter().filter(|r| matches!(r.archive, ArchiveState::Archived { .. })).count(),
                "pending": self.retired_chats.iter().filter(|r| matches!(r.archive, ArchiveState::Pending { .. })).count(),
            },
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsScope {
    Device,
    /// Only the member's own name and the current mode; no owner chat names,
    /// models, keys or other members.
    MemberSummary,
}

/// Per-profile, bounded queue for completion reports, reminders and other
/// deferred announcements. Items are only released to their own profile, and
/// only when that profile is the current authorised speaker.
#[derive(Debug, Default)]
pub struct Outbox<T> {
    queues: BTreeMap<ProfileId, VecDeque<(u64, T)>>,
}
pub const OUTBOX_PER_PROFILE: usize = 32;
impl<T> Outbox<T> {
    pub fn new() -> Self {
        Self {
            queues: BTreeMap::new(),
        }
    }
    /// Queue an item for the profile that made the request. The oldest item is
    /// dropped when the per-profile bound is reached; it is returned so the
    /// caller can record that it was not delivered.
    pub fn push(&mut self, lock: &TurnLock, item: T) -> Option<T> {
        let queue = self.queues.entry(lock.profile.clone()).or_default();
        let dropped = (queue.len() >= OUTBOX_PER_PROFILE)
            .then(|| queue.pop_front().map(|(_, t)| t))
            .flatten();
        queue.push_back((lock.epoch, item));
        dropped
    }
    /// Release items for the active authorised speaker. Items whose epoch is
    /// stale (access changed since the request) are discarded, never spoken.
    pub fn take_for(&mut self, roster: &Roster, active: &TurnLock) -> Vec<T> {
        if roster.validate_lock(active).is_err() {
            return Vec::new();
        }
        let Some(queue) = self.queues.remove(&active.profile) else {
            return Vec::new();
        };
        queue
            .into_iter()
            .filter(|(epoch, _)| *epoch == active.epoch)
            .map(|(_, t)| t)
            .collect()
    }
    pub fn pending_for(&self, id: &ProfileId) -> usize {
        self.queues.get(id).map_or(0, VecDeque::len)
    }
    /// After a restart or roster change, drop items for removed profiles.
    pub fn retain_known(&mut self, roster: &Roster) {
        self.queues.retain(|id, _| roster.profile(id).is_some());
    }
    pub fn forget(&mut self, id: &ProfileId) {
        self.queues.remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roster_with_member() -> (Roster, ProfileId) {
        let mut r = Roster::new("Kamba").unwrap();
        let m = r.add_member(&Actor::LocalDevice, "Ada").unwrap();
        let owner = r.owner().clone();
        r.set_voice_enrolled(&Actor::LocalDevice, &owner, true)
            .unwrap();
        r.set_voice_enrolled(&Actor::LocalDevice, &m, true).unwrap();
        (r, m)
    }
    #[test]
    fn shared_chat_titles_are_kept_for_display_only() {
        let (mut r, m) = roster_with_member();
        r.grant_chat(&Actor::LocalDevice, "owner-a", &m, ChatAccess::Read)
            .unwrap();
        // Single-line, trimmed and bounded; blank input keeps what was there.
        r.name_chat(&Actor::LocalDevice, "owner-a", "  Weekend\n plans\t ")
            .unwrap();
        assert_eq!(r.grants()[0].title.as_deref(), Some("Weekend plans"));
        r.name_chat(&Actor::LocalDevice, "owner-a", " \u{7} ")
            .unwrap();
        assert_eq!(r.grants()[0].title.as_deref(), Some("Weekend plans"));
        r.name_chat(&Actor::LocalDevice, "owner-a", &"é".repeat(500))
            .unwrap();
        assert_eq!(
            r.grants()[0].title.as_ref().unwrap().chars().count(),
            MAX_CHAT_TITLE
        );
        r.name_chat(&Actor::LocalDevice, "owner-a", "Weekend plans")
            .unwrap();
        // Allowing messages later keeps the title.
        r.grant_chat(&Actor::LocalDevice, "owner-a", &m, ChatAccess::ReadWrite)
            .unwrap();
        assert_eq!(r.grants()[0].title.as_deref(), Some("Weekend plans"));
        assert_eq!(
            r.summary()["people"][1]["shared_chats"][0]["title"],
            "Weekend plans"
        );
        // Naming needs the local device, and an unshared chat is left alone.
        let voice = Actor::Voice(admit(&r, recognised(&r.owner().clone())));
        assert!(r.name_chat(&voice, "owner-a", "Renamed").is_err());
        assert!(r.name_chat(&Actor::LocalDevice, "bad id!", "x").is_err());
        r.name_chat(&Actor::LocalDevice, "not-shared", "Other")
            .unwrap();
        assert_eq!(r.grants().len(), 1);
        // Saved without a title for older grants, and older files still load.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profiles.json");
        r.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"title\": \"Weekend plans\""));
        std::fs::write(
            &path,
            text.replace(",\n      \"title\": \"Weekend plans\"", ""),
        )
        .unwrap();
        match Roster::load(&path, "Kamba") {
            LoadOutcome::Loaded(old) => assert_eq!(old.grants()[0].title, None),
            _ => panic!("a grant without a title must still load"),
        }
    }
    fn admit(r: &Roster, outcome: SpeakerOutcome) -> TurnLock {
        match r.admit("cap-1", &outcome) {
            Admission::Admit(l) => l,
            other => panic!("expected admission, got {other:?}"),
        }
    }
    fn recognised(id: &ProfileId) -> SpeakerOutcome {
        SpeakerOutcome::Recognised {
            profile: id.clone(),
            score: 0.8,
        }
    }

    #[test]
    fn profile_ids_are_random_uuid_v4_and_never_names_or_paths() {
        let a = ProfileId::random();
        assert_ne!(a, ProfileId::random());
        assert!(ProfileId::parse(a.as_str()).is_ok());
        for bad in [
            "Kamba",
            "../memory",
            "",
            "00000000-0000-1000-8000-000000000000",
            &a.as_str().to_uppercase(),
        ] {
            assert!(ProfileId::parse(bad).is_err(), "{bad}");
        }
        assert!(serde_json::from_str::<ProfileId>("\"../x\"").is_err());
    }

    #[test]
    fn open_mode_serves_unknown_as_owner_and_members_privately() {
        let (r, m) = roster_with_member();
        for o in [
            SpeakerOutcome::Disabled,
            SpeakerOutcome::Unknown,
            SpeakerOutcome::Ambiguous,
            SpeakerOutcome::Unusable,
            SpeakerOutcome::Unavailable,
        ] {
            let l = admit(&r, o);
            assert_eq!(&l.profile, r.owner());
            assert!(l.open_fallback);
        }
        let l = admit(&r, recognised(&m));
        assert_eq!(l.profile, m);
        assert_eq!(l.role, Role::Member);
        assert!(!l.open_fallback);
    }

    #[test]
    fn locked_modes_fail_closed() {
        let (mut r, m) = roster_with_member();
        let owner = r.owner().clone();
        r.set_mode(&Actor::LocalDevice, IdentityMode::OwnerOnly)
            .unwrap();
        assert_eq!(admit(&r, recognised(&owner)).profile, owner);
        let cases = [
            (recognised(&m), Refusal::OwnerOnly),
            (SpeakerOutcome::Unknown, Refusal::UnrecognisedVoice),
            (SpeakerOutcome::Ambiguous, Refusal::UncertainVoice),
            (SpeakerOutcome::Unusable, Refusal::UnclearAudio),
            (SpeakerOutcome::Unavailable, Refusal::IdentityUnavailable),
            (SpeakerOutcome::Disabled, Refusal::IdentityUnavailable),
            (recognised(&ProfileId::random()), Refusal::UnrecognisedVoice),
        ];
        for (o, want) in cases.clone() {
            assert_eq!(r.admit("c", &o), Admission::Refuse(want), "{o:?}");
        }
        r.set_mode(&Actor::LocalDevice, IdentityMode::TrustedGroup)
            .unwrap();
        assert_eq!(admit(&r, recognised(&m)).profile, m);
        for (o, want) in cases.into_iter().skip(1) {
            assert_eq!(r.admit("c", &o), Admission::Refuse(want));
        }
    }

    #[test]
    fn removed_members_private_chat_is_retired_for_everyone_forever() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profiles.json");
        let (mut r, m) = roster_with_member();
        r.set_companion_thread(&m, "ada-private", Some("owner-thread"))
            .unwrap();
        r.grant_chat(&Actor::LocalDevice, "owner-shared", &m, ChatAccess::Read)
            .unwrap();
        r.remove_member(&Actor::LocalDevice, Path::new("/d"), &m)
            .unwrap();
        // Only the private chat is retired; shared owner chats are untouched.
        assert_eq!(r.retired_chats().len(), 1);
        assert!(r.is_retired("ada-private") && !r.is_retired("owner-shared"));
        assert!(r.grants().is_empty());
        let owner = admit(&r, recognised(&r.owner().clone()));
        let g = GlobalChatMode::WhenAsked;
        for op in [
            ChatOp::Discover,
            ChatOp::Read,
            ChatOp::Status,
            ChatOp::Send,
            ChatOp::Steer,
            ChatOp::Stop,
        ] {
            assert!(!r.chat_allowed(&owner, "ada-private", op, g), "{op:?}");
        }
        assert!(
            r.filter_chats(&owner, vec!["ada-private", "owner-shared"], |c| c)
                == vec!["owner-shared"]
        );
        // Re-adding the same name starts completely fresh.
        let again = r.add_member(&Actor::LocalDevice, "Ada").unwrap();
        assert_ne!(again, m);
        let p = r.profile(&again).unwrap();
        assert!(
            !p.voice_enrolled && p.companion_thread.is_none() && p.voice_consent_unix_ms.is_none()
        );
        assert!(
            r.grant_chat(&Actor::LocalDevice, "ada-private", &again, ChatAccess::Read)
                .is_err()
        );
        assert!(r.set_companion_thread(&again, "ada-private", None).is_err());
        // Durable across restart.
        r.save(&path).unwrap();
        let LoadOutcome::Loaded(back) = Roster::load(&path, "x") else {
            panic!()
        };
        assert!(back.is_retired("ada-private"));
        // A hand-edited file that shares a retired chat is refused.
        let text = std::fs::read_to_string(&path).unwrap();
        let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
        v["grants"] = serde_json::json!([{"chat_id":"ada-private","member":again,"access":"read","granted_unix_ms":1}]);
        std::fs::write(&path, v.to_string()).unwrap();
        assert!(matches!(Roster::load(&path, "x"), LoadOutcome::Damaged(..)));
    }

    #[test]
    fn archive_attempts_back_off_and_never_unretire() {
        let (mut r, m) = roster_with_member();
        r.set_companion_thread(&m, "ada-private", None).unwrap();
        r.remove_member(&Actor::LocalDevice, Path::new("/d"), &m)
            .unwrap();
        assert_eq!(r.archives_due(0), vec!["ada-private"]);
        r.record_archive("ada-private", false, 1_000);
        assert!(r.archives_due(1_000).is_empty());
        assert_eq!(r.archives_due(31_000), vec!["ada-private"]);
        for i in 0..20 {
            r.record_archive("ada-private", false, i);
        }
        assert!(
            matches!(r.retired_chats()[0].archive, ArchiveState::Pending { attempts: 21, next_attempt_unix_ms } if next_attempt_unix_ms == 19 + 3_600_000)
        );
        r.record_archive("ada-private", true, 5);
        r.record_archive("ada-private", false, 6);
        assert!(matches!(
            r.retired_chats()[0].archive,
            ArchiveState::Archived {
                archived_unix_ms: 5
            }
        ));
        assert!(r.archives_due(u64::MAX).is_empty());
        assert!(r.is_retired("ada-private"));
        assert_eq!(
            r.summary()["removed_chats"],
            serde_json::json!({"archived":1,"pending":0})
        );
    }

    #[test]
    fn owner_can_never_be_removed_even_by_recorded_removal() {
        let (mut r, _) = roster_with_member();
        let owner = r.owner().clone();
        assert!(r.remove_member_recorded(Path::new("/d"), &owner).is_err());
        assert!(
            r.remove_member(&Actor::LocalDevice, Path::new("/d"), &owner)
                .is_err()
        );
        assert!(r.profile(&owner).is_some());
    }

    #[test]
    fn consent_is_recorded_locally_and_cleared_with_the_voice() {
        let (mut r, m) = roster_with_member();
        let voice = Actor::Voice(admit(&r, recognised(&m)));
        assert!(r.record_voice_consent(&voice, &m).is_err());
        r.record_voice_consent(&Actor::LocalDevice, &m).unwrap();
        assert!(r.profile(&m).unwrap().voice_consent_unix_ms.is_some());
        r.set_voice_enrolled(&Actor::LocalDevice, &m, false)
            .unwrap();
        assert!(r.profile(&m).unwrap().voice_consent_unix_ms.is_none());
        let fresh = Roster::new(OWNER_PLACEHOLDER_NAME).unwrap();
        assert!(!fresh.profile(fresh.owner()).unwrap().named());
        assert!(r.profile(&m).unwrap().named());
    }

    #[test]
    fn recognition_of_a_profile_without_a_voice_is_not_admission() {
        let (mut r, m) = roster_with_member();
        r.set_voice_enrolled(&Actor::LocalDevice, &m, false)
            .unwrap();
        // Open mode: falls back to the owner context, never the member's.
        let l = admit(&r, recognised(&m));
        assert_eq!(&l.profile, r.owner());
        assert!(l.open_fallback);
        r.set_mode(&Actor::LocalDevice, IdentityMode::TrustedGroup)
            .unwrap();
        assert_eq!(
            r.admit("c", &recognised(&m)),
            Admission::Refuse(Refusal::UnrecognisedVoice)
        );
    }

    #[test]
    fn cannot_lock_without_owner_voice_or_remove_it_while_locked() {
        let mut r = Roster::new("Kamba").unwrap();
        assert!(
            r.set_mode(&Actor::LocalDevice, IdentityMode::OwnerOnly)
                .is_err()
        );
        let owner = r.owner().clone();
        r.set_voice_enrolled(&Actor::LocalDevice, &owner, true)
            .unwrap();
        r.set_mode(&Actor::LocalDevice, IdentityMode::OwnerOnly)
            .unwrap();
        assert!(
            r.set_voice_enrolled(&Actor::LocalDevice, &owner, false)
                .is_err()
        );
    }

    #[test]
    fn voices_never_gain_security_authority() {
        let (mut r, m) = roster_with_member();
        let owner_voice = Actor::Voice(admit(&r, recognised(&r.owner().clone())));
        let member_voice = Actor::Voice(admit(&r, recognised(&m)));
        for cap in [
            Capability::ManageMembers,
            Capability::ChangeIdentityMode,
            Capability::ManageChatGrants,
            Capability::ChangeCredentials,
            Capability::ResetData,
        ] {
            assert!(r.authorise(&owner_voice, cap).is_err());
            assert!(r.authorise(&member_voice, cap).is_err());
            assert!(r.authorise(&Actor::LocalDevice, cap).is_ok());
        }
        for cap in [
            Capability::ChangeDeviceSettings,
            Capability::ReadDeviceSettings,
            Capability::UseOwnerMemory,
        ] {
            assert!(r.authorise(&owner_voice, cap).is_ok());
            assert!(r.authorise(&member_voice, cap).is_err());
        }
        // A member cannot grant themselves anything, enrol or change mode.
        assert!(
            r.grant_chat(&member_voice, "chat-1", &m, ChatAccess::Read)
                .is_err()
        );
        assert!(r.add_member(&member_voice, "Eve").is_err());
        assert!(r.set_mode(&member_voice, IdentityMode::Open).is_err());
        assert!(Roster::recover(&member_voice, "x").is_err());
        assert!(r.grants().is_empty());
    }

    #[test]
    fn namespaces_are_isolated_and_keyed_by_id() {
        let (mut r, m) = roster_with_member();
        let root = Path::new("/data");
        let owner = r.namespace(root, &r.owner().clone()).unwrap();
        let member = r.namespace(root, &m).unwrap();
        assert_eq!(owner.memory_dir, root.join("memory"));
        assert_eq!(
            member.memory_dir,
            root.join("profiles").join(m.as_str()).join("memory")
        );
        assert!(!member.memory_dir.starts_with(&owner.memory_dir));
        // Renaming does not move data.
        r.rename(&Actor::LocalDevice, &m, "Ada Lovelace").unwrap();
        assert_eq!(r.namespace(root, &m).unwrap(), member);
        // Duplicate names are refused (case-insensitive).
        assert!(r.add_member(&Actor::LocalDevice, "ada lovelace").is_err());
    }

    #[test]
    fn companion_threads_are_never_reused() {
        let (mut r, m) = roster_with_member();
        let other = r.add_member(&Actor::LocalDevice, "Grace").unwrap();
        assert!(
            r.set_companion_thread(&m, "owner-thread", Some("owner-thread"))
                .is_err()
        );
        r.set_companion_thread(&m, "ada-thread", Some("owner-thread"))
            .unwrap();
        assert!(r.set_companion_thread(&other, "ada-thread", None).is_err());
        let owner = r.owner().clone();
        assert!(r.set_companion_thread(&owner, "new", None).is_err());
        r.grant_chat(&Actor::LocalDevice, "shared", &other, ChatAccess::Read)
            .unwrap();
        assert!(r.set_companion_thread(&m, "shared", None).is_err());
        assert!(
            r.grant_chat(&Actor::LocalDevice, "ada-thread", &other, ChatAccess::Read)
                .is_err()
        );
    }

    #[test]
    fn chat_acl_filters_discovery_reads_and_writes() {
        let (mut r, m) = roster_with_member();
        let other = r.add_member(&Actor::LocalDevice, "Grace").unwrap();
        r.set_voice_enrolled(&Actor::LocalDevice, &other, true)
            .unwrap();
        r.set_companion_thread(&m, "ada-own", None).unwrap();
        let owner_lock = admit(&r, recognised(&r.owner().clone()));
        let member_lock = admit(&r, recognised(&m));
        let other_lock = admit(&r, recognised(&other));
        let chats = vec!["owner-a", "owner-b", "ada-own"];
        // Nothing shared yet: a member sees only their own companion chat.
        assert_eq!(
            r.filter_chats(&member_lock, chats.clone(), |c| c),
            vec!["ada-own"]
        );
        assert!(r.filter_chats(&other_lock, chats.clone(), |c| c).is_empty());
        // The owner cannot see a member's private chat either.
        assert_eq!(
            r.filter_chats(&owner_lock, chats.clone(), |c| c),
            vec!["owner-a", "owner-b"]
        );
        // ReadWrite cannot be the first grant.
        assert!(
            r.grant_chat(&Actor::LocalDevice, "owner-a", &m, ChatAccess::ReadWrite)
                .is_err()
        );
        r.grant_chat(&Actor::LocalDevice, "owner-a", &m, ChatAccess::Read)
            .unwrap();
        let member_lock = admit(&r, recognised(&m));
        let g = GlobalChatMode::WhenAsked;
        assert_eq!(
            r.filter_chats(&member_lock, chats.clone(), |c| c),
            vec!["owner-a", "ada-own"]
        );
        assert!(r.chat_allowed(&member_lock, "owner-a", ChatOp::Read, g));
        assert!(r.chat_allowed(&member_lock, "owner-a", ChatOp::Status, g));
        for op in [ChatOp::Send, ChatOp::Steer, ChatOp::Stop] {
            assert!(!r.chat_allowed(&member_lock, "owner-a", op, g));
        }
        assert!(!r.chat_allowed(&member_lock, "owner-b", ChatOp::Read, g));
        assert!(!r.chat_allowed(&other_lock, "owner-a", ChatOp::Discover, g));
        r.grant_chat(&Actor::LocalDevice, "owner-a", &m, ChatAccess::ReadWrite)
            .unwrap();
        assert!(r.chat_allowed(&member_lock, "owner-a", ChatOp::Send, g));
        // The global ceiling still applies.
        assert!(!r.chat_allowed(
            &member_lock,
            "owner-a",
            ChatOp::Send,
            GlobalChatMode::ReadOnly
        ));
        assert!(!r.chat_allowed(
            &owner_lock,
            "owner-a",
            ChatOp::Stop,
            GlobalChatMode::ReadOnly
        ));
        // Downgrade invalidates in-flight member turns.
        r.grant_chat(&Actor::LocalDevice, "owner-a", &m, ChatAccess::Read)
            .unwrap();
        assert!(!r.chat_allowed(&member_lock, "owner-a", ChatOp::Read, g));
        let fresh = admit(&r, recognised(&m));
        assert!(r.chat_allowed(&fresh, "owner-a", ChatOp::Read, g));
        assert!(!r.chat_allowed(&fresh, "owner-a", ChatOp::Send, g));
        r.revoke_chat(&Actor::LocalDevice, "owner-a", &m).unwrap();
        assert!(!r.chat_allowed(&fresh, "owner-a", ChatOp::Read, g));
    }

    #[test]
    fn revocation_and_mode_changes_cancel_in_flight_turns() {
        let (mut r, m) = roster_with_member();
        let owner_lock = admit(&r, recognised(&r.owner().clone()));
        let member_lock = admit(&r, recognised(&m));
        let fallback = admit(&r, SpeakerOutcome::Unknown);
        r.set_mode(&Actor::LocalDevice, IdentityMode::TrustedGroup)
            .unwrap();
        assert_eq!(r.validate_lock(&owner_lock), Err(Denied::Revoked));
        assert_eq!(r.validate_lock(&member_lock), Err(Denied::Revoked));
        assert_eq!(r.validate_lock(&fallback), Err(Denied::Revoked));
        r.set_mode(&Actor::LocalDevice, IdentityMode::OwnerOnly)
            .unwrap();
        assert_eq!(r.validate_lock(&owner_lock), Err(Denied::Revoked));
        assert_eq!(r.validate_lock(&member_lock), Err(Denied::Revoked));
        r.set_mode(&Actor::LocalDevice, IdentityMode::TrustedGroup)
            .unwrap();
        let member_lock = admit(&r, recognised(&m));
        let ns = r
            .remove_member(&Actor::LocalDevice, Path::new("/d"), &m)
            .unwrap();
        assert_eq!(ns.profile, m);
        assert_eq!(r.validate_lock(&member_lock), Err(Denied::UnknownProfile));
        // Re-enrolment also invalidates old turns.
        let owner = r.owner().clone();
        let before = admit(&r, recognised(&owner));
        r.set_voice_enrolled(&Actor::LocalDevice, &owner, true)
            .unwrap();
        assert_eq!(r.validate_lock(&before), Err(Denied::Revoked));
        let owner_id = r.owner().clone();
        assert!(
            r.remove_member(&Actor::LocalDevice, Path::new("/d"), &owner_id)
                .is_err()
        );
    }

    #[test]
    fn outbox_delivers_only_to_requesting_profile() {
        let (mut r, m) = roster_with_member();
        let owner_lock = admit(&r, recognised(&r.owner().clone()));
        let member_lock = admit(&r, recognised(&m));
        let mut out = Outbox::new();
        out.push(&owner_lock, "owner build finished");
        out.push(&member_lock, "ada reminder");
        // The member being active never releases the owner's result.
        assert_eq!(out.take_for(&r, &member_lock), vec!["ada reminder"]);
        assert_eq!(out.pending_for(r.owner()), 1);
        // Open-fallback (unknown voice) cannot collect anything either once
        // locked; in open mode it is the owner context by design.
        r.set_mode(&Actor::LocalDevice, IdentityMode::TrustedGroup)
            .unwrap();
        let stale_fallback = TurnLock {
            open_fallback: true,
            ..owner_lock.clone()
        };
        assert!(out.take_for(&r, &stale_fallback).is_empty());
        let fresh_owner = admit(&r, recognised(&r.owner().clone()));
        assert!(
            out.take_for(&r, &fresh_owner).is_empty(),
            "mode changes revoke old receipts too"
        );
        out.push(&fresh_owner, "new owner build finished");
        assert_eq!(
            out.take_for(&r, &fresh_owner),
            vec!["new owner build finished"]
        );
        // Stale epoch items are discarded, and bounds hold.
        out.push(&member_lock, "before revoke");
        r.set_mode(&Actor::LocalDevice, IdentityMode::OwnerOnly)
            .unwrap();
        r.set_mode(&Actor::LocalDevice, IdentityMode::TrustedGroup)
            .unwrap();
        let fresh = admit(&r, recognised(&m));
        assert!(out.take_for(&r, &fresh).is_empty());
        let mut out2: Outbox<i32> = Outbox::new();
        for i in 0..40 {
            out2.push(&fresh, i);
        }
        assert_eq!(out2.pending_for(&m), OUTBOX_PER_PROFILE);
        r.remove_member(&Actor::LocalDevice, Path::new("/d"), &m)
            .unwrap();
        out2.retain_known(&r);
        assert_eq!(out2.pending_for(&m), 0);
    }

    #[test]
    fn settings_scope_hides_device_file_from_members() {
        let (r, m) = roster_with_member();
        assert_eq!(
            r.settings_scope(&admit(&r, recognised(&r.owner().clone()))),
            SettingsScope::Device
        );
        assert_eq!(
            r.settings_scope(&admit(&r, recognised(&m))),
            SettingsScope::MemberSummary
        );
    }

    #[test]
    fn roster_round_trips_and_damage_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("speakers").join("roster.json");
        assert!(matches!(
            Roster::load(&path, "Kamba"),
            LoadOutcome::Fresh(_)
        ));
        let (mut r, m) = roster_with_member();
        r.set_mode(&Actor::LocalDevice, IdentityMode::TrustedGroup)
            .unwrap();
        r.grant_chat(&Actor::LocalDevice, "owner-a", &m, ChatAccess::Read)
            .unwrap();
        r.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("embedding") && !text.contains("voiceprint"));
        let LoadOutcome::Loaded(back) = Roster::load(&path, "x") else {
            panic!("load")
        };
        assert_eq!(back, r);
        // Damage: truncated JSON, hand-edited second owner, oversized file.
        for bad in [
            text[..text.len() / 2].to_owned(),
            text.replace("\"member\"", "\"owner\""),
            " ".repeat(70 * 1024),
        ] {
            std::fs::write(&path, bad).unwrap();
            let LoadOutcome::Damaged(d, _) = Roster::load(&path, "x") else {
                panic!("expected damaged")
            };
            assert!(d.recovery_required());
            let o = d.owner().clone();
            assert_eq!(
                d.admit("c", &recognised(&o)),
                Admission::Refuse(Refusal::RecoveryRequired)
            );
            assert!(d.save(&path).is_err());
        }
        // Local recovery yields open mode; member dirs are reported, not deleted.
        let member_dir = dir.path().join("profiles").join(m.as_str());
        std::fs::create_dir_all(&member_dir).unwrap();
        let fresh = Roster::recover(&Actor::LocalDevice, "Kamba").unwrap();
        assert_eq!(fresh.mode(), IdentityMode::Open);
        assert_eq!(
            fresh.orphaned_member_dirs(dir.path()),
            vec![member_dir.clone()]
        );
        assert!(member_dir.exists());
    }

    #[test]
    fn locked_roster_without_owner_voice_is_rejected_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("roster.json");
        let (mut r, _) = roster_with_member();
        r.set_mode(&Actor::LocalDevice, IdentityMode::OwnerOnly)
            .unwrap();
        r.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let owner_entry = text.find("\"owner\"").unwrap();
        let tampered = text.replacen("\"voice_enrolled\": true", "\"voice_enrolled\": false", 1);
        assert!(owner_entry > 0);
        std::fs::write(&path, tampered).unwrap();
        assert!(matches!(Roster::load(&path, "x"), LoadOutcome::Damaged(..)));
    }
}
