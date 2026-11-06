//! Encrypted voiceprint storage.
//!
//! File layout: `b"NDVP"` · version byte · 12-byte random nonce · ciphertext
//! with a 16-byte Poly1305 tag. The 5-byte header is authenticated as AAD.
//! Plaintext (binary, never JSON so no names leak into it): 32-byte model ID ·
//! u16 dim · u8 profile count · per profile: u8 id length, id bytes, u8 sample
//! count, `count × dim` little-endian f32.
//!
//! The 256-bit key lives only in the OS credential store via [`KeyStore`].
//! Missing key, wrong key, tampering or a different model are all reported as
//! errors so locked modes can fail closed; nothing silently resets.
use crate::matcher::{MAX_SAMPLES, MIN_SAMPLES, Voiceprint};
use ring::{
    aead::{Aad, CHACHA20_POLY1305, LessSafeKey, NONCE_LEN, Nonce, UnboundKey},
    rand::{SecureRandom, SystemRandom},
};
use std::path::{Path, PathBuf};
use zeroize::{Zeroize, Zeroizing};

const MAGIC: &[u8; 4] = b"NDVP";
const VERSION: u8 = 1;
const HEADER: usize = 5;
pub const MAX_FILE_BYTES: u64 = 256 * 1024;
pub const MAX_PROFILES: usize = 8;

pub type Key = Zeroizing<[u8; 32]>;

/// OS-protected key storage (Keychain, Windows Credential Manager, Secret
/// Service). Implemented in the app with the `keyring` crate.
pub trait KeyStore: Send + Sync {
    fn load(&self) -> Result<Option<Key>, String>;
    fn store(&self, key: &Key) -> Result<(), String>;
    fn delete(&self) -> Result<(), String>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum StoreError {
    /// Voiceprints exist but the device key is gone.
    KeyMissing,
    /// The key store could not be reached (locked keychain, no secret service).
    KeyUnavailable(String),
    /// Wrong key, truncated or modified file.
    Tampered,
    /// Stored for a different speaker model; re-enrolment needed.
    ModelChanged,
    Io(String),
    Invalid(&'static str),
}
impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::KeyMissing => "The voice key for this computer is missing",
            Self::KeyUnavailable(_) => "The system keychain is unavailable",
            Self::Tampered => "Saved voices could not be verified",
            Self::ModelChanged => "Saved voices need to be taught again",
            Self::Io(_) => "Saved voices could not be read or written",
            Self::Invalid(m) => m,
        })
    }
}

pub struct VoiceprintStore {
    path: PathBuf,
    keys: Box<dyn KeyStore>,
    rng: SystemRandom,
}

fn sealing_key(key: &Key) -> LessSafeKey {
    LessSafeKey::new(UnboundKey::new(&CHACHA20_POLY1305, key.as_ref()).expect("32-byte key"))
}

fn encode(model: &[u8; 32], prints: &[Voiceprint]) -> Result<Zeroizing<Vec<u8>>, StoreError> {
    if prints.len() > MAX_PROFILES {
        return Err(StoreError::Invalid("Too many voiceprints"));
    }
    let dim = prints
        .first()
        .and_then(|p| p.samples.first())
        .map_or(0, Vec::len);
    // Exact capacity (plus the tag) so no reallocation leaves plaintext
    // copies behind in freed memory.
    let size = 35
        + prints
            .iter()
            .map(|p| 2 + p.profile.len() + p.samples.len() * dim * 4)
            .sum::<usize>()
        + CHACHA20_POLY1305.tag_len();
    let mut out = Zeroizing::new(Vec::with_capacity(size));
    out.extend_from_slice(model);
    out.extend_from_slice(
        &u16::try_from(dim)
            .map_err(|_| StoreError::Invalid("dim"))?
            .to_le_bytes(),
    );
    out.push(prints.len() as u8);
    for p in prints {
        let id = p.profile.as_bytes();
        if id.is_empty() || id.len() > 64 {
            return Err(StoreError::Invalid("Invalid profile ID"));
        }
        if !(MIN_SAMPLES..=MAX_SAMPLES).contains(&p.samples.len())
            || p.samples.iter().any(|s| s.len() != dim)
        {
            return Err(StoreError::Invalid("Invalid voiceprint"));
        }
        out.push(id.len() as u8);
        out.extend_from_slice(id);
        out.push(p.samples.len() as u8);
        for s in &p.samples {
            for x in s {
                out.extend_from_slice(&x.to_le_bytes());
            }
        }
    }
    Ok(out)
}

fn decode(model: &[u8; 32], bytes: &[u8]) -> Result<Vec<Voiceprint>, StoreError> {
    struct Cursor<'a>(&'a [u8]);
    impl<'a> Cursor<'a> {
        fn take(&mut self, n: usize) -> Result<&'a [u8], StoreError> {
            if self.0.len() < n {
                return Err(StoreError::Tampered);
            }
            let (a, b) = self.0.split_at(n);
            self.0 = b;
            Ok(a)
        }
        fn byte(&mut self) -> Result<u8, StoreError> {
            Ok(self.take(1)?[0])
        }
    }
    let mut c = Cursor(bytes);
    if c.take(32)? != model {
        return Err(StoreError::ModelChanged);
    }
    let dim = u16::from_le_bytes(c.take(2)?.try_into().expect("2 bytes")) as usize;
    let count = c.byte()? as usize;
    if count > MAX_PROFILES || (count > 0 && dim == 0) || dim > 4096 {
        return Err(StoreError::Tampered);
    }
    let mut prints = Vec::with_capacity(count);
    for _ in 0..count {
        let len = c.byte()? as usize;
        let profile = std::str::from_utf8(c.take(len)?)
            .map_err(|_| StoreError::Tampered)?
            .to_owned();
        let n = c.byte()? as usize;
        if profile.is_empty() || !(MIN_SAMPLES..=MAX_SAMPLES).contains(&n) {
            return Err(StoreError::Tampered);
        }
        let mut samples = Vec::with_capacity(n);
        for _ in 0..n {
            let raw = c.take(dim * 4)?;
            samples.push(
                raw.chunks_exact(4)
                    .map(|b| f32::from_le_bytes(b.try_into().expect("4 bytes")))
                    .collect(),
            );
        }
        prints.push(Voiceprint { profile, samples });
    }
    if !c.0.is_empty() {
        return Err(StoreError::Tampered);
    }
    Ok(prints)
}

impl VoiceprintStore {
    pub fn new(path: PathBuf, keys: Box<dyn KeyStore>) -> Self {
        Self {
            path,
            keys,
            rng: SystemRandom::new(),
        }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read all voiceprints. No file means nobody is enrolled.
    pub fn load(&self, model: &[u8; 32]) -> Result<Vec<Voiceprint>, StoreError> {
        let meta = match std::fs::symlink_metadata(&self.path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(StoreError::Io(e.kind().to_string())),
            Ok(m) => m,
        };
        if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
            return Err(StoreError::Tampered);
        }
        let mut bytes = Zeroizing::new(
            std::fs::read(&self.path).map_err(|e| StoreError::Io(e.kind().to_string()))?,
        );
        if bytes.len() < HEADER + NONCE_LEN + CHACHA20_POLY1305.tag_len()
            || &bytes[..4] != MAGIC
            || bytes[4] != VERSION
        {
            return Err(StoreError::Tampered);
        }
        let key = self
            .keys
            .load()
            .map_err(StoreError::KeyUnavailable)?
            .ok_or(StoreError::KeyMissing)?;
        let header: [u8; HEADER] = bytes[..HEADER].try_into().expect("header");
        let nonce = Nonce::try_assume_unique_for_key(&bytes[HEADER..HEADER + NONCE_LEN])
            .map_err(|_| StoreError::Tampered)?;
        let plain = sealing_key(&key)
            .open_in_place(nonce, Aad::from(header), &mut bytes[HEADER + NONCE_LEN..])
            .map_err(|_| StoreError::Tampered)?;
        decode(model, plain)
    }

    /// Replace all voiceprints atomically. Creates the device key on first use.
    pub fn save(&self, model: &[u8; 32], prints: &[Voiceprint]) -> Result<(), StoreError> {
        let key = match self.keys.load().map_err(StoreError::KeyUnavailable)? {
            Some(k) => k,
            None => {
                if self.path.exists() {
                    // Never overwrite voiceprints we cannot read with a new key
                    // implicitly; recovery must remove them deliberately.
                    return Err(StoreError::KeyMissing);
                }
                let mut k = Zeroizing::new([0u8; 32]);
                self.rng
                    .fill(k.as_mut())
                    .map_err(|_| StoreError::Invalid("Randomness unavailable"))?;
                self.keys.store(&k).map_err(StoreError::KeyUnavailable)?;
                k
            }
        };
        let mut body = encode(model, prints)?;
        let mut nonce = [0u8; NONCE_LEN];
        self.rng
            .fill(&mut nonce)
            .map_err(|_| StoreError::Invalid("Randomness unavailable"))?;
        let mut header = [0u8; HEADER];
        header[..4].copy_from_slice(MAGIC);
        header[4] = VERSION;
        let mut sealed = std::mem::take(&mut *body);
        sealing_key(&key)
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(header),
                &mut sealed,
            )
            .map_err(|_| StoreError::Invalid("Encryption failed"))?;
        let mut file = Vec::with_capacity(HEADER + NONCE_LEN + sealed.len());
        file.extend_from_slice(&header);
        file.extend_from_slice(&nonce);
        file.extend_from_slice(&sealed);
        if file.len() as u64 > MAX_FILE_BYTES {
            return Err(StoreError::Invalid("Voiceprints are too large"));
        }
        write_atomic(&self.path, &file).map_err(StoreError::Io)
    }

    /// Delete every voiceprint and the device key. Profiles, memories and chats
    /// are untouched; that is the caller's separate, explicit decision.
    pub fn forget_all(&self) -> Result<(), StoreError> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(StoreError::Io(e.kind().to_string())),
        }
        self.keys.delete().map_err(StoreError::KeyUnavailable)
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let parent = path.parent().ok_or("Invalid voiceprint path")?;
    std::fs::create_dir_all(parent).map_err(|e| e.kind().to_string())?;
    let mut nonce = [0u8; 8];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| "Randomness unavailable")?;
    let tmp = parent.join(format!(
        ".voiceprints-{}.tmp",
        nonce.iter().map(|b| format!("{b:02x}")).collect::<String>()
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&tmp).map_err(|e| e.kind().to_string())?;
        file.write_all(bytes).map_err(|e| e.kind().to_string())?;
        file.sync_all().map_err(|e| e.kind().to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.kind().to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// In-memory key store for tests and QA harnesses only.
#[derive(Default)]
pub struct MemoryKeyStore(std::sync::Mutex<Option<[u8; 32]>>);
impl KeyStore for MemoryKeyStore {
    fn load(&self) -> Result<Option<Key>, String> {
        Ok(self.0.lock().expect("key").map(Zeroizing::new))
    }
    fn store(&self, key: &Key) -> Result<(), String> {
        *self.0.lock().expect("key") = Some(**key);
        Ok(())
    }
    fn delete(&self) -> Result<(), String> {
        if let Some(k) = self.0.lock().expect("key").as_mut() {
            k.zeroize();
        }
        *self.0.lock().expect("key") = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct Shared(Arc<MemoryKeyStore>);
    impl KeyStore for Shared {
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
    fn prints() -> Vec<Voiceprint> {
        vec![
            Voiceprint {
                profile: "2f1e6a1c-1111-4aaa-8bbb-000000000001".into(),
                samples: vec![vec![0.25f32; 512]; 4],
            },
            Voiceprint {
                profile: "2f1e6a1c-1111-4aaa-8bbb-000000000002".into(),
                samples: vec![vec![-0.125f32; 512]; 3],
            },
        ]
    }
    const MODEL: [u8; 32] = [7; 32];

    #[test]
    fn round_trip_is_encrypted_and_bound_to_model_and_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("speakers/voiceprints.bin");
        let keys = Arc::new(MemoryKeyStore::default());
        let store = VoiceprintStore::new(path.clone(), Box::new(Shared(keys.clone())));
        assert_eq!(store.load(&MODEL).unwrap(), vec![]);
        store.save(&MODEL, &prints()).unwrap();
        assert_eq!(store.load(&MODEL).unwrap(), prints());
        let raw = std::fs::read(&path).unwrap();
        // No profile ID or float pattern appears in plaintext.
        assert!(!raw.windows(8).any(|w| w == b"2f1e6a1c"));
        assert!(!raw.windows(4).any(|w| w == 0.25f32.to_le_bytes()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        // Each save uses a fresh nonce.
        store.save(&MODEL, &prints()).unwrap();
        assert_ne!(std::fs::read(&path).unwrap(), raw);
        assert_eq!(store.load(&[8; 32]), Err(StoreError::ModelChanged));
        // Tampering anywhere is detected, including the header.
        for i in [0usize, 4, 10, raw.len() / 2, raw.len() - 1] {
            let mut bad = std::fs::read(&path).unwrap();
            bad[i] ^= 1;
            std::fs::write(&path, &bad).unwrap();
            assert_eq!(store.load(&MODEL), Err(StoreError::Tampered), "byte {i}");
        }
        std::fs::write(&path, &raw).unwrap();
        // A different key cannot decrypt.
        let other = VoiceprintStore::new(path.clone(), Box::new(MemoryKeyStore::default()));
        assert_eq!(other.load(&MODEL), Err(StoreError::KeyMissing));
        other.keys.store(&Zeroizing::new([9; 32])).unwrap();
        assert_eq!(other.load(&MODEL), Err(StoreError::Tampered));
        // Lost key: refuse to silently re-key over existing voiceprints.
        keys.delete().unwrap();
        assert_eq!(store.load(&MODEL), Err(StoreError::KeyMissing));
        assert_eq!(store.save(&MODEL, &prints()), Err(StoreError::KeyMissing));
        // Forget all removes the file and key; then a fresh start works.
        store.forget_all().unwrap();
        assert!(!path.exists());
        assert_eq!(store.load(&MODEL).unwrap(), vec![]);
        store.save(&MODEL, &prints()[..1]).unwrap();
        assert_eq!(store.load(&MODEL).unwrap().len(), 1);
    }

    #[test]
    fn oversized_and_malformed_inputs_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v.bin");
        let store = VoiceprintStore::new(path.clone(), Box::new(MemoryKeyStore::default()));
        std::fs::write(&path, vec![0u8; MAX_FILE_BYTES as usize + 1]).unwrap();
        assert_eq!(store.load(&MODEL), Err(StoreError::Tampered));
        std::fs::write(&path, b"NDVP").unwrap();
        assert_eq!(store.load(&MODEL), Err(StoreError::Tampered));
        std::fs::remove_file(&path).unwrap();
        let mut too_few = prints();
        too_few[0].samples.truncate(2);
        assert!(store.save(&MODEL, &too_few).is_err());
        let many: Vec<_> = (0..9)
            .map(|i| Voiceprint {
                profile: format!("p{i}"),
                samples: vec![vec![0.1; 8]; 3],
            })
            .collect();
        assert!(store.save(&MODEL, &many).is_err());
    }
}
