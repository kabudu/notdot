#!/usr/bin/env python3
"""Finish notarized universal distribution containers before uploading anything."""
import argparse
import hashlib
import plistlib
import re
import json
import os
from pathlib import Path
import subprocess
import time
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


def staple(path):
    # Accepted notarization can precede ticket propagation. Tauri's current
    # stapler wrapper ignores the command exit status; verify it ourselves.
    for attempt in range(5):
        try:
            result = subprocess.run(['xcrun', 'stapler', 'staple', '-v', str(path)],
                                    capture_output=True, timeout=120)
            if result.returncode == 0:
                result = subprocess.run(['xcrun', 'stapler', 'validate', str(path)],
                                        capture_output=True, timeout=120)
                if result.returncode == 0:
                    return
        except (subprocess.SubprocessError, OSError):
            pass
        if attempt < 4:
            time.sleep(15)
    raise RuntimeError('Could not staple and validate the accepted macOS ticket')


def finalize(bundle, version):
    if not re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?', version):
        raise ValueError('Invalid release version')
    team = os.environ.get('APPLE_TEAM_ID')
    identity = os.environ.get('APPLE_SIGNING_IDENTITY')
    if not team or not identity:
        raise ValueError('Apple team and Developer ID signing identity are required')
    bundle = bundle.resolve()
    app = bundle / 'macos/notdot.app'
    staple(app)
    verify_bundle(app, team, version)
    expected_fingerprint = bundle_fingerprint(app)
    # Build and sign the exact ticket-bearing archive, preserving native metadata.
    archive = app.with_suffix('.app.tar.gz')
    archive.unlink(missing_ok=True)
    Path(str(archive) + '.sig').unlink(missing_ok=True)
    environment = os.environ.copy()
    # The notarization ticket is the regular Contents/CodeResources file.
    # AppleDouble xattr sidecars break the pinned updater's root stripping.
    environment['COPYFILE_DISABLE'] = '1'
    subprocess.run(['/usr/bin/tar', '-czf', str(archive), '-C', str(app.parent), app.name],
                   check=True, timeout=180, env=environment)
    subprocess.run(['cargo', 'tauri', 'signer', 'sign', '--app-version', version, str(archive)], check=True, timeout=180)
    verify_archive(archive, team, version)
    # Recreate the DMG from this same app, rather than a pre-stapling Tauri copy.
    image_dir = bundle / 'dmg'
    image_dir.mkdir(exist_ok=True)
    image = image_dir / f'notdot_{version}_universal.dmg'
    if any(path != image for path in image_dir.glob('*.dmg')):
        raise ValueError('Unexpected extra disk image in the release directory')
    with tempfile.TemporaryDirectory(prefix='notdot-dmg-stage-') as stage:
        subprocess.run(['/usr/bin/ditto', str(app), str(Path(stage) / app.name)], check=True, timeout=180)
        (Path(stage) / 'Applications').symlink_to('/Applications')
        subprocess.run(['hdiutil', 'create', '-ov', '-volname', 'notdot', '-srcfolder', stage,
                        '-format', 'UDZO', str(image)], check=True, timeout=180)
    subprocess.run(['codesign', '--force', '--timestamp', '--sign', identity, str(image)], check=True, timeout=180)
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
    staple(image)
    subprocess.run(['spctl', '--assess', '--type', 'open', '--context', 'context:primary-signature', str(image)], check=True)
    # Validate what a user actually mounts, not only the pre-container app.
    with tempfile.TemporaryDirectory(prefix='notdot-dmg-verify-') as mount:
        attached = False
        try:
            subprocess.run(['hdiutil', 'attach', '-readonly', '-nobrowse', '-mountpoint', mount, str(image)], check=True)
            attached = True
            mounted = Path(mount) / 'notdot.app'
            verify_bundle(mounted, team, version)
            if bundle_fingerprint(mounted) != expected_fingerprint:
                raise ValueError('Mounted app differs from the verified updater app')
        finally:
            # A failed attach can leave a partial mount; attempt cleanup too.
            subprocess.run(['hdiutil', 'detach', mount], check=attached)
    print('Universal app, signed updater and notarized mounted disk image verified.')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('bundle', type=Path)
    args = parser.parse_args()
    version = json.loads(Path('src-tauri/tauri.conf.json').read_text())['version']
    finalize(args.bundle, version)
