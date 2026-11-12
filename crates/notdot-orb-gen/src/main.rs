//! `notdot-orb-gen lint` — fail if tracked files copy or commit shaders.
//! `notdot-orb-gen glsl <file>` — write the Android GLSL ES shader.
//! `notdot-orb-gen msl <dir>` — write `OrbFragment.metal` and `OrbVertex.metal`.
//! `notdot-orb-gen verify <file>...` — check generated files are current.
use std::{path::PathBuf, process::ExitCode};

fn write(path: PathBuf, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    // Write only on change so build systems do not rebuild needlessly.
    if std::fs::read_to_string(&path).ok().as_deref() != Some(contents) {
        std::fs::write(&path, contents).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

fn lint() -> Result<(), String> {
    let output = std::process::Command::new("git")
        .args(["ls-files", "-z"])
        .output()
        .map_err(|e| format!("git ls-files: {e}"))?;
    if !output.status.success() {
        return Err("git ls-files failed; run from the repository".into());
    }
    let paths: Vec<String> = output
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect();
    let mut files = Vec::with_capacity(paths.len());
    for path in &paths {
        // Deleted-but-unstaged files are skipped; everything else must read.
        match std::fs::read(path) {
            Ok(bytes) => files.push((path.as_str(), bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{path}: {e}")),
        }
    }
    let violations = notdot_orb_gen::lint(files.iter().map(|(p, b)| (*p, b.as_slice())));
    for violation in &violations {
        eprintln!("{}: {}", violation.path, violation.reason);
    }
    if violations.is_empty() {
        println!("orb shader lint: {} tracked files clean", files.len());
        Ok(())
    } else {
        Err(format!("{} orb shader violation(s)", violations.len()))
    }
}

fn run(args: &[String]) -> Result<(), String> {
    match args {
        [command] if command == "lint" => lint(),
        [command, out] if command == "glsl" => write(out.into(), &notdot_orb_gen::glsl_es()),
        [command, dir] if command == "msl" => {
            let dir = PathBuf::from(dir);
            let fragment = notdot_orb_gen::msl_fragment().map_err(|e| e.to_string())?;
            let vertex = notdot_orb_gen::msl_vertex().map_err(|e| e.to_string())?;
            write(dir.join("OrbFragment.metal"), &fragment.source)?;
            write(dir.join("OrbVertex.metal"), &vertex.source)?;
            println!(
                "fragment={} vertex={}",
                fragment.entry_point, vertex.entry_point
            );
            Ok(())
        }
        [command, files @ ..] if command == "verify" && !files.is_empty() => {
            for file in files {
                let text = std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
                notdot_orb_gen::verify(&text).map_err(|e| format!("{file}: {e}"))?;
            }
            Ok(())
        }
        _ => Err("usage: notdot-orb-gen lint | glsl <file> | msl <dir> | verify <file>...".into()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("notdot-orb-gen: {error}");
            ExitCode::FAILURE
        }
    }
}
