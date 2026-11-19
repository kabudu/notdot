//! Generate the platform shaders from the canonical orb.frag into OUT_DIR.
//! Nothing generated is written to the source tree.
use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let canonical = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"))
        .join("../notdot-orb/src/orb.frag");
    println!("cargo:rerun-if-changed={}", canonical.display());
    fs::write(out.join("orb.frag"), notdot_orb_gen::glsl_es()).expect("write GLSL");
    fs::write(out.join("orb.frag.sha256"), notdot_orb_gen::shader_sha256()).expect("write hash");
    let fragment = notdot_orb_gen::msl_fragment().expect("generate fragment MSL");
    let vertex = notdot_orb_gen::msl_vertex().expect("generate vertex MSL");
    fs::write(out.join("OrbFragment.metal"), fragment.source).expect("write MSL");
    fs::write(out.join("OrbVertex.metal"), vertex.source).expect("write MSL");
    ort_shim();
}

/// The ONNX Runtime shim for the experimental personal wake candidate,
/// compiled only against the pinned official headers (see
/// native/onnxruntime/README.md), and only with `personal-phonetic`.
fn ort_shim() {
    const HEADERS: [(&str, &str); 3] = [
        (
            "onnxruntime_c_api.h",
            "b69a133143c0da61782b2b02cc8a620d21ac139231fd211719776d10385135c1",
        ),
        (
            "onnxruntime_ep_c_api.h",
            "db86df0f846b8d3bdbf429a5c76c5805293196575ca3c475878612591cd18e51",
        ),
        (
            "onnxruntime_error_code.h",
            "5ce3b054e798eced8d14f5b86e98692fd33470463f96194ce0700a2d53dd8721",
        ),
    ];
    if env::var_os("CARGO_FEATURE_PERSONAL_PHONETIC").is_none()
        || env::var("CARGO_CFG_WINDOWS").is_ok()
    {
        return;
    }
    let native =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir")).join("native");
    let shim = native.join("ort_shim.c");
    println!("cargo:rerun-if-changed={}", shim.display());
    use sha2::{Digest, Sha256};
    for (name, pin) in HEADERS {
        let header = native.join("onnxruntime").join(name);
        println!("cargo:rerun-if-changed={}", header.display());
        let digest = Sha256::digest(fs::read(&header).expect("read ONNX Runtime header"));
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, pin, "{name} does not match its pin");
    }
    cc::Build::new()
        .file(&shim)
        .include(native.join("onnxruntime"))
        .warnings(true)
        .extra_warnings(true)
        .compile("notdot_ort_shim");
}
