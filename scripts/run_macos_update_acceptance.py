#!/usr/bin/env python3
"""Opt-in native, no-window installed update/restart fixture. No real account/runtime.

Creates a private lab under .local, signs only synthetic fixture packages with a
new disposable key, deletes that private key before launch, and retains evidence.
The fake writer uses the real notdot Bridge and an exclusive OS file lease.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / 'scripts/update_acceptance_fixture/Cargo.toml'
TARGET = ROOT / '.local/macos-readiness/installer-probe/target'
BINARY = TARGET / 'debug/notdot-update-acceptance-fixture'
IDENTIFIER = 'com.kabudu.notdot.update-acceptance-fixture'


def run_native():
    if sys.platform != 'darwin':
        raise RuntimeError('this fixture is macOS-only')
    os.umask(0o077)
    base = ROOT / '.local/macos-update-acceptance'
    base.mkdir(parents=True, exist_ok=True)
    lab = Path(tempfile.mkdtemp(prefix='run-', dir=base))
    (lab / 'fixture-only').write_text('notdot-disposable-native-update')
    profile = b'{"thread":"synthetic-update-thread","notes":"synthetic fixture only"}'
    (lab / 'profile.json').write_bytes(profile)
    env = {k: v for k, v in os.environ.items() if k not in (
        'TAURI_SIGNING_PRIVATE_KEY', 'TAURI_SIGNING_PRIVATE_KEY_PATH',
        'TAURI_SIGNING_PRIVATE_KEY_PASSWORD', 'TAURI_CONFIG')}
    env['CARGO_BUILD_JOBS'] = '2'
    env['CARGO_TARGET_DIR'] = str(TARGET)
    name = 'notdot-update-acceptance-fixture'
    def bundle(version, destination):
        build_env = dict(env, TAURI_CONFIG=json.dumps({'version': version}))
        with (lab / ('build-' + version + '.log')).open('wb') as log:
            subprocess.run(['cargo', 'build', '--manifest-path', str(MANIFEST), '--locked', '--offline'],
                           cwd=ROOT, env=build_env, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=300)
        executable = destination / 'Contents/MacOS' / name
        executable.parent.mkdir(parents=True)
        shutil.copyfile(BINARY, executable)
        executable.chmod(0o755)
        info = {'CFBundleExecutable': name, 'CFBundleIdentifier': IDENTIFIER,
                'CFBundleName': 'notdot Update Acceptance Fixture', 'CFBundlePackageType': 'APPL',
                'CFBundleShortVersionString': version, 'CFBundleVersion': version,
                'LSUIElement': True, 'LSMinimumSystemVersion': '14.0'}
        (destination / 'Contents/Info.plist').write_bytes(plistlib.dumps(info))
        subprocess.run(['codesign', '--force', '--deep', '--sign', '-', str(destination)],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True, timeout=20)
        subprocess.run(['codesign', '--verify', '--strict', '--deep', str(destination)],
                       capture_output=True, check=True, timeout=20)
        return executable
    live = lab / 'Installed Fixture.app'
    old = bundle('9.9.8', live)
    shutil.copyfile(old, lab / 'fixture-writer')
    (lab / 'fixture-writer').chmod(0o755)
    new = lab / 'prepared/Installed Fixture.app'
    bundle('9.9.9', new)
    package = lab / 'update.app.tar.gz'
    with tarfile.open(package, 'w:gz') as archive:
        archive.add(new, arcname='Installed Fixture.app')
    (lab / 'malformed.bin').write_bytes(b'validly-signed-not-a-gzip-archive')
    key, private_log = lab / 'fixture-key', lab / 'private-generation.log'
    try:
        with private_log.open('wb') as output:
            subprocess.run(['cargo', 'tauri', 'signer', 'generate', '--ci', '--password', '',
                            '--write-keys', str(key)], cwd=ROOT, env=env, stdout=output,
                           stderr=subprocess.STDOUT, check=True, timeout=30)
            key.chmod(0o600)
            shutil.copyfile(lab / 'fixture-key.pub', lab / 'fixture.pub')
            for payload in (package, lab / 'malformed.bin'):
                subprocess.run(['cargo', 'tauri', 'signer', 'sign', '--private-key-path', str(key),
                                '--password', '', '--app-version', '9.9.9', str(payload)],
                               cwd=ROOT, env=env, stdout=output, stderr=subprocess.STDOUT,
                               check=True, timeout=30)
    finally:
        key.unlink(missing_ok=True)
        private_log.unlink(missing_ok=True)
    assert not key.exists() and not private_log.exists()
    metadata = {'version': '9.9.9', 'platforms': {'fixture-macos': {
        'url': 'https://example.invalid/not-requested.app.tar.gz',
        'signature': (lab / 'update.app.tar.gz.sig').read_text().strip()}}}
    (lab / 'update.json').write_text(json.dumps(metadata))
    with (lab / 'native-process.log').open('wb') as output:
        process = subprocess.Popen([str(old), '--lab', str(lab)], cwd=lab,
                                   stdin=subprocess.DEVNULL, stdout=output, stderr=output, env=env)
        deadline = time.monotonic() + 70
        while time.monotonic() < deadline and not (lab / 'result.json').exists():
            if process.poll() not in (None, 0):
                raise RuntimeError('native fixture exited before result; see ' + str(lab))
            time.sleep(0.1)
        if not (lab / 'result.json').exists():
            if process.poll() is None:
                process.kill()
            raise RuntimeError('native fixture timed out; see ' + str(lab))
        result = json.loads((lab / 'result.json').read_text())
        process.wait(timeout=10)
    events = [json.loads(s) for s in (lab / 'events.jsonl').read_text().splitlines()]
    kinds = [x['event'] for x in events]
    assert result.get('passed') is True, result
    assert 'failed_install_recovered' in kinds and 'installed_before_restart' in kinds
    assert 'duplicate_rejected' in kinds and 'writer_overlap' not in kinds and 'watchdog_timeout' not in kinds
    owners = set()
    for event in events:
        if event['event'] == 'writer_started':
            owners.add(event['pid'])
            assert len(owners) == 1, 'overlapping sole writers'
        elif event['event'] == 'writer_stopped':
            owners.remove(event['pid'])
    assert not owners and (lab / 'profile.json').read_bytes() == profile
    owned_pids = {event['pid'] for event in events}
    def alive(pid):
        try:
            os.kill(pid, 0)
            return True
        except ProcessLookupError:
            return False
    deadline = time.monotonic() + 5
    while any(alive(pid) for pid in owned_pids) and time.monotonic() < deadline:
        time.sleep(0.05)
    assert not any(alive(pid) for pid in owned_pids), 'fixture process did not exit'
    assert plistlib.loads((live / 'Contents/Info.plist').read_bytes())['CFBundleShortVersionString'] == '9.9.9'
    assert old.read_bytes() == (new / 'Contents/MacOS' / name).read_bytes(), 'installed executable differs from signed replacement'
    subprocess.run(['codesign', '--verify', '--strict', '--deep', str(live)], capture_output=True, check=True)
    summary = {'passed': True, 'lab': str(lab), 'fixture_identifier': IDENTIFIER,
               'installed_version': '9.9.9', 'writer_processes': kinds.count('writer_started'),
               'maximum_concurrent_writers': 1, 'profile_preserved': True,
               'failure_recovery': True, 'duplicate_rejected': True,
               'private_key_retained': False, 'all_fixture_pids_exited': True,
               'installed_executable_sha256': hashlib.sha256(old.read_bytes()).hexdigest(),
               'scope': 'real pinned native updater install/restart/single-instance and actual Bridge against fake RPC; not notdot consent/download UI, production signing key, GitHub publication or real Codex account'}
    (lab / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    print(json.dumps(summary, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--run-native-fixture', action='store_true')
    if not parser.parse_args().run_native_fixture:
        parser.error('explicit --run-native-fixture required; launches only the disposable no-window fixture')
    run_native()


if __name__ == '__main__':
    main()
