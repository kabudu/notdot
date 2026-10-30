#!/usr/bin/env python3
"""Import the CI signing identity into one temporary keychain; always clean it up."""
import argparse
import base64
import json
import os
from pathlib import Path
import secrets
import shlex
import subprocess


def security(*arguments):
    try:
        result = subprocess.run(['security', *arguments], capture_output=True, text=True, timeout=60)
    except (subprocess.SubprocessError, OSError):
        raise RuntimeError('Temporary signing keychain command could not finish') from None
    if result.returncode:
        # Import/password arguments must never appear in a traceback or output.
        raise RuntimeError('Temporary signing keychain command failed')
    return result.stdout


def cleanup(folder):
    if not folder.exists():
        return
    certificate = folder / 'certificate.p12'
    certificate.unlink(missing_ok=True)
    state = folder / 'search-list.json'
    failed = False
    if state.exists():
        original = json.loads(state.read_text())
        try:
            security('list-keychains', '-d', 'user', '-s', *original)
        except RuntimeError:
            failed = True
    keychain = folder / 'release.keychain-db'
    if keychain.exists():
        try:
            security('delete-keychain', str(keychain))
        except RuntimeError:
            failed = True
    if failed:
        raise RuntimeError('Temporary signing keychain cleanup failed')
    state.unlink(missing_ok=True)
    folder.rmdir()


def setup(folder):
    encoded = os.environ.get('APPLE_CERTIFICATE', '')
    if not encoded:
        raise ValueError('Apple signing certificate is required')
    folder.mkdir(mode=0o700)
    certificate = folder / 'certificate.p12'
    try:
        original = shlex.split(security('list-keychains', '-d', 'user'))
        (folder / 'search-list.json').write_text(json.dumps(original))
        payload = base64.b64decode(''.join(encoded.split()), validate=True)
        with certificate.open('xb') as stream:
            certificate.chmod(0o600)
            stream.write(payload)
        password = secrets.token_hex(24)
        keychain = str(folder / 'release.keychain-db')
        security('create-keychain', '-p', password, keychain)
        security('set-keychain-settings', '-lut', '21600', keychain)
        security('unlock-keychain', '-p', password, keychain)
        security('import', str(certificate), '-P', os.environ.get('APPLE_CERTIFICATE_PASSWORD', ''),
                 '-A', '-t', 'cert', '-f', 'pkcs12', '-k', keychain)
        security('set-key-partition-list', '-S', 'apple-tool:,apple:', '-s', '-k', password, keychain)
        security('list-keychains', '-d', 'user', '-s', keychain, *original)
    except Exception:
        try:
            cleanup(folder)
        except Exception:
            pass  # The workflow's unconditional cleanup step retries safely.
        raise RuntimeError('Temporary macOS signing setup failed') from None
    finally:
        certificate.unlink(missing_ok=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['setup', 'cleanup'])
    parser.add_argument('folder', type=Path)
    args = parser.parse_args()
    try:
        {'setup': setup, 'cleanup': cleanup}[args.action](args.folder.resolve())
    except Exception:
        raise SystemExit(f'Temporary macOS signing {args.action} failed') from None
