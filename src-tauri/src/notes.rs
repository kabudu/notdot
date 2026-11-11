//! One private notebook per person, stored beside that person's other private
//! state. Every command uses the admitted profile context; there is no
//! profile argument a webview or model could change.
use crate::AppState;
use notdot_core::notes::{self, Notebook, Outcome, Session};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::State;
use tokio::sync::Mutex;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub struct Notes {
    path: PathBuf,
    state: Mutex<Inner>,
}
struct Inner {
    /// An unreadable file is reported and left untouched, never overwritten.
    book: Result<Notebook, String>,
    session: Session,
}

fn load(path: &Path) -> Result<Notebook, String> {
    // Temporary files only remain if a save was interrupted before its
    // rename; the previous complete notebook is still at `path`.
    crate::file_dialog::remove_stale_temps(path);
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Notebook::default()),
        Ok(m) if !m.file_type().is_file() => {
            return Err("Your notes file is not a regular file, so notes are unavailable.".into());
        }
        _ => {}
    }
    let bytes = crate::file_dialog::read_bounded(path, notes::MAX_FILE_BYTES).map_err(|e| {
        if e.kind() == std::io::ErrorKind::FileTooLarge {
            "Your notes file is too large to open. It was left unchanged.".to_string()
        } else {
            format!("Your notes could not be read: {e}")
        }
    })?;
    let book: Notebook = serde_json::from_slice(&bytes)
        .map_err(|_| "Your notes file could not be read. It was left unchanged.".to_string())?;
    book.validate()
        .map_err(|e| format!("{e}. Your notes file was left unchanged."))?;
    Ok(book)
}

/// Durably replace the notebook file. `Unconfirmed` means the new notebook is
/// already in place and must be treated as current.
pub fn save_file(path: &Path, book: &Notebook) -> Result<(), crate::file_dialog::WriteError> {
    use crate::file_dialog::WriteError;
    book.validate().map_err(WriteError::NotWritten)?;
    let bytes = serde_json::to_vec(book).map_err(|e| WriteError::NotWritten(e.to_string()))?;
    if bytes.len() as u64 > notes::MAX_FILE_BYTES {
        return Err(WriteError::NotWritten(
            "your notes are too large; delete a note first".into(),
        ));
    }
    crate::file_dialog::write_atomic(path, &bytes)
}
async fn save_async(path: &Path, book: &Notebook) -> Result<(), crate::file_dialog::WriteError> {
    let path = path.to_owned();
    let book = book.clone();
    tokio::task::spawn_blocking(move || save_file(&path, &book))
        .await
        .unwrap_or_else(|e| Err(crate::file_dialog::WriteError::NotWritten(e.to_string())))
}
/// Apply a save result: the in-memory notebook follows whatever is on disk.
fn settle(
    slot: &mut Result<Notebook, String>,
    next: Notebook,
    result: Result<(), crate::file_dialog::WriteError>,
) -> Result<(), String> {
    use crate::file_dialog::WriteError;
    match result {
        Ok(()) => {
            *slot = Ok(next);
            Ok(())
        }
        Err(WriteError::Unconfirmed(e)) => {
            *slot = Ok(next);
            Err(format!(
                "The change was saved, but the system could not confirm it reached the disk: {e}."
            ))
        }
        Err(WriteError::NotWritten(e)) => Err(format!(
            "Your notes could not be saved: {e}. Nothing was changed."
        )),
    }
}

impl Notes {
    pub fn open(path: PathBuf) -> Self {
        let book = load(&path);
        Self {
            path,
            state: Mutex::new(Inner {
                book,
                session: Session::default(),
            }),
        }
    }
    pub async fn cancel(&self) {
        self.state.lock().await.session.cancel();
    }
    pub async fn waiting(&self) -> bool {
        self.state.lock().await.session.waiting(now())
    }
    /// A spoken request, or `None` for ordinary conversation.
    pub async fn command(&self, text: &str) -> Option<String> {
        let mut inner = self.state.lock().await;
        let Inner { book, session } = &mut *inner;
        let current = match book {
            Ok(book) => book,
            Err(error) => {
                // Only claim the request if it is plainly about notes.
                return notes::parse(text).map(|_| error.clone());
            }
        };
        match notes::handle(current, session, text, now()) {
            Outcome::NotMine => None,
            Outcome::Reply(reply) => Some(reply),
            Outcome::Changed(next, reply) => {
                let result = save_async(&self.path, &next).await;
                Some(match settle(book, next, result) {
                    Ok(()) => reply,
                    Err(error) => {
                        session.cancel();
                        error
                    }
                })
            }
        }
    }
    pub async fn snapshot(&self) -> Result<Notebook, String> {
        self.state.lock().await.book.clone()
    }
    /// Replace the notebook as part of a journalled restore. The caller holds
    /// the person's turn lock and has validated the notebook.
    #[allow(dead_code)] // Used by encrypted restore in the next change.
    pub async fn replace(&self, next: Notebook) -> Result<(), String> {
        let mut inner = self.state.lock().await;
        if let Err(error) = &inner.book {
            return Err(error.clone());
        }
        let result = save_async(&self.path, &next).await;
        inner.session.cancel();
        settle(&mut inner.book, next, result)
    }
    #[allow(dead_code)] // Used by encrypted restore in the next change.
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// Reload after a crash-recovery rollback rewrote the file.
    #[allow(dead_code)] // Used by encrypted restore in the next change.
    pub async fn reload(&self) {
        let mut inner = self.state.lock().await;
        inner.book = load(&self.path);
        inner.session.cancel();
    }
    async fn delete(&self, id: &str) -> Result<(), String> {
        let mut inner = self.state.lock().await;
        let book = inner.book.as_ref().map_err(Clone::clone)?;
        let mut next = book.clone();
        let before = next.notes.len();
        next.notes.retain(|n| n.id != id);
        if next.notes.len() == before {
            return Err("That note no longer exists".into());
        }
        next.version = 1;
        next.revision = next
            .revision
            .checked_add(1)
            .filter(|r| *r < notes::MAX_REVISION)
            .ok_or("Your notes can't take more changes")?;
        let result = save_async(&self.path, &next).await;
        inner.session.cancel();
        settle(&mut inner.book, next, result)
    }
}

/// Who may use notes by voice. Notes belong to a person, so once any voice is
/// set up (or a restricted mode is chosen), an unrecognised voice served as
/// the owner in Anyone mode cannot reach the owner's notes. On a computer
/// where nobody has taught notdot a voice, there is no voice identity at all:
/// the single person using the unlocked computer is the owner, exactly as for
/// memories, and notes work without any setup.
pub fn voice_allowed(identity: &notdot_core::profiles::TurnLock, identification: bool) -> bool {
    !identity.open_fallback || !identification
}
pub const UNRECOGNISED: &str = "Your notes are private to your voice, and I didn't recognise it this time. Please try again a little closer to the microphone. To use notes as someone new, ask the owner to add you in Settings, under People and voices.";

/// The spoken notes path used by `send_message`, bound to the admitted person.
pub async fn voice_command(
    state: &AppState,
    identity: &notdot_core::profiles::TurnLock,
    context: &crate::profile_context::ProfileContext,
    text: &str,
) -> Option<String> {
    if voice_allowed(identity, state.speakers.needs_identification()) {
        if context.id != identity.profile {
            return notes::parse(text).map(|_| "The speaker changed. Please try again.".to_owned());
        }
        context.notes.command(text).await
    } else {
        notes::parse(text).map(|_| UNRECOGNISED.to_owned())
    }
}
/// Plain text for export from the active person's notebook only.
pub async fn export_body(state: &AppState, id: Option<&str>) -> Result<(String, usize), String> {
    let book = state.context().notes.snapshot().await?;
    let chosen: Vec<_> = match id {
        Some(id) => book.notes.iter().filter(|n| n.id == id).collect(),
        None => book.notes.iter().collect(),
    };
    if chosen.is_empty() {
        return Err("There are no notes to export".into());
    }
    Ok((notes::export_text(&chosen), chosen.len()))
}

#[tauri::command]
pub async fn notes_list(state: State<'_, Arc<AppState>>) -> Result<Notebook, String> {
    state.context().notes.snapshot().await
}
#[tauri::command]
pub async fn notes_delete(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    if uuid::Uuid::parse_str(&id).is_err() {
        return Err("Invalid note".into());
    }
    let context = state.context();
    let _turn = context.turn_lock.lock().await;
    context.notes.delete(&id).await
}
#[tauri::command]
pub async fn notes_export(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<AppState>>,
    id: Option<String>,
) -> Result<String, String> {
    let context = state.context();
    let (text, count) = export_body(&state, id.as_deref()).await?;
    let Some(path) = crate::file_dialog::choose_save(
        &window,
        "Export notes",
        "notdot notes.txt",
        ("Plain text", &["txt"]),
    )
    .await
    else {
        return Ok("Export cancelled. Nothing was saved.".into());
    };
    // The profile may have changed while the panel was open.
    if state.context().id != context.id {
        return Err("The active person changed. Nothing was saved.".into());
    }
    tokio::task::spawn_blocking(move || crate::file_dialog::write_atomic(&path, text.as_bytes()))
        .await
        .map_err(|e| e.to_string())??;
    Ok(format!(
        "Exported {count} {}.",
        if count == 1 { "note" } else { "notes" }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn notebooks_survive_restart_and_failed_saves_change_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.json");
        let notes = Notes::open(path.clone());
        assert!(notes.command("what's the weather").await.is_none());
        notes
            .command("create a list called Groceries with milk, eggs")
            .await
            .unwrap();
        notes.command("add oat milk to that list").await.unwrap();
        drop(notes);
        // Restart: the notebook is durable and the follow-up focus is not.
        let notes = Notes::open(path.clone());
        assert_eq!(notes.snapshot().await.unwrap().notes[0].items.len(), 3);
        assert!(
            notes
                .command("add tea to that list")
                .await
                .unwrap()
                .starts_with("Which list")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            // A save that cannot complete leaves the notebook as it was.
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
            let reply = notes
                .command("add tea to the groceries list")
                .await
                .unwrap();
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
            assert!(reply.contains("could not be saved"), "{reply}");
            assert_eq!(notes.snapshot().await.unwrap().notes[0].items.len(), 3);
            assert_eq!(load(&path).unwrap().notes[0].items.len(), 3);
        }
    }

    #[tokio::test]
    async fn corrupt_or_interrupted_files_are_reported_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.json");
        std::fs::write(&path, b"{not json").unwrap();
        let notes = Notes::open(path.clone());
        let reply = notes.command("create a list called X").await.unwrap();
        assert!(reply.contains("left unchanged"), "{reply}");
        assert!(notes.command("hello there").await.is_none());
        assert_eq!(std::fs::read(&path).unwrap(), b"{not json");
        // An interrupted save leaves only a temporary file, which is discarded.
        std::fs::remove_file(&path).unwrap();
        let partial = dir.path().join(".notes.json.0f.partial");
        std::fs::write(&partial, b"partial").unwrap();
        let notes = Notes::open(path.clone());
        assert!(notes.snapshot().await.unwrap().notes.is_empty());
        assert!(!partial.exists());
        // Oversized files are refused before parsing.
        std::fs::write(&path, vec![b' '; notes::MAX_FILE_BYTES as usize + 1]).unwrap();
        assert!(Notes::open(path).snapshot().await.is_err());
    }

    #[tokio::test]
    async fn each_person_has_a_separate_notebook() {
        let dir = tempfile::tempdir().unwrap();
        let owner = Notes::open(dir.path().join("owner-notes.json"));
        let member = Notes::open(dir.path().join("member-notes.json"));
        owner
            .command("create a note called Bank saying private owner text")
            .await
            .unwrap();
        // Same words from another person never reach the owner's note.
        let reply = member.command("read my bank note").await.unwrap();
        assert!(reply.contains("couldn't find"), "{reply}");
        assert!(!reply.contains("private owner text"));
        assert!(
            member
                .command("what notes do I have")
                .await
                .unwrap()
                .contains("no notes")
        );
        assert!(member.snapshot().await.unwrap().notes.is_empty());
    }

    #[test]
    fn unrecognised_voices_in_anyone_mode_cannot_use_notes() {
        let mut lock = notdot_core::profiles::TurnLock {
            capture_token: "test-capture".into(),
            profile: notdot_core::profiles::ProfileId::random(),
            role: notdot_core::profiles::Role::Owner,
            epoch: 0,
            open_fallback: false,
        };
        assert!(voice_allowed(&lock, true));
        lock.open_fallback = true;
        // Default single-person setup: no voices, so nothing to recognise.
        assert!(voice_allowed(&lock, false));
        // With voices set up, an unrecognised speaker is refused.
        assert!(!voice_allowed(&lock, true));
    }

    #[tokio::test]
    async fn native_commands_follow_real_admission_and_never_cross_people() {
        use notdot_core::profiles::{Actor, SpeakerOutcome};
        use tauri::Manager;
        let dir = tempfile::tempdir().unwrap();
        let state = crate::test_support::state(dir.path());
        // Default single-person setup: no voices, Anyone mode. The owner can
        // use notes without enrolling a voice.
        assert!(!state.speakers.needs_identification());
        let owner = crate::test_support::admit(&state, SpeakerOutcome::Disabled)
            .await
            .unwrap();
        assert!(owner.open_fallback);
        let context = state.context();
        let reply = voice_command(
            &state,
            &owner,
            &context,
            "create a note called Bank saying owner secret",
        )
        .await
        .unwrap();
        assert!(reply.starts_with("Saved a note"), "{reply}");
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        app.manage(state.clone());
        let owner_book = notes_list(app.state()).await.unwrap();
        assert_eq!(owner_book.notes.len(), 1);
        let owner_note = owner_book.notes[0].id.clone();

        // Add a trusted member and enrol voices: identification is now in use.
        let mut roster = state.speakers.roster().unwrap();
        let member = roster.add_member(&Actor::LocalDevice, "Member").unwrap();
        for id in [roster.owner().clone(), member.clone()] {
            roster
                .set_voice_enrolled(&Actor::LocalDevice, &id, true)
                .unwrap();
        }
        state.speakers.save_roster(roster).unwrap();
        assert!(state.speakers.needs_identification());

        // An unrecognised voice still gets owner context in Anyone mode, but
        // never the owner's notes.
        let stranger = crate::test_support::admit(&state, SpeakerOutcome::Unknown)
            .await
            .unwrap();
        assert!(stranger.open_fallback);
        let context = state.context();
        let reply = voice_command(&state, &stranger, &context, "read my bank note")
            .await
            .unwrap();
        assert_eq!(reply, UNRECOGNISED);
        assert!(
            voice_command(&state, &stranger, &context, "what's the time")
                .await
                .is_none()
        );

        // The member is admitted into their own context.
        let member_lock = crate::test_support::admit(
            &state,
            SpeakerOutcome::Recognised {
                profile: member.clone(),
                score: 0.99,
            },
        )
        .await
        .unwrap();
        let context = state.context();
        assert_eq!(context.id, member);
        let reply = voice_command(&state, &member_lock, &context, "read my bank note")
            .await
            .unwrap();
        assert!(!reply.contains("owner secret"), "{reply}");
        assert!(notes_list(app.state()).await.unwrap().notes.is_empty());
        // Native delete and export are scoped to the active person, whatever
        // ID the webview sends.
        assert!(notes_delete(app.state(), owner_note.clone()).await.is_err());
        assert!(export_body(&state, Some(&owner_note)).await.is_err());
        assert!(export_body(&state, None).await.is_err());
        voice_command(
            &state,
            &member_lock,
            &context,
            "create a list called Chores with dishes",
        )
        .await
        .unwrap();
        let (text, count) = export_body(&state, None).await.unwrap();
        assert_eq!((count, text.contains("owner secret")), (1, false));
        // A stale lock from another person cannot use this context.
        assert_eq!(
            voice_command(&state, &owner, &context, "read my chores list")
                .await
                .unwrap(),
            UNRECOGNISED
        );
        let owner_id = state.speakers.roster().unwrap().owner().clone();
        let notdot_core::profiles::Admission::Admit(recognised_owner) =
            state.speakers.roster().unwrap().admit(
                "other",
                &SpeakerOutcome::Recognised {
                    profile: owner_id,
                    score: 0.99,
                },
            )
        else {
            panic!("owner admission")
        };
        let reply = voice_command(&state, &recognised_owner, &context, "read my chores list")
            .await
            .unwrap();
        assert!(reply.contains("speaker changed"), "{reply}");

        // Back to the owner: their note is intact and deletable natively.
        crate::test_support::admit(
            &state,
            SpeakerOutcome::Recognised {
                profile: state.speakers.roster().unwrap().owner().clone(),
                score: 0.99,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            notes_list(app.state()).await.unwrap().notes[0].id,
            owner_note
        );
        notes_delete(app.state(), owner_note).await.unwrap();
        assert!(notes_list(app.state()).await.unwrap().notes.is_empty());
    }
}
