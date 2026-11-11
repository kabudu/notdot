//! Native save and open panels. The person chooses every location; notdot
//! never writes exports or backups to a path supplied by the webview.
use std::path::{Path, PathBuf};

pub async fn choose_save(
    window: &tauri::WebviewWindow,
    title: &str,
    file_name: &str,
    filter: (&str, &[&str]),
) -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .set_title(title)
        .set_file_name(file_name)
        .add_filter(filter.0, filter.1)
        .set_parent(window)
        .save_file()
        .await
        .map(|handle| handle.path().to_owned())
}
#[allow(dead_code)] // Used by encrypted restore in the next change.
pub async fn choose_open(
    window: &tauri::WebviewWindow,
    title: &str,
    filter: (&str, &[&str]),
) -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .set_title(title)
        .add_filter(filter.0, filter.1)
        .set_parent(window)
        .pick_file()
        .await
        .map(|handle| handle.path().to_owned())
}

/// Why an atomic write did not fully succeed.
#[derive(Debug, PartialEq, Eq)]
pub enum WriteError {
    /// Nothing at the destination changed.
    NotWritten(String),
    /// The new file is in place, but flushing its folder failed, so the
    /// change may not survive a power loss.
    Unconfirmed(String),
}
impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteError::NotWritten(e) => {
                write!(f, "Could not save the file: {e}. Nothing was changed.")
            }
            WriteError::Unconfirmed(e) => write!(
                f,
                "Saved, but the system could not confirm it reached the disk: {e}."
            ),
        }
    }
}
impl From<WriteError> for String {
    fn from(e: WriteError) -> Self {
        e.to_string()
    }
}
/// Prefix shared by every temporary file this writer creates beside `path`.
pub fn temp_prefix(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|name| format!(".{name}."))
}
/// Write a new, exclusively created temporary file with an unpredictable
/// name beside the destination, flush it, then rename it into place. The
/// temporary file is never opened through an existing path or symlink.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), WriteError> {
    use std::io::Write;
    let not_written = |e: &dyn std::fmt::Display| WriteError::NotWritten(e.to_string());
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| not_written(&"choose a folder to save into"))?;
    let prefix = temp_prefix(path).ok_or_else(|| not_written(&"choose a valid file name"))?;
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink() || m.is_dir()) {
        return Err(not_written(&"the destination is a link or folder"));
    }
    let temp = parent.join(format!("{prefix}{}.partial", uuid::Uuid::new_v4().simple()));
    let staged = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)
    })();
    if let Err(e) = staged {
        let _ = std::fs::remove_file(&temp);
        return Err(not_written(&e));
    }
    #[cfg(unix)]
    std::fs::File::open(parent)
        .and_then(|d| d.sync_all())
        .map_err(|e| WriteError::Unconfirmed(e.to_string()))?;
    Ok(())
}
/// Remove temporary files left by an interrupted [`write_atomic`]. Only
/// regular files and links with the exact writer prefix are removed; a link
/// is removed itself and never followed.
pub fn remove_stale_temps(path: &Path) {
    let (Some(parent), Some(prefix)) = (path.parent(), temp_prefix(path)) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten().take(10_000) {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with(&prefix)
            && name.ends_with(".partial")
            && entry.file_type().is_ok_and(|t| !t.is_dir())
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Read at most `limit` bytes from a regular file. The size and type are
/// checked on the opened handle, and the read itself is capped, so a file
/// that changes or grows after the check cannot exceed the bound.
pub fn read_bounded(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
    use std::io::{Error, ErrorKind, Read};
    let file = std::fs::File::open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(Error::new(ErrorKind::InvalidInput, "not a regular file"));
    }
    if meta.len() > limit {
        return Err(Error::new(ErrorKind::FileTooLarge, "file is too large"));
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(Error::new(ErrorKind::FileTooLarge, "file is too large"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn atomic_writes_replace_whole_files_and_leave_no_partial_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.txt");
        std::fs::write(&path, b"old").unwrap();
        write_atomic(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        let missing = dir.path().join("missing").join("out.txt");
        assert!(matches!(
            write_atomic(&missing, b"x"),
            Err(WriteError::NotWritten(_))
        ));
        // A stale temporary file is cleaned up; a planted link is removed,
        // never followed.
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("victim"), b"keep").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            outside.path().join("victim"),
            dir.path().join(".out.txt.0123.partial"),
        )
        .unwrap();
        std::fs::write(dir.path().join(".out.txt.4567.partial"), b"x").unwrap();
        remove_stale_temps(&path);
        assert_eq!(
            std::fs::read(outside.path().join("victim")).unwrap(),
            b"keep"
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        // The destination itself must not be a link.
        #[cfg(unix)]
        {
            let link = dir.path().join("link.txt");
            std::os::unix::fs::symlink(outside.path().join("victim"), &link).unwrap();
            assert!(write_atomic(&link, b"x").is_err());
            assert_eq!(
                std::fs::read(outside.path().join("victim")).unwrap(),
                b"keep"
            );
        }
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
    }
    #[test]
    fn bounded_reads_refuse_folders_and_large_files() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_bounded(dir.path(), 10).is_err());
        let path = dir.path().join("big");
        std::fs::write(&path, vec![0; 11]).unwrap();
        assert!(read_bounded(&path, 10).is_err());
        assert_eq!(read_bounded(&path, 11).unwrap().len(), 11);
    }
}
