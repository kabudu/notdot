fn main() {
    use sha2::{Digest, Sha256};
    for folder in ["resources/wake", "resources/wake/phonetic"] {
        println!("cargo:rerun-if-changed={folder}");
        let manifest = std::fs::read_to_string(format!("{folder}/SHA256SUMS"))
            .expect("bundled wake model checksums");
        for line in manifest.lines() {
            let (expected, name) = line.split_once("  ").expect("model checksum entry");
            assert!(
                !name.contains('/') && !name.contains('\\'),
                "model file name"
            );
            let bytes =
                std::fs::read(format!("{folder}/{name}")).expect("bundled wake model asset");
            assert_eq!(
                format!("{:x}", Sha256::digest(&bytes)),
                expected,
                "wake model checksum: {name}"
            );
        }
    }
    let helper = std::path::Path::new("resources/wake-accelerated/notdot-wake-helper.exe");
    println!("cargo:rerun-if-changed=resources/wake-accelerated");
    let digest =
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") && helper.is_file() {
            let metadata = std::fs::symlink_metadata(helper).expect("helper metadata");
            assert!(
                !metadata.file_type().is_symlink()
                    && metadata.len() > 0
                    && metadata.len() <= 64 * 1024 * 1024,
                "invalid helper package"
            );
            let bytes = std::fs::read(helper).expect("optional wake helper");
            assert!(
                bytes.len() >= 64 && &bytes[..2] == b"MZ",
                "wake helper must be a Windows executable"
            );
            let pe = u32::from_le_bytes(bytes[60..64].try_into().expect("PE header")) as usize;
            assert!(
                pe.checked_add(6).is_some_and(|end| end <= bytes.len())
                    && &bytes[pe..pe + 4] == b"PE\0\0"
                    && u16::from_le_bytes([bytes[pe + 4], bytes[pe + 5]]) == 0x8664,
                "wake helper must target Windows x86-64"
            );
            format!("{:x}", Sha256::digest(bytes))
        } else {
            String::new()
        };
    println!("cargo:rustc-env=NOTDOT_WAKE_HELPER_SHA256={digest}");
    tauri_build::build()
}
