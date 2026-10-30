#!/usr/bin/env python3
"""Build optional guarded Windows wake acceleration in a separate Cargo cache.

Only the helper subprocess receives ISA flags. The normal app remains generic.
No installation, runtime execution, network credential access or publishing.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
FLAGS = {"GGML_NATIVE": "OFF", "GGML_SSE42": "ON", "GGML_AVX": "ON",
         "GGML_AVX2": "ON", "GGML_FMA": "ON", "GGML_F16C": "ON",
         "GGML_BMI2": "ON", "GGML_AVX512": "OFF", "GGML_AVX_VNNI": "OFF"}
GUARD = "/clang:-mno-fsgsbase /clang:-mno-invpcid /clang:-mno-sahf"


def optimized_build(cache):
    values = {}
    for line in cache.splitlines():
        if '=' in line and not line.startswith(('#', '//')):
            key, value = line.split('=', 1)
            values[key.split(':', 1)[0]] = value
    if values.get('CMAKE_GENERATOR') != 'Ninja':
        raise ValueError('The guarded helper must use the explicit Ninja compiler path')
    profile = values.get('CMAKE_BUILD_TYPE', '')
    if profile not in ('Release', 'RelWithDebInfo'):
        raise ValueError('The guarded helper must use an optimized build profile')
    flags = values.get('CMAKE_CXX_FLAGS', '') + ' ' + values.get('CMAKE_CXX_FLAGS_' + profile.upper(), '')
    if not re.search(r'(?:^|\s)(?:/O2|/Ox|-O[23])(?:\s|$)', flags) or re.search(r'(?:^|\s)(?:/Od|-O0)(?:\s|$)', flags):
        raise ValueError('The guarded helper is missing effective C++ optimization')
    return {'generator': values['CMAKE_GENERATOR'], 'build_type': profile,
            'cxx_flags': flags.strip(), 'cxx_compiler': values.get('CMAKE_CXX_COMPILER', '')}

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target-dir", type=Path, default=ROOT / ".local/windows-wake-helper-target")
    parser.add_argument("--container-image", help="Optional existing cargo-xwin/LLVM20 image for a non-Windows host")
    args = parser.parse_args()
    target = args.target_dir.resolve()
    forbidden = {ROOT / "target", ROOT / ".local/windows-e2e/target"}
    if os.environ.get("CARGO_TARGET_DIR"):
        inherited = Path(os.environ["CARGO_TARGET_DIR"])
        forbidden.add((inherited if inherited.is_absolute() else ROOT / inherited).resolve())
    if target in forbidden:
        raise SystemExit("Use a dedicated helper target directory, never the app's Cargo cache")
    if not args.container_image and sys.platform != "win32":
        raise SystemExit("Build on Windows with clang-cl, or choose the existing cross-build container")
    env = os.environ.copy()
    env.update(FLAGS)
    env.update(CMAKE_C_FLAGS=GUARD, CMAKE_CXX_FLAGS=GUARD + " /EHsc", CARGO_BUILD_JOBS="2")
    target.mkdir(parents=True, exist_ok=True)
    base = ["build", "-p", "notdot-wake-runtime", "--bin", "notdot-wake-helper",
            "--features", "accelerated", "--release", "--target", "x86_64-pc-windows-msvc", "--locked", "-j", "2"]
    if args.container_image:
        cmd = ["docker", "run", "--rm", "--cpus", "2", "--memory", "4g", "--memory-swap", "4g",
               "-v", f"{ROOT}:/work", "-v", f"{target}:/work/.local/windows-e2e/target",
               "-v", "notdot-windows-cargo:/usr/local/cargo/registry",
               "-v", "notdot-windows-xwin:/root/.cache"]
        for key, value in {**FLAGS, "CMAKE_C_FLAGS": GUARD, "CMAKE_CXX_FLAGS": GUARD + " /EHsc",
                           "CARGO_TARGET_DIR": "/work/.local/windows-e2e/target", "CARGO_BUILD_JOBS": "2"}.items():
            cmd.extend(["-e", f"{key}={value}"])
        cmd.extend([args.container_image, "cargo", "xwin", *base])
    else:
        if not shutil.which("clang-cl"):
            raise SystemExit("clang-cl is required for the guarded helper build")
        if not shutil.which("ninja"):
            raise SystemExit("Ninja is required for the guarded helper build")
        # The cmake crate's implicit Visual Studio path replaces release flags
        # with non-optimizing flags and can select a different compiler. Use the
        # same explicit compiler/generator path as the validated cross-build.
        env.update(CC="clang-cl", CXX="clang-cl", CMAKE_C_COMPILER="clang-cl",
                   CMAKE_CXX_COMPILER="clang-cl", CMAKE_GENERATOR="Ninja")
        cmd = ["cargo", *base, "--target-dir", str(target)]
    # Fresh dedicated caches are safest. When reusing an earlier helper cache,
    # discard only its native decoder outputs so changed flags cannot be hidden.
    if (target / "x86_64-pc-windows-msvc/release").exists():
        if args.container_image:
            clean = cmd[:cmd.index(args.container_image) + 1] + ["cargo", "clean", "-p", "whisper-rs-sys", "--release", "--target", "x86_64-pc-windows-msvc"]
        else:
            clean = ["cargo", "clean", "-p", "whisper-rs-sys", "--release", "--target", "x86_64-pc-windows-msvc", "--target-dir", str(target)]
        subprocess.run(clean, cwd=ROOT, env=env, check=True)
    subprocess.run(cmd, cwd=ROOT, env=env, check=True)
    caches = list((target / "x86_64-pc-windows-msvc/release/build").glob("whisper-rs-sys-*/out/build/CMakeCache.txt"))
    if not caches:
        raise SystemExit("Missing native helper build evidence")
    cache = max(caches, key=lambda path: path.stat().st_mtime).read_text()
    native_build = optimized_build(cache)
    for key, value in FLAGS.items():
        if not re.search(rf"^{key}:[^=]+={value}$", cache, re.M):
            raise SystemExit(f"Unexpected helper build flag: {key}")
    if any(flag not in cache for flag in GUARD.split()):
        raise SystemExit("Helper compiler guard flags were not applied")
    source = target / "x86_64-pc-windows-msvc/release/notdot-wake-helper.exe"
    data = source.read_bytes()
    if not 0 < len(data) <= 64 * 1024 * 1024:
        raise SystemExit("Invalid helper size")
    if len(data) < 64 or data[:2] != b"MZ":
        raise SystemExit("Helper is not a Windows executable")
    offset = struct.unpack_from("<I", data, 0x3c)[0]
    if offset + 6 > len(data) or data[offset:offset + 4] != b"PE\0\0" or struct.unpack_from("<H", data, offset + 4)[0] != 0x8664:
        raise SystemExit("Helper is not a Windows x64 executable")
    destination = ROOT / "src-tauri/resources/wake-accelerated"
    destination.mkdir(parents=True, exist_ok=True)
    pending = destination / "notdot-wake-helper.exe.tmp"
    pending.write_bytes(data)
    pending.replace(destination / "notdot-wake-helper.exe")
    metadata = {"sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data), "protocol": 1,
                "ggml_flags": FLAGS, "compiler_exclusions": GUARD.split(),
                "native_build": native_build}
    (destination / "BUILD.json").write_text(json.dumps(metadata, indent=2) + "\n")
    print(json.dumps(metadata))

if __name__ == "__main__":
    main()
