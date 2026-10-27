#!/usr/bin/env python3
"""Stage complete, signed platform updater bundles. Does not publish anything."""
import argparse
import base64
import hashlib
import json
from pathlib import Path
import re
import shutil

PLATFORMS = {"darwin-aarch64": ".app.tar.gz", "windows-x86_64": "-setup.exe", "linux-x86_64": ".AppImage"}
LIMIT = 256 * 1024 * 1024

def stage(version, source, output):
    if not re.fullmatch(r'(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)', version):
        raise ValueError("Use a stable major.minor.patch release version")
    manifest = {"version": version, "platforms": {}}
    planned = []
    for platform, suffix in PLATFORMS.items():
        files = [p for p in (source / platform).rglob('*') if p.is_file() and p.name.endswith(suffix)]
        if len(files) != 1:
            raise ValueError(f"Expected one {platform} updater, found {len(files)}")
        artifact = files[0]
        if artifact.is_symlink() or not 0 < artifact.stat().st_size <= LIMIT:
            raise ValueError("Invalid updater size or symbolic link")
        signature_file = Path(str(artifact) + '.sig')
        if signature_file.is_symlink():
            raise ValueError("Signature cannot be a symbolic link")
        signature = signature_file.read_text().strip()
        decoded = base64.b64decode(signature, validate=True).decode()
        # Signature crypto verification is performed by Tauri before install. Require the
        # version-bound trusted comment here too; never stage old unbound signatures.
        comment = next((line for line in decoded.splitlines() if line.startswith('trusted comment: ')), '')
        if f'version:{version}' not in comment.split('\t') and not re.search(rf'(?:^|[\s:]){re.escape(version)}(?:$|\s)', comment):
            raise ValueError("Updater signature is not bound to the release version")
        filename = f"notdot_{version}_{platform}{suffix}"
        manifest['platforms'][platform] = {'url': f'https://github.com/kabudu/notdot/releases/download/v{version}/{filename}', 'signature': signature}
        planned.append((artifact, signature_file, filename))
    if output.exists() and any(output.iterdir()):
        raise ValueError("Output must be a new or empty staging directory")
    output.mkdir(parents=True, exist_ok=True)
    for artifact, signature, filename in planned:
        shutil.copyfile(artifact, output / filename)
        shutil.copyfile(signature, output / (filename + '.sig'))
    (output / 'latest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    sums = []
    for path in sorted(output.iterdir()):
        hasher = hashlib.sha256()
        with path.open('rb') as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b''):
                hasher.update(chunk)
        digest = hasher.hexdigest()
        sums.append(f'{digest}  {path.name}')
    (output / 'SHA256SUMS.txt').write_text('\n'.join(sums) + '\n')
    return manifest

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--version', required=True)
    parser.add_argument('--input', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    stage(args.version, args.input, args.output)
