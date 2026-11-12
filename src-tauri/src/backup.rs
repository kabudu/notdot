//! Optional, password-encrypted backup of one person's memories and notes.
//!
//! Format (version 1), all integers big-endian:
//!
//! | bytes | field |
//! | --- | --- |
//! | 8 | magic `NOTDOTBK` |
//! | 1 | format version (1) |
//! | 1 | KDF id (1 = PBKDF2-HMAC-SHA256) |
//! | 4 | KDF iterations (exactly 600,000) |
//! | 16 | random salt |
//! | 12 | random nonce |
//! | 4 | ciphertext length including the 16-byte tag |
//! | n | ChaCha20-Poly1305 ciphertext and tag |
//!
//! The whole 46-byte header is authenticated as associated data. Every header
//! field is checked, and the length is bounded, before any key derivation, so
//! a hostile file cannot request unbounded work. Cryptography comes from
//! `ring`, already used by rustls and the voiceprint store; nothing here is a
//! custom primitive. The password is never stored.
//!
//! A backup contains current memories and notes, plus colour and reminder
//! limits for the owner. It never contains provider keys, Codex credentials,
//! voiceprints, chat grants, approvals, microphone, wake, startup or
//! proactivity switches, or anyone else's data.
use crate::AppState;
use notdot_core::{
    memory::{BACKUP_METADATA, BackupRecord, MAX_RECORDS},
    notes::{self, Notebook},
};
use ring::{
    aead::{Aad, CHACHA20_POLY1305, LessSafeKey, NONCE_LEN, Nonce, UnboundKey},
    pbkdf2,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use std::{
    num::NonZeroU32,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::State;
use zeroize::{Zeroize, Zeroizing};

const MAGIC: &[u8; 8] = b"NOTDOTBK";
const VERSION: u8 = 1;
const KDF_PBKDF2_SHA256: u8 = 1;
pub const ITERATIONS: u32 = 600_000;
const SALT_LEN: usize = 16;
const TAG_LEN: usize = 16;
const HEADER_LEN: usize = 8 + 1 + 1 + 4 + SALT_LEN + NONCE_LEN + 4;
pub const MAX_FILE: usize = 8 * 1024 * 1024;
const MAX_CIPHERTEXT: usize = MAX_FILE - HEADER_LEN;
const MIN_PASSWORD: usize = 12;
const MAX_PASSWORD: usize = 1024;
const MAX_CONTENT: usize = 4000;
const MAX_SOURCES: usize = 64;
const STAGED_FOR: Duration = Duration::from_secs(300);
pub const JOURNAL: &str = "restore-journal.json";

#[derive(Debug, PartialEq, Eq)]
pub enum OpenError {
    /// Not a notdot backup, or a damaged, truncated or oversized one.
    Malformed(&'static str),
    /// A notdot backup from a newer version.
    Unsupported,
    /// Wrong password, or a file changed after it was made.
    Authentication,
}
impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::Malformed(why) => write!(f, "This isn't a readable notdot backup ({why}). Nothing was restored."),
            OpenError::Unsupported => f.write_str("This backup was made by a newer notdot. Update notdot, then try again. Nothing was restored."),
            OpenError::Authentication => f.write_str("The password is wrong, or the backup was changed after it was made. Nothing was restored."),
        }
    }
}

fn check_password(password: &str) -> Result<(), String> {
    if password.len() < MIN_PASSWORD {
        return Err(format!(
            "Use a backup password of at least {MIN_PASSWORD} characters."
        ));
    }
    if password.len() > MAX_PASSWORD {
        return Err("That password is too long.".into());
    }
    Ok(())
}
fn derive(password: &str, salt: &[u8], iterations: u32) -> Zeroizing<[u8; 32]> {
    let mut key = Zeroizing::new([0u8; 32]);
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        NonZeroU32::new(iterations).expect("nonzero iterations"),
        salt,
        password.as_bytes(),
        &mut *key,
    );
    key
}
fn cipher(key: &[u8; 32]) -> LessSafeKey {
    LessSafeKey::new(UnboundKey::new(&CHACHA20_POLY1305, key).expect("32-byte key"))
}

fn seal_with(plaintext: &[u8], password: &str, iterations: u32) -> Result<Vec<u8>, String> {
    check_password(password)?;
    if plaintext.len() + TAG_LEN > MAX_CIPHERTEXT {
        return Err("This backup would be too large.".into());
    }
    let rng = SystemRandom::new();
    let mut salt = [0u8; SALT_LEN];
    let mut nonce = [0u8; NONCE_LEN];
    rng.fill(&mut salt)
        .and_then(|()| rng.fill(&mut nonce))
        .map_err(|_| "Secure random numbers are unavailable")?;
    let mut out = Vec::with_capacity(HEADER_LEN + plaintext.len() + TAG_LEN);
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.push(KDF_PBKDF2_SHA256);
    out.extend_from_slice(&iterations.to_be_bytes());
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&((plaintext.len() + TAG_LEN) as u32).to_be_bytes());
    let key = derive(password, &salt, iterations);
    let mut body = Zeroizing::new(plaintext.to_vec());
    let tag = cipher(&key)
        .seal_in_place_separate_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(&out[..HEADER_LEN]),
            &mut body,
        )
        .map_err(|_| "Encryption failed")?;
    out.extend_from_slice(&body);
    out.extend_from_slice(tag.as_ref());
    Ok(out)
}
pub fn seal(plaintext: &[u8], password: &str) -> Result<Vec<u8>, String> {
    seal_with(plaintext, password, ITERATIONS)
}

struct Header {
    iterations: u32,
    salt: [u8; SALT_LEN],
    nonce: [u8; NONCE_LEN],
}
/// Check every header field and the exact length without any key work.
fn header(bytes: &[u8], accepted_iterations: u32) -> Result<Header, OpenError> {
    if bytes.len() > MAX_FILE {
        return Err(OpenError::Malformed("too large"));
    }
    if bytes.len() < HEADER_LEN + TAG_LEN || &bytes[..8] != MAGIC {
        return Err(OpenError::Malformed("not a backup"));
    }
    match bytes[8] {
        VERSION => {}
        0 => return Err(OpenError::Malformed("unknown version")),
        _ => return Err(OpenError::Unsupported),
    }
    if bytes[9] != KDF_PBKDF2_SHA256 {
        return Err(OpenError::Unsupported);
    }
    let iterations = u32::from_be_bytes(bytes[10..14].try_into().expect("4 bytes"));
    if iterations != accepted_iterations {
        return Err(OpenError::Malformed("unexpected key settings"));
    }
    let length = u32::from_be_bytes(bytes[42..46].try_into().expect("4 bytes")) as usize;
    if !(TAG_LEN..=MAX_CIPHERTEXT).contains(&length) || HEADER_LEN + length != bytes.len() {
        return Err(OpenError::Malformed("damaged or truncated"));
    }
    Ok(Header {
        iterations,
        salt: bytes[14..30].try_into().expect("salt"),
        nonce: bytes[30..42].try_into().expect("nonce"),
    })
}
fn open_with(
    bytes: &[u8],
    password: &str,
    accepted_iterations: u32,
) -> Result<Zeroizing<Vec<u8>>, OpenError> {
    let header = header(bytes, accepted_iterations)?;
    if password.is_empty() || password.len() > MAX_PASSWORD {
        return Err(OpenError::Authentication);
    }
    let key = derive(password, &header.salt, header.iterations);
    let mut body = Zeroizing::new(bytes[HEADER_LEN..].to_vec());
    let plain = cipher(&key)
        .open_in_place(
            Nonce::assume_unique_for_key(header.nonce),
            Aad::from(&bytes[..HEADER_LEN]),
            &mut body,
        )
        .map_err(|_| OpenError::Authentication)?
        .len();
    body.truncate(plain);
    Ok(body)
}
pub fn open(bytes: &[u8], password: &str) -> Result<Zeroizing<Vec<u8>>, OpenError> {
    open_with(bytes, password, ITERATIONS)
}

/// Owner-only preferences that change appearance or reduce announcements.
/// Nothing here can enable listening, startup, chat access or proactivity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    pub hue: u16,
    pub quiet_start: u16,
    pub quiet_end: u16,
    pub daily_limit: u8,
    pub gap_minutes: u16,
    pub followup_seconds: u32,
}
impl Preferences {
    fn from_settings(s: &notdot_core::settings::Settings) -> Self {
        Self {
            hue: s.hue,
            quiet_start: s.initiative.quiet_start,
            quiet_end: s.initiative.quiet_end,
            daily_limit: s.initiative.daily_limit,
            gap_minutes: s.initiative.gap_minutes,
            followup_seconds: s.followup_seconds,
        }
    }
    fn apply(&self, s: &mut notdot_core::settings::Settings) {
        s.hue = self.hue;
        s.initiative.quiet_start = self.quiet_start;
        s.initiative.quiet_end = self.quiet_end;
        s.initiative.daily_limit = self.daily_limit;
        s.initiative.gap_minutes = self.gap_minutes;
        s.followup_seconds = self.followup_seconds;
    }
    fn valid(&self) -> bool {
        self.hue <= 360
            && self.quiet_start < 1440
            && self.quiet_end < 1440
            && (1..=10).contains(&self.daily_limit)
            && (15..=1440).contains(&self.gap_minutes)
            && (3..=20).contains(&self.followup_seconds)
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Payload {
    pub format: String,
    pub schema: u32,
    pub created_unix_ms: u64,
    pub memories: Vec<BackupRecord>,
    pub notes: Notebook,
    pub preferences: Option<Preferences>,
}
fn uuid(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|u| u.hyphenated().to_string() == id)
}
impl Payload {
    /// Reject anything malformed before planning a restore.
    pub fn validate(&self) -> Result<(), String> {
        if self.format != "notdot-backup" || self.schema != 1 {
            return Err("This backup's contents are in an unsupported format.".into());
        }
        if self.memories.len() > MAX_RECORDS {
            return Err("This backup has more memories than notdot can keep.".into());
        }
        let ids: std::collections::BTreeSet<_> =
            self.memories.iter().map(|r| r.id.as_str()).collect();
        if ids.len() != self.memories.len() {
            return Err("This backup repeats a memory.".into());
        }
        for r in &self.memories {
            let bad = !uuid(&r.id)
                || r.content.trim().is_empty()
                || r.content.len() > MAX_CONTENT
                || r.content.chars().any(|c| c.is_control() && c != '\n')
                || r.sources.len() > MAX_SOURCES
                || r.sources
                    .iter()
                    .any(|s| s == &r.id || !ids.contains(s.as_str()))
                || (r.summary && r.sources.is_empty())
                || r.metadata.len() > BACKUP_METADATA.len()
                || r.metadata
                    .iter()
                    .any(|(k, v)| !BACKUP_METADATA.contains(&k.as_str()) || v.len() > MAX_CONTENT);
            if bad {
                return Err("This backup contains a memory notdot can't use.".into());
            }
        }
        // Summaries may only cite facts, never other summaries.
        let summaries: std::collections::BTreeSet<_> = self
            .memories
            .iter()
            .filter(|r| r.summary)
            .map(|r| r.id.as_str())
            .collect();
        if self
            .memories
            .iter()
            .any(|r| r.sources.iter().any(|s| summaries.contains(s.as_str())))
        {
            return Err("This backup contains a memory notdot can't use.".into());
        }
        self.notes
            .validate()
            .map_err(|_| "This backup contains notes notdot can't use.".to_string())?;
        if self.preferences.as_ref().is_some_and(|p| !p.valid()) {
            return Err("This backup contains settings notdot can't use.".into());
        }
        Ok(())
    }
}

/// What a confirmed restore will add. Existing data is never replaced.
#[derive(Clone, Debug)]
pub struct Plan {
    pub memories: Vec<BackupRecord>,
    pub skipped_memories: usize,
    pub notebook: Notebook,
    pub added_notes: usize,
    pub renamed_notes: usize,
    pub preferences: Option<Preferences>,
}
/// Merge a backup into current data with fresh IDs. Matching memories are
/// kept once; a note whose name is already used is added as "(restored)".
pub fn plan(
    payload: Payload,
    existing_memories: &std::collections::BTreeMap<String, String>,
    current_notes: &Notebook,
    owner: bool,
) -> Result<Plan, String> {
    payload.validate()?;
    let mut ids = std::collections::BTreeMap::new();
    let mut memories = Vec::new();
    let mut skipped = 0;
    // Facts before summaries, so every source has its final ID.
    let mut ordered: Vec<_> = payload.memories.into_iter().collect();
    ordered.sort_by_key(|r| r.summary);
    for mut r in ordered {
        if let Some(existing) = existing_memories.get(&r.content.trim().to_lowercase()) {
            ids.insert(r.id.clone(), existing.clone());
            skipped += 1;
            continue;
        }
        let fresh = uuid::Uuid::new_v4().to_string();
        ids.insert(std::mem::replace(&mut r.id, fresh.clone()), fresh);
        memories.push(r);
    }
    for r in &mut memories {
        r.sources = r
            .sources
            .iter()
            .map(|s| {
                ids.get(s)
                    .cloned()
                    .ok_or("This backup contains a broken memory link.")
            })
            .collect::<Result<_, _>>()?;
    }
    if existing_memories.len() + memories.len() > MAX_RECORDS {
        return Err("Restoring would pass the 2,000 memory limit. Nothing was restored.".into());
    }
    let mut notebook = current_notes.clone();
    notebook.version = 1;
    let mut added = 0;
    let mut renamed = 0;
    for mut note in payload.notes.notes {
        if notebook.notes.iter().any(|n| {
            notes::normalise(&n.title) == notes::normalise(&note.title)
                && n.kind == note.kind
                && n.body == note.body
                && n.items
                    .iter()
                    .map(|i| (&i.text, i.done))
                    .eq(note.items.iter().map(|i| (&i.text, i.done)))
        }) {
            continue;
        }
        note.id = uuid::Uuid::new_v4().to_string();
        for item in &mut note.items {
            item.id = uuid::Uuid::new_v4().to_string();
        }
        if notebook
            .notes
            .iter()
            .any(|n| notes::normalise(&n.title) == notes::normalise(&note.title))
        {
            let base: String = note
                .title
                .chars()
                .take(notes::MAX_TITLE_CHARS - 11)
                .collect();
            note.title = format!("{} (restored)", base.trim_end());
            if notebook
                .notes
                .iter()
                .any(|n| notes::normalise(&n.title) == notes::normalise(&note.title))
            {
                return Err(format!(
                    "You already have a note called {}. Rename it, then restore again.",
                    note.title
                ));
            }
            renamed += 1;
        }
        added += 1;
        notebook.notes.push(note);
    }
    if notebook.notes.len() > notes::MAX_NOTES {
        return Err("Restoring would pass the 200 note limit. Nothing was restored.".into());
    }
    if added > 0 {
        notebook.revision = notebook
            .revision
            .checked_add(1)
            .filter(|r| *r < notes::MAX_REVISION)
            .ok_or("Your notes can't take more changes.")?;
    }
    notebook.validate()?;
    Ok(Plan {
        memories,
        skipped_memories: skipped,
        notebook,
        added_notes: added,
        renamed_notes: renamed,
        preferences: payload.preferences.filter(|_| owner),
    })
}

/// Written and flushed before any change, removed only after all succeed.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u32,
    inserted: Vec<String>,
    /// `None` when there was no notes file.
    previous_notes: Option<Notebook>,
    previous_settings: Option<notdot_core::settings::Settings>,
}
fn journal_path(state_dir: &Path) -> PathBuf {
    state_dir.join(JOURNAL)
}
/// Whether a restore journal (or anything at its path) is waiting.
pub fn journal_pending(state_dir: &Path) -> bool {
    std::fs::symlink_metadata(journal_path(state_dir)).is_ok()
}
/// While a journal exists, ordinary note and memory changes are refused:
/// undoing the restore puts back the earlier notebook and removes the
/// restored memories, so a later change could otherwise be lost silently.
pub fn hold(context: &crate::profile_context::ProfileContext, held: bool) {
    context.notes.hold_for_restore(held);
    context.memory.hold_for_restore(held);
}
/// Put back only the preferences a restore changes, so other settings
/// changed since then are kept.
fn restore_preferences(
    previous: &notdot_core::settings::Settings,
    path: &Path,
) -> Result<(), String> {
    let mut current = notdot_core::settings::Settings::load(path)?;
    Preferences::from_settings(previous).apply(&mut current);
    current.save(path)
}
fn read_journal(state_dir: &Path) -> Result<Option<Journal>, String> {
    let path = journal_path(state_dir);
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Ok(m) if !m.file_type().is_file() => {
            return Err("Restore journal is not a regular file".into());
        }
        _ => {}
    }
    let bytes = crate::file_dialog::read_bounded(&path, 4 * 1024 * 1024)
        .map_err(|e| format!("Read restore journal: {e}"))?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| format!("Read restore journal: {e}"))
}
fn remove_journal(state_dir: &Path) -> Result<(), String> {
    match std::fs::remove_file(journal_path(state_dir)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            return Err(format!("Finish restore: {e}"));
        }
        _ => {}
    }
    #[cfg(unix)]
    std::fs::File::open(state_dir)
        .and_then(|d| d.sync_all())
        .map_err(|e| format!("Finish restore: {e}"))?;
    Ok(())
}

/// Put back the settings recorded by an unfinished owner restore. Runs at
/// startup before settings are loaded; the journal stays for
/// [`recover_profile`].
pub fn recover_settings(state_dir: &Path, settings_path: &Path) -> Result<(), String> {
    if let Some(Journal {
        previous_settings: Some(settings),
        ..
    }) = read_journal(state_dir)?
    {
        restore_preferences(&settings, settings_path)?;
    }
    Ok(())
}
/// Undo an unfinished restore for one person: remove its memories, put the
/// previous notebook back, then drop the journal. Safe to repeat.
pub async fn recover_profile(
    context: &crate::profile_context::ProfileContext,
    settings_path: Option<&Path>,
) -> Result<bool, String> {
    let journal = match read_journal(&context.state_dir) {
        Ok(Some(journal)) => journal,
        Ok(None) => {
            hold(context, false);
            return Ok(false);
        }
        Err(error) => {
            hold(context, true);
            return Err(error);
        }
    };
    // Stays held until every step below has succeeded.
    hold(context, true);
    context.memory.remove_records(&journal.inserted).await?;
    let notes_path = context.notes.path().to_owned();
    match &journal.previous_notes {
        Some(book) => crate::notes::save_file(&notes_path, book).map_err(String::from)?,
        None => match std::fs::remove_file(&notes_path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(format!("Undo restore: {e}"));
            }
            _ => {}
        },
    }
    context.notes.reload().await;
    if let (Some(settings), Some(path)) = (&journal.previous_settings, settings_path) {
        restore_preferences(settings, path)?;
    }
    remove_journal(&context.state_dir)?;
    hold(context, false);
    Ok(true)
}

/// Failure injection for the restore transaction in tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Memories,
    Notes,
    Settings,
}

/// Apply a plan as one transaction for this person. The caller holds the
/// submission gate and the person's turn lock. On any failure every part is
/// undone; if undoing fails too, the journal stays and the next start
/// finishes it.
pub async fn apply(
    context: &crate::profile_context::ProfileContext,
    plan: &Plan,
    expected: &Revisions,
    settings: Option<(&Path, &mut notdot_core::settings::Settings)>,
    fail_at: Option<Step>,
) -> Result<(), String> {
    if read_journal(&context.state_dir)?.is_some() {
        return Err(
            "A previous restore is still being undone. Restart notdot, then try again.".into(),
        );
    }
    let previous_notes = match std::fs::symlink_metadata(context.notes.path()) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        _ => Some(context.notes.snapshot().await?),
    };
    let (settings_path, settings) = match settings {
        Some((path, settings)) if plan.preferences.is_some() => (Some(path), Some(settings)),
        _ => (None, None),
    };
    let journal = Journal {
        version: 1,
        inserted: plan.memories.iter().map(|r| r.id.clone()).collect(),
        previous_notes,
        previous_settings: settings.as_ref().map(|s| (**s).clone()),
    };
    let bytes = serde_json::to_vec(&journal).map_err(|e| e.to_string())?;
    if let Err(e) = crate::file_dialog::write_atomic(&journal_path(&context.state_dir), &bytes) {
        // An unconfirmed journal may still be on disk and would be undone.
        hold(context, journal_pending(&context.state_dir));
        return Err(format!("Could not start the restore: {e}"));
    }
    hold(context, true);
    context.learning.cancel();
    let result = async {
        if fail_at == Some(Step::Memories) {
            return Err("injected memory failure".to_string());
        }
        context
            .memory
            .import_records(&plan.memories, expected.memory)
            .await?;
        if fail_at == Some(Step::Notes) {
            return Err("injected notes failure".to_string());
        }
        if plan.added_notes > 0 {
            context
                .notes
                .replace(plan.notebook.clone(), expected.notes)
                .await?;
        }
        if let (Some(path), Some(prefs)) = (settings_path, &plan.preferences) {
            if fail_at == Some(Step::Settings) {
                return Err("injected settings failure".to_string());
            }
            let mut next = settings.as_ref().map(|s| (**s).clone()).expect("settings");
            prefs.apply(&mut next);
            next.save(path)?;
        }
        Ok(())
    }
    .await;
    match result {
        Ok(()) => {
            if let (Some(settings), Some(prefs)) = (settings, &plan.preferences) {
                prefs.apply(settings);
            }
            // If the journal can't be removed, the next start undoes this
            // restore; changes stay paused so nothing newer is lost then.
            remove_journal(&context.state_dir).map_err(|_| {
                "Restored, but notdot couldn't finish recording it. Restart notdot: it will undo this restore, then you can restore again. Note and memory changes are paused until then.".to_string()
            })?;
            hold(context, false);
            Ok(())
        }
        Err(error) => {
            let undo = recover_profile(context, settings_path).await;
            Err(match undo {
                Ok(_) => format!("Restore failed: {error}. Your existing data was kept as it was."),
                Err(undo) => format!(
                    "Restore failed: {error}. Undoing it also failed ({undo}); restart notdot to finish undoing it."
                ),
            })
        }
    }
}

/// The exact state a preview was computed against. A restore is applied
/// only if every part is unchanged; otherwise a fresh preview is needed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Revisions {
    profile: notdot_core::profiles::ProfileId,
    profile_epoch: u64,
    memory: u64,
    notes: u64,
    /// Serialised settings, for an owner restore that includes preferences.
    settings: Option<String>,
}
/// One person, captured once when an export or restore starts and used for
/// the whole operation, however long a dialog or key derivation takes.
pub struct Admitted {
    context: Arc<crate::profile_context::ProfileContext>,
    epoch: u64,
    owner: bool,
}
impl Admitted {
    pub fn capture(state: &AppState) -> Result<Self, String> {
        let context = state.context();
        let roster = state.speakers.roster()?;
        let epoch = roster
            .profile(&context.id)
            .ok_or("This person is no longer set up")?
            .epoch;
        Ok(Self {
            owner: context.id == *roster.owner(),
            context,
            epoch,
        })
    }
    pub fn context(&self) -> &Arc<crate::profile_context::ProfileContext> {
        &self.context
    }
    /// Still the active person, with the same permissions epoch.
    pub fn check(&self, state: &AppState) -> Result<(), String> {
        let roster = state.speakers.roster()?;
        if state.context().id != self.context.id
            || roster.profile(&self.context.id).map(|p| p.epoch) != Some(self.epoch)
        {
            return Err("The active person changed. Nothing was changed or saved.".into());
        }
        Ok(())
    }
    async fn revisions(&self, state: &AppState) -> Result<Revisions, String> {
        let settings = if self.owner {
            Some(
                serde_json::to_string(&state.snapshot.lock().await.settings)
                    .map_err(|e| e.to_string())?,
            )
        } else {
            None
        };
        Ok(Revisions {
            profile: self.context.id.clone(),
            profile_epoch: self.epoch,
            memory: self.context.memory.revision(),
            notes: self.context.notes.snapshot().await?.revision,
            settings,
        })
    }
}

struct Staged {
    id: String,
    plan: Plan,
    revisions: Revisions,
    created: Instant,
}
/// At most one decrypted restore waits for confirmation, in memory only.
#[derive(Default)]
pub struct Backups(std::sync::Mutex<Option<Staged>>);
impl Backups {
    fn stage(&self, plan: Plan, revisions: Revisions) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        *self.0.lock().expect("backups") = Some(Staged {
            id: id.clone(),
            plan,
            revisions,
            created: Instant::now(),
        });
        id
    }
    /// Single use: the staged plan is removed whether or not it matches.
    fn take(&self, id: &str) -> Result<(Plan, Revisions), String> {
        let staged = self
            .0
            .lock()
            .expect("backups")
            .take()
            .ok_or("That restore is no longer waiting. Choose the backup again.")?;
        if staged.id != id || staged.created.elapsed() > STAGED_FOR {
            return Err("That restore is no longer waiting. Choose the backup again.".into());
        }
        Ok((staged.plan, staged.revisions))
    }
    pub fn cancel(&self) {
        self.0.lock().expect("backups").take();
    }
}

#[derive(Debug, Serialize)]
pub struct Preview {
    id: String,
    created_unix_ms: u64,
    memories: usize,
    skipped_memories: usize,
    notes: usize,
    renamed_notes: usize,
    preferences: bool,
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Encrypt the admitted person's data. Key derivation runs off-thread.
pub async fn export_bytes(
    state: &AppState,
    who: &Admitted,
    password: &str,
) -> Result<(Vec<u8>, usize, usize), String> {
    check_password(password)?;
    let memories = who.context.memory.backup_records().await?;
    let notes = who.context.notes.snapshot().await?;
    let preferences = if who.owner {
        Some(Preferences::from_settings(
            &state.snapshot.lock().await.settings,
        ))
    } else {
        None
    };
    who.check(state)?;
    let counts = (memories.len(), notes.notes.len());
    let payload = Payload {
        format: "notdot-backup".into(),
        schema: 1,
        created_unix_ms: now_ms(),
        memories,
        notes,
        preferences,
    };
    let plaintext = Zeroizing::new(serde_json::to_vec(&payload).map_err(|e| e.to_string())?);
    drop(payload);
    let password = Zeroizing::new(password.to_owned());
    let sealed = tokio::task::spawn_blocking(move || seal(&plaintext, &password))
        .await
        .map_err(|e| e.to_string())??;
    Ok((sealed, counts.0, counts.1))
}
/// Decrypt, validate and plan a restore for the admitted person.
pub async fn inspect_bytes(
    state: &AppState,
    who: &Admitted,
    bytes: Vec<u8>,
    password: &str,
) -> Result<Preview, String> {
    let password = Zeroizing::new(password.to_owned());
    let plaintext = tokio::task::spawn_blocking(move || open(&bytes, &password))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let payload: Payload = serde_json::from_slice(&plaintext).map_err(|_| {
        "This backup's contents could not be read. Nothing was restored.".to_string()
    })?;
    drop(plaintext);
    // Revisions first: any later change makes this preview stale.
    let revisions = who.revisions(state).await?;
    let existing = who.context.memory.current_contents().await?;
    let current = who.context.notes.snapshot().await?;
    if current.revision != revisions.notes || who.context.memory.revision() != revisions.memory {
        return Err("Your data changed while the backup was checked. Choose it again.".into());
    }
    let created = payload.created_unix_ms;
    let plan = plan(payload, &existing, &current, who.owner)?;
    who.check(state)?;
    let preview = Preview {
        id: String::new(),
        created_unix_ms: created,
        memories: plan.memories.len(),
        skipped_memories: plan.skipped_memories,
        notes: plan.added_notes,
        renamed_notes: plan.renamed_notes,
        preferences: plan.preferences.is_some(),
    };
    let id = state.backups.stage(plan, revisions);
    Ok(Preview { id, ..preview })
}
/// Apply a staged restore only to the exact state it was previewed against.
pub async fn restore_staged(state: &AppState, id: &str) -> Result<String, String> {
    let (plan, expected) = state.backups.take(id)?;
    let who = Admitted::capture(state)?;
    let _submission = state
        .send_gate
        .try_lock()
        .map_err(|_| "Wait for the current reply, then choose the backup again.")?;
    let _turn = who.context.turn_lock.lock().await;
    // The snapshot lock also blocks settings saves for the whole transaction.
    let mut snapshot = state.snapshot.lock().await;
    let now = Revisions {
        profile: who.context.id.clone(),
        profile_epoch: who.epoch,
        memory: who.context.memory.revision(),
        notes: who.context.notes.snapshot().await?.revision,
        settings: if who.owner {
            Some(serde_json::to_string(&snapshot.settings).map_err(|e| e.to_string())?)
        } else {
            None
        },
    };
    who.check(state)?;
    if now != expected {
        return Err(if now.profile != expected.profile || now.profile_epoch != expected.profile_epoch {
            "This restore was prepared for a different person. Nothing was restored."
        } else {
            "Your memories, notes or settings changed after the preview. Nothing was restored; choose the backup again for a fresh preview."
        }
        .into());
    }
    let settings = who
        .owner
        .then_some((state.path.as_path(), &mut snapshot.settings));
    apply(&who.context, &plan, &expected, settings, None).await?;
    let added = plan.memories.len();
    Ok(format!(
        "Restored {added} {} and {} {}.{}",
        if added == 1 { "memory" } else { "memories" },
        plan.added_notes,
        if plan.added_notes == 1 {
            "note"
        } else {
            "notes"
        },
        if plan.preferences.is_some() {
            " Orb colour, quiet hours, reminder limits and follow-up length were restored."
        } else {
            ""
        }
    ))
}

#[tauri::command]
pub async fn backup_export(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    mut password: String,
) -> Result<String, String> {
    let result = async {
        check_password(&password)?;
        // Choose the person before the dialog; never re-read it afterwards.
        let who = Admitted::capture(&state)?;
        let Some(path) = crate::file_dialog::choose_save(
            &window,
            "Save encrypted backup",
            "notdot backup.notdotbackup",
            ("notdot backup", &["notdotbackup"]),
        )
        .await
        else {
            return Ok("Backup cancelled. Nothing was saved.".to_owned());
        };
        who.check(&state)?;
        let (bytes, memories, notes) = export_bytes(&state, &who, &password).await?;
        who.check(&state)?;
        tokio::task::spawn_blocking(move || crate::file_dialog::write_atomic(&path, &bytes))
            .await
            .map_err(|e| e.to_string())??;
        Ok(format!(
            "Saved an encrypted backup of {memories} {} and {notes} {}. Keep the password safe: notdot can't recover it.",
            if memories == 1 { "memory" } else { "memories" },
            if notes == 1 { "note" } else { "notes" }
        ))
    }
    .await;
    password.zeroize();
    result
}
#[tauri::command]
pub async fn backup_inspect(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    mut password: String,
) -> Result<Option<Preview>, String> {
    let result = async {
        state.backups.cancel();
        let who = Admitted::capture(&state)?;
        let Some(path) = crate::file_dialog::choose_open(
            &window,
            "Choose an encrypted backup",
            ("notdot backup", &["notdotbackup"]),
        )
        .await
        else {
            return Ok(None);
        };
        who.check(&state)?;
        let bytes = tokio::task::spawn_blocking(move || {
            crate::file_dialog::read_bounded(&path, MAX_FILE as u64)
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::FileTooLarge {
                OpenError::Malformed("too large").to_string()
            } else {
                format!("Could not read the backup: {e}")
            }
        })?;
        inspect_bytes(&state, &who, bytes, &password)
            .await
            .map(Some)
    }
    .await;
    password.zeroize();
    result
}
#[tauri::command]
pub async fn backup_restore(state: State<'_, Arc<AppState>>, id: String) -> Result<String, String> {
    restore_staged(&state, &id).await
}
#[tauri::command]
pub fn backup_cancel(state: State<'_, Arc<AppState>>) {
    state.backups.cancel();
}

#[cfg(test)]
mod tests {
    use super::*;
    const FAST: u32 = 1000;
    const PASSWORD: &str = "correct horse battery";

    #[test]
    fn real_settings_round_trip_and_bounded_header_checks_run_before_key_work() {
        let sealed = seal(b"{\"hello\":1}", PASSWORD).unwrap();
        assert_eq!(&*open(&sealed, PASSWORD).unwrap(), b"{\"hello\":1}");
        // Each header rejection happens without deriving a key: these return
        // the specific format error, not an authentication failure.
        let mut v = sealed.clone();
        v[8] = 2;
        assert_eq!(open(&v, PASSWORD).unwrap_err(), OpenError::Unsupported);
        let mut v = sealed.clone();
        v[9] = 7;
        assert_eq!(open(&v, PASSWORD).unwrap_err(), OpenError::Unsupported);
        let mut v = sealed.clone();
        v[10..14].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(matches!(open(&v, PASSWORD), Err(OpenError::Malformed(_))));
        let mut v = sealed.clone();
        v[42..46].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(matches!(open(&v, PASSWORD), Err(OpenError::Malformed(_))));
        assert!(matches!(
            open(&sealed[..sealed.len() - 1], PASSWORD),
            Err(OpenError::Malformed(_))
        ));
        assert!(matches!(
            open(b"not a backup at all", PASSWORD),
            Err(OpenError::Malformed(_))
        ));
        assert!(matches!(
            open(&vec![0; MAX_FILE + 1], PASSWORD),
            Err(OpenError::Malformed(_))
        ));
    }

    #[test]
    fn wrong_passwords_and_any_tampering_fail_authentication() {
        let sealed = seal_with(b"private", PASSWORD, FAST).unwrap();
        assert_eq!(&*open_with(&sealed, PASSWORD, FAST).unwrap(), b"private");
        assert_eq!(
            open_with(&sealed, "wrong password!!", FAST).unwrap_err(),
            OpenError::Authentication
        );
        // Flip one bit in the salt, nonce, ciphertext and tag.
        for at in [20, 35, HEADER_LEN, sealed.len() - 1] {
            let mut v = sealed.clone();
            v[at] ^= 1;
            assert_eq!(
                open_with(&v, PASSWORD, FAST).unwrap_err(),
                OpenError::Authentication,
                "byte {at}"
            );
        }
        // Production refuses a file asking for different key settings.
        assert!(matches!(
            open(&sealed, PASSWORD),
            Err(OpenError::Malformed(_))
        ));
        assert!(seal(b"x", "short").is_err());
        // Fresh salt and nonce every time.
        assert_ne!(
            seal_with(b"x", PASSWORD, FAST).unwrap(),
            seal_with(b"x", PASSWORD, FAST).unwrap()
        );
    }

    fn record(id: &str, content: &str, sources: &[&str], summary: bool) -> BackupRecord {
        BackupRecord {
            id: id.into(),
            summary,
            content: content.into(),
            metadata: [("origin".to_owned(), "voice confirmation".to_owned())].into(),
            created_unix_ms: 1,
            updated_unix_ms: 2,
            sources: sources.iter().map(|s| (*s).to_owned()).collect(),
        }
    }
    fn payload(memories: Vec<BackupRecord>) -> Payload {
        Payload {
            format: "notdot-backup".into(),
            schema: 1,
            created_unix_ms: 1,
            memories,
            notes: Notebook::default(),
            preferences: None,
        }
    }
    #[test]
    fn payloads_with_bad_references_or_contents_are_rejected() {
        let a = uuid::Uuid::new_v4().to_string();
        let b = uuid::Uuid::new_v4().to_string();
        let c = uuid::Uuid::new_v4().to_string();
        let good = payload(vec![
            record(&a, "fact a", &[], false),
            record(&b, "fact b", &[], false),
            record(&c, "a and b", &[&a, &b], true),
        ]);
        good.validate().unwrap();
        let cases = [
            payload(vec![record(&a, "dangling", &[&b], false)]),
            payload(vec![record(&a, "self", &[&a], false)]),
            payload(vec![
                record(&a, "x", &[], false),
                record(&a, "dup", &[], false),
            ]),
            payload(vec![record("../x", "bad id", &[], false)]),
            payload(vec![record(&a, "summary without sources", &[], true)]),
            payload(vec![record(&a, &"x".repeat(MAX_CONTENT + 1), &[], false)]),
            payload(vec![
                record(&a, "s1", &[&b], true),
                record(&b, "f", &[], false),
                record(&c, "s2", &[&a], true),
            ]),
            {
                let mut p = payload(vec![record(&a, "x", &[], false)]);
                p.memories[0]
                    .metadata
                    .insert("source_thread".into(), "chat".into());
                p
            },
            {
                let mut p = payload(vec![]);
                p.schema = 2;
                p
            },
            {
                let mut p = payload(vec![]);
                p.preferences = Some(Preferences {
                    hue: 999,
                    quiet_start: 0,
                    quiet_end: 0,
                    daily_limit: 1,
                    gap_minutes: 15,
                    followup_seconds: 8,
                });
                p
            },
        ];
        for (i, case) in cases.iter().enumerate() {
            assert!(case.validate().is_err(), "case {i}");
        }
        // Unknown fields anywhere are rejected while parsing.
        let json = serde_json::to_value(&good).unwrap();
        let mut extra = json.clone();
        extra["memories"][0]["chat_grant"] = "x".into();
        assert!(serde_json::from_value::<Payload>(extra).is_err());
        let mut extra = json;
        extra["provider_key"] = "x".into();
        assert!(serde_json::from_value::<Payload>(extra).is_err());
    }

    #[test]
    fn plans_merge_without_replacing_existing_data_and_remap_links() {
        let a = uuid::Uuid::new_v4().to_string();
        let b = uuid::Uuid::new_v4().to_string();
        let s = uuid::Uuid::new_v4().to_string();
        let mut p = payload(vec![
            record(&a, "Already here", &[], false),
            record(&b, "New fact", &[], false),
            record(&s, "Summary", &[&a, &b], true),
        ]);
        let mut book = Notebook::default();
        let mut session = notes::Session::default();
        let notes::Outcome::Changed(current, _) = notes::handle(
            &book,
            &mut session,
            "create a list called Groceries with milk",
            0,
        ) else {
            panic!()
        };
        book = current;
        p.notes = book.clone();
        p.notes.notes[0].items[0].text = "bread".into();
        p.preferences = Some(Preferences {
            hue: 10,
            quiet_start: 0,
            quiet_end: 60,
            daily_limit: 1,
            gap_minutes: 15,
            followup_seconds: 8,
        });
        let existing_id = uuid::Uuid::new_v4().to_string();
        let existing = [("already here".to_owned(), existing_id.clone())].into();
        let plan = plan(p, &existing, &book, false).unwrap();
        assert_eq!(plan.skipped_memories, 1);
        assert_eq!(plan.memories.len(), 2);
        let summary = plan.memories.iter().find(|r| r.summary).unwrap();
        let new_fact = plan
            .memories
            .iter()
            .find(|r| r.content == "New fact")
            .unwrap();
        assert!(summary.sources.contains(&existing_id));
        assert!(summary.sources.contains(&new_fact.id));
        assert!(
            plan.memories
                .iter()
                .all(|r| r.id != a && r.id != b && r.id != s)
        );
        // The existing list is untouched; the backup's version is added.
        assert_eq!(plan.notebook.notes.len(), 2);
        assert_eq!(plan.notebook.notes[0], book.notes[0]);
        assert_eq!(plan.notebook.notes[1].title, "Groceries (restored)");
        assert_ne!(plan.notebook.notes[1].id, book.notes[0].id);
        // A member's restore never carries preferences.
        assert!(plan.preferences.is_none());
    }

    async fn seed(state: &Arc<AppState>, fact: &str, note: &str) {
        let context = state.context();
        context.memory.set_learning_enabled(true).await;
        context.memory.remember(fact, "seed").await.unwrap();
        context.notes.command(note).await.unwrap();
    }
    async fn contents(state: &AppState) -> (Vec<String>, Vec<String>) {
        let context = state.context();
        let mut memories: Vec<_> = context
            .memory
            .backup_records()
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.content)
            .collect();
        memories.sort();
        let mut notes: Vec<_> = context
            .notes
            .snapshot()
            .await
            .unwrap()
            .notes
            .into_iter()
            .map(|n| n.title)
            .collect();
        notes.sort();
        (memories, notes)
    }

    #[tokio::test]
    async fn export_then_confirmed_restore_merges_without_granting_permissions() {
        let source_dir = tempfile::tempdir().unwrap();
        let source = crate::test_support::state(source_dir.path());
        seed(
            &source,
            "I prefer jasmine tea",
            "create a list called Groceries with milk",
        )
        .await;
        source.snapshot.lock().await.settings.hue = 42;
        let (bytes, memories, notes) =
            export_bytes(&source, &Admitted::capture(&source).unwrap(), PASSWORD)
                .await
                .unwrap();
        assert_eq!((memories, notes), (1, 1));
        // Nothing private is readable in the file.
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("jasmine") && !text.contains("Groceries"));

        let target_dir = tempfile::tempdir().unwrap();
        let target = crate::test_support::state(target_dir.path());
        seed(
            &target,
            "I live in Leeds",
            "create a note called Ideas saying owls",
        )
        .await;
        let before = target.snapshot.lock().await.settings.clone();
        assert!(
            inspect_bytes(
                &target,
                &Admitted::capture(&target).unwrap(),
                bytes.clone(),
                "wrong password!!"
            )
            .await
            .is_err()
        );
        let preview = inspect_bytes(
            &target,
            &Admitted::capture(&target).unwrap(),
            bytes.clone(),
            PASSWORD,
        )
        .await
        .unwrap();
        assert_eq!(
            (preview.memories, preview.notes, preview.preferences),
            (1, 1, true)
        );
        // Previewing changes nothing.
        assert_eq!(
            contents(&target).await.0,
            vec!["I live in Leeds".to_owned()]
        );
        let reply = restore_staged(&target, &preview.id).await.unwrap();
        assert!(
            reply.starts_with("Restored 1 memory and 1 note."),
            "{reply}"
        );
        assert_eq!(
            contents(&target).await,
            (
                vec![
                    "I live in Leeds".to_owned(),
                    "I prefer jasmine tea".to_owned()
                ],
                vec!["Groceries".to_owned(), "Ideas".to_owned()]
            )
        );
        let after = target.snapshot.lock().await.settings.clone();
        assert_eq!(after.hue, 42);
        let saved = notdot_core::settings::Settings::load(&target.path).unwrap();
        assert_eq!(saved.hue, 42);
        // Permissions and listening are untouched.
        for (a, b) in [
            (after.microphone_muted, before.microphone_muted),
            (after.wake_enabled, before.wake_enabled),
            (after.memory_enabled, before.memory_enabled),
            (after.followup_enabled, before.followup_enabled),
            (after.task_notifications, before.task_notifications),
            (after.open_desktop, before.open_desktop),
        ] {
            assert_eq!(a, b);
        }
        assert_eq!(after.startup, before.startup);
        assert_eq!(after.session_control, before.session_control);
        assert_eq!(after.initiative.behaviour, before.initiative.behaviour);
        // One use only.
        assert!(restore_staged(&target, &preview.id).await.is_err());
        // Restoring the same backup again adds nothing new.
        let again = inspect_bytes(
            &target,
            &Admitted::capture(&target).unwrap(),
            bytes,
            PASSWORD,
        )
        .await
        .unwrap();
        assert_eq!(
            (again.memories, again.notes, again.skipped_memories),
            (0, 0, 1)
        );
    }

    #[tokio::test]
    async fn every_failed_step_and_an_interrupted_restore_leave_existing_data_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let state = crate::test_support::state(dir.path());
        seed(
            &state,
            "I live in Leeds",
            "create a note called Ideas saying owls",
        )
        .await;
        let a = uuid::Uuid::new_v4().to_string();
        let mut p = payload(vec![record(&a, "Restored fact", &[], false)]);
        p.notes.version = 1;
        p.notes.notes.push(notes::Note {
            id: uuid::Uuid::new_v4().to_string(),
            title: "Trip".into(),
            kind: notes::Kind::List,
            body: String::new(),
            items: vec![],
            created: 0,
            updated: 0,
        });
        p.preferences = Some(Preferences {
            hue: 7,
            quiet_start: 0,
            quiet_end: 60,
            daily_limit: 1,
            gap_minutes: 15,
            followup_seconds: 8,
        });
        let original = contents(&state).await;
        let original_hue = state.snapshot.lock().await.settings.hue;
        notdot_core::settings::Settings::default()
            .save(&state.path)
            .unwrap();
        let original_file = std::fs::read(&state.path).unwrap();
        for step in [Step::Memories, Step::Notes, Step::Settings] {
            let context = state.context();
            let existing = context.memory.current_contents().await.unwrap();
            let current = context.notes.snapshot().await.unwrap();
            let json = serde_json::to_vec(&p).unwrap();
            let plan = plan(
                serde_json::from_slice(&json).unwrap(),
                &existing,
                &current,
                true,
            )
            .unwrap();
            let expected = Admitted::capture(&state)
                .unwrap()
                .revisions(&state)
                .await
                .unwrap();
            let mut snapshot = state.snapshot.lock().await;
            let error = apply(
                &context,
                &plan,
                &expected,
                Some((state.path.as_path(), &mut snapshot.settings)),
                Some(step),
            )
            .await
            .unwrap_err();
            drop(snapshot);
            assert!(
                error.contains("existing data was kept"),
                "{step:?}: {error}"
            );
            assert_eq!(contents(&state).await, original, "{step:?}");
            assert_eq!(state.snapshot.lock().await.settings.hue, original_hue);
            assert_eq!(
                std::fs::read(&state.path).unwrap(),
                original_file,
                "{step:?}"
            );
            assert!(!context.state_dir.join(JOURNAL).exists());
        }

        // Simulate a crash after the journal and some writes, then recovery.
        let context = state.context();
        let existing = context.memory.current_contents().await.unwrap();
        let current = context.notes.snapshot().await.unwrap();
        let plan = plan(p, &existing, &current, true).unwrap();
        let journal = Journal {
            version: 1,
            inserted: plan.memories.iter().map(|r| r.id.clone()).collect(),
            previous_notes: Some(current.clone()),
            previous_settings: Some(notdot_core::settings::Settings::default()),
        };
        crate::file_dialog::write_atomic(
            &context.state_dir.join(JOURNAL),
            &serde_json::to_vec(&journal).unwrap(),
        )
        .unwrap();
        let revision = context.memory.revision();
        context
            .memory
            .import_records(&plan.memories, revision)
            .await
            .unwrap();
        crate::notes::save_file(context.notes.path(), &plan.notebook).unwrap();
        let changed = notdot_core::settings::Settings {
            hue: 7,
            ..Default::default()
        };
        changed.save(&state.path).unwrap();
        // While the journal exists, another restore is refused.
        let expected = Admitted::capture(&state)
            .unwrap()
            .revisions(&state)
            .await
            .unwrap();
        let mut snapshot = state.snapshot.lock().await;
        assert!(
            apply(
                &context,
                &plan,
                &expected,
                Some((state.path.as_path(), &mut snapshot.settings)),
                None
            )
            .await
            .unwrap_err()
            .contains("still being undone")
        );
        drop(snapshot);
        // Startup order: settings first, then the person's stores.
        recover_settings(&context.state_dir, &state.path).unwrap();
        assert!(recover_profile(&context, Some(&state.path)).await.unwrap());
        assert_eq!(contents(&state).await, original);
        assert_eq!(std::fs::read(&state.path).unwrap(), original_file);
        assert!(
            !recover_profile(&context, Some(&state.path)).await.unwrap(),
            "idempotent"
        );
    }

    #[tokio::test]
    async fn while_a_restore_waits_to_be_undone_note_and_memory_changes_are_refused() {
        use notdot_core::memory::RESTORE_PENDING;
        let dir = tempfile::tempdir().unwrap();
        let original;
        let settings_path;
        {
            let state = crate::test_support::state(dir.path());
            seed(
                &state,
                "I live in Leeds",
                "create a list called Groceries with milk",
            )
            .await;
            original = contents(&state).await;
            settings_path = state.path.clone();
            // A restore that stopped after its journal and some writes, and
            // whose undo has not finished (a crash, or a failed undo).
            let context = state.context();
            let a = uuid::Uuid::new_v4().to_string();
            let mut p = payload(vec![record(&a, "Restored fact", &[], false)]);
            p.notes.version = 1;
            p.notes.notes.push(notes::Note {
                id: uuid::Uuid::new_v4().to_string(),
                title: "Trip".into(),
                kind: notes::Kind::List,
                body: String::new(),
                items: vec![],
                created: 0,
                updated: 0,
            });
            let existing = context.memory.current_contents().await.unwrap();
            let current = context.notes.snapshot().await.unwrap();
            let plan = plan(p, &existing, &current, true).unwrap();
            let previous = notdot_core::settings::Settings {
                hue: 3,
                ..Default::default()
            };
            let journal = Journal {
                version: 1,
                inserted: plan.memories.iter().map(|r| r.id.clone()).collect(),
                previous_notes: Some(current),
                previous_settings: Some(previous),
            };
            crate::file_dialog::write_atomic(
                &context.state_dir.join(JOURNAL),
                &serde_json::to_vec(&journal).unwrap(),
            )
            .unwrap();
            let revision = context.memory.revision();
            context
                .memory
                .import_records(&plan.memories, revision)
                .await
                .unwrap();
            crate::notes::save_file(context.notes.path(), &plan.notebook).unwrap();
            // The restore set the colour; another setting changed later.
            notdot_core::settings::Settings {
                hue: 7,
                microphone_muted: true,
                ..Default::default()
            }
            .save(&settings_path)
            .unwrap();
        }
        // The next start opens this person with the journal still waiting.
        let state = crate::test_support::state(dir.path());
        let context = state.context();
        let waiting = contents(&state).await;
        assert_ne!(waiting, original);
        // Spoken note changes, deletion and every memory write are refused,
        // so the undo below cannot discard anything the person did since.
        for request in [
            "add eggs to the groceries list",
            "create a list called Later",
            "tick off milk on the groceries list",
        ] {
            assert_eq!(
                context.notes.command(request).await.as_deref(),
                Some(RESTORE_PENDING),
                "{request}"
            );
        }
        let groceries = context.notes.snapshot().await.unwrap().notes[0].id.clone();
        assert_eq!(
            context.notes.delete(&groceries).await.unwrap_err(),
            RESTORE_PENDING
        );
        // Reading still works.
        assert!(
            context
                .notes
                .command("read my groceries list")
                .await
                .unwrap()
                .contains("milk")
        );
        let memory = &context.memory;
        assert_eq!(
            memory.remember("I like owls", "t").await.unwrap_err(),
            RESTORE_PENDING
        );
        let id = memory.review("").await.unwrap()[0].id.clone();
        assert_eq!(memory.forget(&id).await.unwrap_err(), RESTORE_PENDING);
        assert_eq!(
            memory.edit(&id, "I live in York").await.unwrap_err(),
            RESTORE_PENDING
        );
        assert_eq!(memory.maintain().await.unwrap_err(), RESTORE_PENDING);
        memory.set_learning_enabled(true).await;
        let ticket = memory.learning_ticket().unwrap();
        assert_eq!(
            memory
                .remember_explicit_submission(ticket, "t", "Remember that I like owls")
                .await
                .unwrap_err(),
            RESTORE_PENDING
        );
        assert_eq!(contents(&state).await, waiting);
        // Finishing the undo removes only the restore's writes, keeps the
        // later setting and lets changes through again.
        recover_settings(&context.state_dir, &settings_path).unwrap();
        assert!(
            recover_profile(&context, Some(&settings_path))
                .await
                .unwrap()
        );
        assert_eq!(contents(&state).await, original);
        let saved = notdot_core::settings::Settings::load(&settings_path).unwrap();
        assert_eq!((saved.hue, saved.microphone_muted), (3, true));
        assert!(
            context
                .notes
                .command("add eggs to the groceries list")
                .await
                .unwrap()
                .starts_with("Added eggs")
        );
        memory.remember("I like owls", "t").await.unwrap();
    }
    #[tokio::test]
    async fn backups_belong_to_one_person_and_staged_restores_cannot_cross_people() {
        use notdot_core::profiles::{Actor, SpeakerOutcome};
        let dir = tempfile::tempdir().unwrap();
        let state = crate::test_support::state(dir.path());
        seed(
            &state,
            "Owner private fact",
            "create a note called Owner plans saying secret",
        )
        .await;
        let mut roster = state.speakers.roster().unwrap();
        let owner = roster.owner().clone();
        let member = roster.add_member(&Actor::LocalDevice, "Member").unwrap();
        for id in [owner.clone(), member.clone()] {
            roster
                .set_voice_enrolled(&Actor::LocalDevice, &id, true)
                .unwrap();
        }
        state.speakers.save_roster(roster).unwrap();
        let recognised = |profile| SpeakerOutcome::Recognised {
            profile,
            score: 0.99,
        };
        crate::test_support::admit(&state, recognised(member.clone()))
            .await
            .unwrap();
        seed(
            &state,
            "Member fact",
            "create a list called Chores with dishes",
        )
        .await;
        let (bytes, memories, notes) =
            export_bytes(&state, &Admitted::capture(&state).unwrap(), PASSWORD)
                .await
                .unwrap();
        assert_eq!((memories, notes), (1, 1));
        let plain = open(&bytes, PASSWORD).unwrap();
        let payload: Payload = serde_json::from_slice(&plain).unwrap();
        assert!(
            payload.preferences.is_none(),
            "members never export device settings"
        );
        assert!(!String::from_utf8_lossy(&plain).contains("Owner"));
        // A restore staged by the member cannot be applied to the owner.
        crate::test_support::admit(&state, recognised(owner.clone()))
            .await
            .unwrap();
        let preview = inspect_bytes(
            &state,
            &Admitted::capture(&state).unwrap(),
            bytes.clone(),
            PASSWORD,
        )
        .await
        .unwrap();
        crate::test_support::admit(&state, recognised(member.clone()))
            .await
            .unwrap();
        assert!(restore_staged(&state, &preview.id).await.is_err());
        crate::test_support::admit(&state, recognised(owner))
            .await
            .unwrap();
        assert!(
            restore_staged(&state, &preview.id).await.is_err(),
            "single use even after a refusal"
        );
        assert_eq!(
            contents(&state).await.0,
            vec!["Owner private fact".to_owned()]
        );
    }

    #[tokio::test]
    async fn a_preview_is_refused_after_any_change_and_requires_a_fresh_one() {
        use notdot_core::profiles::{Actor, SpeakerOutcome};
        let source_dir = tempfile::tempdir().unwrap();
        let source = crate::test_support::state(source_dir.path());
        seed(
            &source,
            "Backed up fact",
            "create a list called Trip with tent",
        )
        .await;
        let (bytes, ..) = export_bytes(&source, &Admitted::capture(&source).unwrap(), PASSWORD)
            .await
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let state = crate::test_support::state(dir.path());
        seed(
            &state,
            "Local fact",
            "create a list called Groceries with milk",
        )
        .await;
        let preview = || async {
            inspect_bytes(
                &state,
                &Admitted::capture(&state).unwrap(),
                bytes.clone(),
                PASSWORD,
            )
            .await
            .unwrap()
            .id
        };
        let stale = "changed after the preview";
        // A note added after the preview would otherwise be overwritten.
        let id = preview().await;
        state
            .context()
            .notes
            .command("add eggs to the groceries list")
            .await
            .unwrap();
        assert!(
            restore_staged(&state, &id)
                .await
                .unwrap_err()
                .contains(stale)
        );
        // An edited, then a forgotten memory.
        let local = state
            .context()
            .memory
            .review("Local")
            .await
            .unwrap()
            .remove(0)
            .id;
        let id = preview().await;
        state
            .context()
            .memory
            .edit(&local, "Local fact, edited")
            .await
            .unwrap();
        assert!(
            restore_staged(&state, &id)
                .await
                .unwrap_err()
                .contains(stale)
        );
        let local = state
            .context()
            .memory
            .review("edited")
            .await
            .unwrap()
            .remove(0)
            .id;
        let id = preview().await;
        state.context().memory.forget(&local).await.unwrap();
        assert!(
            restore_staged(&state, &id)
                .await
                .unwrap_err()
                .contains(stale)
        );
        // A settings change.
        let id = preview().await;
        state.snapshot.lock().await.settings.hue = 300;
        assert!(
            restore_staged(&state, &id)
                .await
                .unwrap_err()
                .contains(stale)
        );
        // Switching to someone else and back drops the waiting restore.
        let mut roster = state.speakers.roster().unwrap();
        let owner = roster.owner().clone();
        let member = roster.add_member(&Actor::LocalDevice, "Member").unwrap();
        for id in [owner.clone(), member.clone()] {
            roster
                .set_voice_enrolled(&Actor::LocalDevice, &id, true)
                .unwrap();
        }
        state.speakers.save_roster(roster).unwrap();
        let recognised = |profile| SpeakerOutcome::Recognised {
            profile,
            score: 0.99,
        };
        crate::test_support::admit(&state, recognised(owner.clone()))
            .await
            .unwrap();
        let id = preview().await;
        crate::test_support::admit(&state, recognised(member))
            .await
            .unwrap();
        crate::test_support::admit(&state, recognised(owner))
            .await
            .unwrap();
        assert!(restore_staged(&state, &id).await.is_err());
        // Nothing from the backup arrived through any refused attempt.
        assert!(
            !contents(&state)
                .await
                .0
                .contains(&"Backed up fact".to_owned())
        );
        assert!(!contents(&state).await.1.contains(&"Trip".to_owned()));
        // A fresh preview of the unchanged state succeeds.
        let id = preview().await;
        restore_staged(&state, &id).await.unwrap();
        let groceries = state.context().notes.snapshot().await.unwrap();
        assert_eq!(
            groceries
                .notes
                .iter()
                .find(|n| n.title == "Groceries")
                .unwrap()
                .items
                .len(),
            2
        );
        assert!(contents(&state).await.1.contains(&"Trip".to_owned()));
    }

    #[tokio::test]
    async fn export_and_inspect_stay_with_the_person_who_started_them() {
        use notdot_core::profiles::{Actor, SpeakerOutcome};
        let dir = tempfile::tempdir().unwrap();
        let state = crate::test_support::state(dir.path());
        seed(
            &state,
            "Owner private fact",
            "create a note called Owner plans saying secret",
        )
        .await;
        let mut roster = state.speakers.roster().unwrap();
        let owner = roster.owner().clone();
        let member = roster.add_member(&Actor::LocalDevice, "Member").unwrap();
        for id in [owner.clone(), member.clone()] {
            roster
                .set_voice_enrolled(&Actor::LocalDevice, &id, true)
                .unwrap();
        }
        state.speakers.save_roster(roster).unwrap();
        let recognised = |profile| SpeakerOutcome::Recognised {
            profile,
            score: 0.99,
        };
        crate::test_support::admit(&state, recognised(member.clone()))
            .await
            .unwrap();
        seed(
            &state,
            "Member fact",
            "create a list called Chores with dishes",
        )
        .await;
        crate::test_support::admit(&state, recognised(owner.clone()))
            .await
            .unwrap();
        let owner_backup = export_bytes(&state, &Admitted::capture(&state).unwrap(), PASSWORD)
            .await
            .unwrap()
            .0;

        // The member starts an export; the owner speaks while key derivation
        // runs. The export is refused rather than switching person.
        crate::test_support::admit(&state, recognised(member.clone()))
            .await
            .unwrap();
        let who = Admitted::capture(&state).unwrap();
        let switch = {
            let state = state.clone();
            let owner = owner.clone();
            tokio::spawn(async move {
                crate::test_support::admit(&state, recognised(owner))
                    .await
                    .unwrap()
            })
        };
        let exported = export_bytes(&state, &who, PASSWORD).await;
        switch.await.unwrap();
        match exported {
            Ok((bytes, ..)) => {
                // Finished before the switch: it must be the member's data.
                let plain = open(&bytes, PASSWORD).unwrap();
                let text = String::from_utf8_lossy(&plain).into_owned();
                assert!(text.contains("Member fact") && !text.contains("Owner private"));
                assert!(
                    who.check(&state).is_err(),
                    "the command refuses to write it now"
                );
            }
            Err(e) => assert!(e.contains("active person changed"), "{e}"),
        }

        // An inspect started by the member, finishing after the owner speaks,
        // never stages a restore for the owner.
        crate::test_support::admit(&state, recognised(member.clone()))
            .await
            .unwrap();
        let who = Admitted::capture(&state).unwrap();
        crate::test_support::admit(&state, recognised(owner.clone()))
            .await
            .unwrap();
        let refused = inspect_bytes(&state, &who, owner_backup.clone(), PASSWORD).await;
        assert!(refused.unwrap_err().contains("active person changed"));
        assert!(state.backups.0.lock().unwrap().is_none());
        // And the member's own data was not touched.
        crate::test_support::admit(&state, recognised(member))
            .await
            .unwrap();
        assert_eq!(contents(&state).await.0, vec!["Member fact".to_owned()]);
    }
}
