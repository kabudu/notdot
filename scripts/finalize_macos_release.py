#!/usr/bin/env python3
"""Finish notarized universal distribution containers before uploading anything."""
import argparse
import hashlib
import plistlib
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
from verify_macos_release import verify_archive, verify_bundle, verify_signer


def bundle_fingerprint(app):
    info = plistlib.loads((app / 'Contents/Info.plist').read_bytes())
    paths = [app / 'Contents/MacOS' / info['CFBundleExecutable'], app / 'Contents/_CodeSignature/CodeResources']
    result = []
    for path in paths:
        with path.open('rb') as stream:
            result.append(hashlib.file_digest(stream, 'sha256').hexdigest())
    return result


def finalize(bundle, version):
    team = os.environ.get('APPLE_TEAM_ID')
    if not team:
        raise ValueError('APPLE_TEAM_ID is required')
    app = bundle / 'macos/notdot.app'
    verify_bundle(app, team, version)
    expected_fingerprint = bundle_fingerprint(app)
    # Package the ticket-bearing app, then sign these exact updater bytes.
    archive = app.with_suffix('.app.tar.gz')
    with tarfile.open(archive, 'w:gz', dereference=False) as stream:
        stream.add(app, arcname='notdot.app')
    subprocess.run(['cargo', 'tauri', 'signer', 'sign', '--app-version', version, str(archive)], check=True)
    verify_archive(archive, team, version)
    images = list((bundle / 'dmg').glob('*.dmg'))
    if len(images) != 1:
        raise ValueError('Expected exactly one signed universal disk image')
    image = images[0]
    subprocess.run(['codesign', '--verify', '--strict', str(image)], check=True)
    verify_signer(image, team)
    credentials = [os.environ.get(key, '') for key in ('APPLE_ID', 'APPLE_PASSWORD', 'APPLE_TEAM_ID')]
    if not all(credentials):
        raise ValueError('Complete Apple notarization credentials are required')
    try:
        submitted = subprocess.run(['xcrun', 'notarytool', 'submit', str(image), '--wait', '--output-format', 'json',
                                    '--apple-id', credentials[0], '--password', credentials[1], '--team-id', credentials[2]],
                                   capture_output=True, text=True, timeout=1800)
    except (subprocess.SubprocessError, OSError):
        # Do not render a subprocess command containing credential arguments.
        raise RuntimeError('Apple disk-image notarization could not finish') from None
    if submitted.returncode:
        raise RuntimeError('Apple disk-image notarization failed; inspect the Apple submission locally')
    result = json.loads(submitted.stdout)
    if result.get('status') != 'Accepted':
        raise RuntimeError('Apple did not accept the disk image')
    subprocess.run(['xcrun', 'stapler', 'staple', str(image)], check=True)
    subprocess.run(['xcrun', 'stapler', 'validate', str(image)], check=True)
    subprocess.run(['spctl', '--assess', '--type', 'open', '--context', 'context:primary-signature', str(image)], check=True)
    # Validate what a user actually mounts, not only the pre-container app.
    with tempfile.TemporaryDirectory(prefix='notdot-dmg-verify-') as mount:
        subprocess.run(['hdiutil', 'attach', '-readonly', '-nobrowse', '-mountpoint', mount, str(image)], check=True)
        try:
            mounted = Path(mount) / 'notdot.app'
            verify_bundle(mounted, team, version)
            if bundle_fingerprint(mounted) != expected_fingerprint:
                raise ValueError('Mounted app differs from the verified updater app')
        finally:
            subprocess.run(['hdiutil', 'detach', mount], check=True)
    print('Universal app, signed updater and notarized mounted disk image verified.')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('bundle', type=Path)
    args = parser.parse_args()
    version = json.loads(Path('src-tauri/tauri.conf.json').read_text())['version']
    finalize(args.bundle, version)
