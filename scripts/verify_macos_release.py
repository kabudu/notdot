#!/usr/bin/env python3
"""Refuse ad-hoc, wrong-team or unstapled Mac release apps and updater archives."""
import argparse
import json
import os
from pathlib import Path
import plistlib
import subprocess
import tarfile
import tempfile


def verify_signer(path, team):
    details = subprocess.run(['codesign', '--display', '--verbose=4', str(path)], check=True, capture_output=True, text=True).stderr
    if f'TeamIdentifier={team}' not in details.splitlines() or not any(line.startswith('Authority=Developer ID Application:') for line in details.splitlines()):
        raise ValueError('Release container must use the configured Developer ID team, not ad-hoc signing')


def verify_bundle(app, team, version):
    info = plistlib.loads((app / 'Contents/Info.plist').read_bytes())
    if info.get('CFBundleIdentifier') != 'com.kabudu.notdot' or info.get('CFBundleShortVersionString') != version:
        raise ValueError('Unexpected app identity or version')
    executable = app / 'Contents/MacOS' / info['CFBundleExecutable']
    slices = subprocess.check_output(['lipo', '-archs', str(executable)], text=True).split()
    if set(slices) != {'arm64', 'x86_64'}:
        raise ValueError('Release app must be universal: arm64 and x86_64')
    subprocess.run(['codesign', '--verify', '--deep', '--strict', str(app)], check=True)
    verify_signer(app, team)
    subprocess.run(['xcrun', 'stapler', 'validate', str(app)], check=True)
    subprocess.run(['spctl', '--assess', '--type', 'execute', str(app)], check=True)


def verify_archive(archive, team, version):
    if not 0 < archive.stat().st_size <= 256 * 1024 * 1024:
        raise ValueError('Updater archive exceeds the app download budget')
    with tempfile.TemporaryDirectory(prefix='notdot-release-verify-') as folder:
        with tarfile.open(archive, 'r:gz') as package:
            members = package.getmembers()
            if len(members) > 10000 or sum(item.size for item in members) > 1024 * 1024 * 1024:
                raise ValueError('Unreasonable updater extraction size')
            package.extractall(folder, members=members, filter='data')
        root = Path(folder)
        entries = list(root.iterdir())
        if len(entries) != 1 or entries[0].name != 'notdot.app' or entries[0].is_symlink():
            raise ValueError('Updater must contain exactly notdot.app')
        verify_bundle(entries[0], team, version)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('app', type=Path)
    args = parser.parse_args()
    team = os.environ.get('APPLE_TEAM_ID', '')
    if not team:
        raise SystemExit('APPLE_TEAM_ID is required')
    version = json.loads(Path('src-tauri/tauri.conf.json').read_text())['version']
    verify_bundle(args.app, team, version)
    verify_archive(args.app.with_suffix('.app.tar.gz'), team, version)
    print('App and actual updater archive pass Developer ID, ticket and Gatekeeper checks.')
