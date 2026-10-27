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
    tauri_build::build()
}
