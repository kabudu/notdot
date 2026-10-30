fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // GGML reads the Windows CPU description through the system registry.
        println!("cargo:rustc-link-lib=advapi32");
    }
}
