//! Durable desktop state for paired phones.
//!
//! Two small JSON files in one private directory, each replaced atomically
//! (write a temporary file, flush it to disk, rename it over the old one,
//! then flush the directory):
//!
//! - `phones.json`: paired phones and removed phones ([`RegistrySnapshot`]).
//! - `phone-keys.json`: attested step-up keys, owner grants, App Attest
//!   counters and the outcomes of side-effecting requests, including ones
//!   still running ([`crate::service::ServiceSnapshot`]).
//!
//! Neither file holds a private key: the desktop's identity key lives in the
//! OS credential store, and phones' keys never leave the phones. Each file
//! carries a format version and a generation that rises with every save. A
//! file from a newer notdot, or one that cannot be read, is never
//! overwritten: the desktop refuses to start phone links and says so, and
//! the owner can move the file aside to start again.
//!
//! Rolling a file back (say, from an old backup) cannot replay an action:
//! every action needs a fresh, single-use challenge that only ever lives in
//! memory. It can bring back a phone removed since, or a grant since
//! withdrawn, so restoring these files is not supported; notdot's backups
//! do not include them.
use serde::{Serialize, de::DeserializeOwned};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

/// Current file format.
pub const VERSION: u32 = 1;
/// Largest state file accepted.
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
pub const REGISTRY_FILE: &str = "phones.json";
pub const SERVICE_FILE: &str = "phone-keys.json";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("phone link state was saved by a newer notdot (format {0}); update notdot")]
    Newer(u32),
    #[error("phone link state can't be read ({0}); nothing was changed")]
    Unreadable(String),
    #[error("phone link state can't be saved ({0})")]
    Write(String),
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope<T> {
    version: u32,
    generation: u64,
    data: T,
}

/// One atomically replaced JSON file.
pub struct JsonFile {
    path: PathBuf,
    generation: Mutex<u64>,
}

impl JsonFile {
    /// Open `name` in `dir` and read it, if present.
    pub fn open<T: DeserializeOwned>(
        dir: &Path,
        name: &str,
    ) -> Result<(Self, Option<T>), StoreError> {
        let path = dir.join(name);
        // Leftovers from a save interrupted before its rename.
        let _ = std::fs::remove_file(temp_path(&path));
        let (generation, data) = match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (0, None),
            Err(error) => return Err(StoreError::Unreadable(error.kind().to_string())),
            Ok(meta)
                if meta.file_type().is_symlink()
                    || meta.len() > MAX_FILE_BYTES
                    || !meta.is_file() =>
            {
                return Err(StoreError::Unreadable("unexpected file".into()));
            }
            Ok(_) => {
                let bytes = std::fs::read(&path)
                    .map_err(|e| StoreError::Unreadable(e.kind().to_string()))?;
                let head: serde_json::Value = serde_json::from_slice(&bytes)
                    .map_err(|_| StoreError::Unreadable("not JSON".into()))?;
                let version = head["version"]
                    .as_u64()
                    .ok_or_else(|| StoreError::Unreadable("no version".into()))?;
                if version > u64::from(VERSION) {
                    return Err(StoreError::Newer(version as u32));
                }
                if version != u64::from(VERSION) {
                    return Err(StoreError::Unreadable(format!("format {version}")));
                }
                let envelope: Envelope<T> = serde_json::from_slice(&bytes)
                    .map_err(|e| StoreError::Unreadable(e.to_string()))?;
                (envelope.generation, Some(envelope.data))
            }
        };
        Ok((
            Self {
                path,
                generation: Mutex::new(generation),
            },
            data,
        ))
    }

    /// Replace the file with `data`.
    pub fn save<T: Serialize>(&self, data: &T) -> Result<(), StoreError> {
        let mut generation = self.generation.lock().unwrap_or_else(|e| e.into_inner());
        let next = generation
            .checked_add(1)
            .ok_or_else(|| StoreError::Write("generation exhausted".into()))?;
        let bytes = serde_json::to_vec(&Envelope {
            version: VERSION,
            generation: next,
            data,
        })
        .map_err(|e| StoreError::Write(e.to_string()))?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(StoreError::Write("state file too large".into()));
        }
        write_atomic(&self.path, &bytes).map_err(|e| StoreError::Write(e.kind().to_string()))?;
        *generation = next;
        Ok(())
    }

    pub fn generation(&self) -> u64 {
        *self.generation.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".saving");
    path.with_file_name(name)
}

/// Write `bytes` to `path` so a crash leaves either the old file or the new
/// one, never a mix. Owner-only permissions on Unix.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temp = temp_path(path);
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        // Never follow an existing .saving symlink left by another process.
        if std::fs::symlink_metadata(&temp).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(std::io::Error::other("temporary file is a symbolic link"));
        }
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&temp, path)?;
    #[cfg(unix)]
    if let Some(dir) = path.parent() {
        std::fs::File::open(dir)?.sync_all()?;
    }
    Ok(())
}

/// Create (if needed) a directory only the current user can read.
pub fn private_dir(dir: &Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(dir).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(std::io::Error::other(
            "phone state directory is a symbolic link",
        ));
    }
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Paired and removed phones.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistrySnapshot {
    pub devices: Vec<crate::pairing::DeviceRecord>,
    pub revoked: Vec<crate::DeviceId>,
    /// The UDP port the desktop listened on, reused after a restart so
    /// phones on the same network find it at the address they know.
    #[serde(default)]
    pub port: Option<u16>,
    /// Iroh may allocate a different IPv6 port on the first bind. Keep it
    /// too: phones can remember only the IPv6 address on some networks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipv6_port: Option<u16>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_are_atomic_versioned_and_never_overwrite_unknown_files() {
        let dir = tempfile_dir();
        let (file, data) = JsonFile::open::<Vec<u32>>(&dir, "x.json").unwrap();
        assert_eq!(data, None);
        file.save(&vec![1u32, 2]).unwrap();
        file.save(&vec![3u32]).unwrap();
        let (again, data) = JsonFile::open::<Vec<u32>>(&dir, "x.json").unwrap();
        assert_eq!(data, Some(vec![3]));
        assert_eq!(again.generation(), 2);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("x.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // A leftover temporary file from an interrupted save is ignored.
        std::fs::write(dir.join("x.json.saving"), b"half").unwrap();
        assert_eq!(
            JsonFile::open::<Vec<u32>>(&dir, "x.json").unwrap().1,
            Some(vec![3])
        );
        // Newer and unreadable files are refused and left untouched.
        std::fs::write(
            dir.join("x.json"),
            br#"{"version":9,"generation":1,"data":[]}"#,
        )
        .unwrap();
        assert_eq!(
            JsonFile::open::<Vec<u32>>(&dir, "x.json").err(),
            Some(StoreError::Newer(9))
        );
        std::fs::write(dir.join("x.json"), b"garbage").unwrap();
        assert!(matches!(
            JsonFile::open::<Vec<u32>>(&dir, "x.json").err(),
            Some(StoreError::Unreadable(_))
        ));
        assert_eq!(std::fs::read(dir.join("x.json")).unwrap(), b"garbage");
        std::fs::write(
            dir.join("x.json"),
            br#"{"version":1,"generation":1,"data":[],"extra":1}"#,
        )
        .unwrap();
        assert!(JsonFile::open::<Vec<u32>>(&dir, "x.json").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    pub(crate) fn tempfile_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "notdot-link-test-{}",
            u128::from_be_bytes(crate::random_bytes())
        ));
        private_dir(&dir).unwrap();
        dir
    }
}
