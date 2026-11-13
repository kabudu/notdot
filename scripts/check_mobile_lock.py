#!/usr/bin/env python3
"""Keep the app library's lockfile in step with the desktop lockfile.

python3 scripts/check_mobile_lock.py

mobile/rust is a separate workspace only so the iOS/Android library can pin
libc below 0.2.190 (which hid iOS route/ifreq items that iroh still uses).
Every other package it locks must be locked at a version the desktop
Cargo.lock also uses, so the two graphs cannot drift silently.
"""
from pathlib import Path
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
ALLOWED = {'libc': {'0.2.189'}}


def versions(path):
    found = {}
    for package in tomllib.loads(path.read_text())['package']:
        if 'source' in package:
            found.setdefault(package['name'], set()).add(package['version'])
    return found


def main():
    desktop = versions(ROOT / 'Cargo.lock')
    mobile = versions(ROOT / 'mobile/rust/Cargo.lock')
    problems = []
    for name, locked in sorted(mobile.items()):
        extra = locked - desktop.get(name, set()) - ALLOWED.get(name, set())
        if extra:
            problems.append(f'{name} {sorted(extra)} is not in the desktop Cargo.lock')
    if mobile.get('libc') != ALLOWED['libc']:
        problems.append(f"libc must be pinned to {sorted(ALLOWED['libc'])}, found {sorted(mobile.get('libc', []))}")
    for problem in problems:
        print(problem, file=sys.stderr)
    print(f'mobile lock: {len(mobile)} registry packages, {len(problems)} drift problem(s)')
    return 1 if problems else 0


if __name__ == '__main__':
    sys.exit(main())
