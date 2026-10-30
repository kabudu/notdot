//! Optional package selection. No runtime path/environment override is accepted.
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
};
pub fn capable_cpu() -> bool {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        std::is_x86_feature_detected!("sse3")
            && std::is_x86_feature_detected!("ssse3")
            && std::is_x86_feature_detected!("sse4.1")
            && std::is_x86_feature_detected!("sse4.2")
            && std::is_x86_feature_detected!("avx")
            && std::is_x86_feature_detected!("avx2")
            && std::is_x86_feature_detected!("fma")
            && std::is_x86_feature_detected!("f16c")
            && std::is_x86_feature_detected!("bmi1")
            && std::is_x86_feature_detected!("bmi2")
            && std::is_x86_feature_detected!("lzcnt")
            && std::is_x86_feature_detected!("popcnt")
            && std::is_x86_feature_detected!("pclmulqdq")
            && std::is_x86_feature_detected!("movbe")
            && std::is_x86_feature_detected!("rdrand")
            && std::is_x86_feature_detected!("xsave")
            && std::is_x86_feature_detected!("xsaveopt")
            && std::is_x86_feature_detected!("cmpxchg16b")
    }
    #[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
    {
        false
    }
}
pub fn select(resource_root: &Path, expected: &str, capable: bool) -> Option<PathBuf> {
    if !capable || expected.len() != 64 || !expected.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let root = resource_root.canonicalize().ok()?;
    let file = root.join("wake-accelerated/notdot-wake-helper.exe");
    let metadata = std::fs::symlink_metadata(&file).ok()?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() == 0
        || metadata.len() > 64 * 1024 * 1024
    {
        return None;
    }
    let canonical = file.canonicalize().ok()?;
    if !canonical.starts_with(&root)
        || canonical.parent()? != root.join("wake-accelerated").canonicalize().ok()?
    {
        return None;
    }
    let mut input = std::fs::File::open(&canonical)
        .ok()?
        .take(64 * 1024 * 1024 + 1);
    let mut hash = Sha256::new();
    let mut bytes = [0u8; 65536];
    let mut count = 0u64;
    loop {
        let n = input.read(&mut bytes).ok()?;
        if n == 0 {
            break;
        }
        count += n as u64;
        if count > 64 * 1024 * 1024 {
            return None;
        }
        hash.update(&bytes[..n]);
    }
    if format!("{:x}", hash.finalize()) != expected.to_ascii_lowercase() {
        return None;
    }
    Some(canonical)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unsupported_missing_and_tampered_packages_are_not_selected() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir_all(root.join("wake-accelerated")).unwrap();
        let digest = format!("{:x}", Sha256::digest(b"synthetic helper"));
        assert!(select(root, &digest, true).is_none());
        let path = root.join("wake-accelerated/notdot-wake-helper.exe");
        std::fs::write(&path, b"synthetic helper").unwrap();
        assert!(select(root, &digest, false).is_none());
        assert_eq!(
            select(root, &digest, true).unwrap(),
            path.canonicalize().unwrap()
        );
        std::fs::write(&path, b"tampered helper").unwrap();
        assert!(select(root, &digest, true).is_none());
    }
    #[cfg(unix)]
    #[test]
    fn package_symlinks_cannot_escape_the_resource_root() {
        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let helper = outside.path().join("notdot-wake-helper.exe");
        std::fs::write(&helper, b"synthetic helper").unwrap();
        let digest = format!("{:x}", Sha256::digest(b"synthetic helper"));
        std::os::unix::fs::symlink(outside.path(), directory.path().join("wake-accelerated"))
            .unwrap();
        assert!(select(directory.path(), &digest, true).is_none());
        std::fs::remove_file(directory.path().join("wake-accelerated")).unwrap();
        std::fs::create_dir(directory.path().join("wake-accelerated")).unwrap();
        std::os::unix::fs::symlink(
            helper,
            directory
                .path()
                .join("wake-accelerated/notdot-wake-helper.exe"),
        )
        .unwrap();
        assert!(select(directory.path(), &digest, true).is_none());
    }
}
