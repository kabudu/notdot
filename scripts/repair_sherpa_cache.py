#!/usr/bin/env python3
"""Discard Sherpa's partial target cache before a fresh release build.

rust-cache prunes files outside Cargo profile directories but retains directories.
The pinned Sherpa build script trusts an existing lib/ directory even when empty.
Cached Rust rlibs can hide this until the first build in another target/profile.
"""
import argparse
import os
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def repair(target_dir, targets, run=subprocess.run):
    cache = target_dir / 'sherpa-onnx-prebuilt'
    if cache.is_symlink():
        raise ValueError('Refusing a symlinked Sherpa cache')
    if cache.exists():
        shutil.rmtree(cache)
    # Invalidate retained build-script link paths as well as bundled Rust libs.
    base = ['cargo', 'clean', '-p', 'sherpa-onnx-sys', '--target-dir', str(target_dir)]
    run(base, cwd=ROOT, check=True)
    for target in targets:
        run([*base, '--target', target], cwd=ROOT, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target', required=True)
    args = parser.parse_args()
    target_dir = Path(os.environ.get('CARGO_TARGET_DIR', ROOT / 'target')).resolve()
    targets = ['aarch64-apple-darwin', 'x86_64-apple-darwin'] if args.target == 'universal-apple-darwin' else [args.target]
    repair(target_dir, targets)


if __name__ == '__main__':
    main()
