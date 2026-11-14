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
}
