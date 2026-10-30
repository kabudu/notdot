#!/usr/bin/env python3
"""Stage complete, signed platform updater bundles. Does not publish anything."""
import argparse
import base64
import hashlib
import json
from pathlib import Path
import re
import shutil
import tarfile

PLATFORMS = {"darwin-universal": ".app.tar.gz", "windows-x86_64": "-setup.exe", "linux-x86_64": ".AppImage"}
LIMIT = 256 * 1024 * 1024

def linux_source_record(archive, artifact, revision):
    if archive.is_symlink() or not 0 < archive.stat().st_size < 2 * 1024 * 1024 * 1024:
        raise ValueError("Invalid corresponding-source archive")
    record = None
    expanded = count = 0
    with tarfile.open(archive, 'r|gz') as stream:
        for item in stream:
            count += 1
            expanded += item.size
            if count > 200000 or item.size < 0 or expanded > 16 * 1024 * 1024 * 1024:
                raise ValueError("Unreasonable corresponding-source archive")
            parts = Path(item.name).parts
            if len(parts) == 2 and parts[-1] == 'manifest.json':
                if record is not None or not item.isfile() or not 0 < item.size <= 8 * 1024 * 1024:
                    raise ValueError("Invalid source manifest")
                record = json.load(stream.extractfile(item))
    if not isinstance(record, dict) or not record.get('binaries') or not record.get('sources') or not record.get('files'):
        raise ValueError("Missing complete corresponding-source inventory")
    with artifact.open('rb') as stream:
        digest = hashlib.file_digest(stream, 'sha256').hexdigest()
    if record.get('appimage_sha256') != digest or record.get('appimage_bytes') != artifact.stat().st_size:
        raise ValueError("Corresponding source belongs to a different AppImage")
    if revision is not None and record.get('notdot_revision') != revision:
        raise ValueError("Corresponding source belongs to a different revision")
    return record


def stage(version, source, output, revision=None):
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
        targets = ('darwin-aarch64', 'darwin-x86_64') if platform == 'darwin-universal' else (platform,)
        for target in targets:
            manifest['platforms'][target] = {'url': f'https://github.com/kabudu/notdot/releases/download/v{version}/{filename}', 'signature': signature}
        planned.append((artifact, signature_file, filename))
    images = list((source / 'darwin-universal').rglob('*.dmg'))
    if len(images) != 1 or images[0].is_symlink() or not 0 < images[0].stat().st_size <= LIMIT:
        raise ValueError('Expected one universal Mac disk image')
    toolsets = list((source / 'windows-x86_64').rglob('toolset.json'))
    terms = list((source / 'windows-x86_64').rglob('Microsoft-VS2022-distributable-list.html'))
    if len(toolsets) != 1 or len(terms) != 1 or any(path.is_symlink() or not 0 < path.stat().st_size <= 4 * 1024 * 1024 for path in toolsets + terms):
        raise ValueError('Expected Windows toolset and redistribution provenance')
    toolset = json.loads(toolsets[0].read_text(encoding='utf-8-sig'))
    windows_artifact = next(artifact for artifact, _, name in planned if name.endswith('-setup.exe'))
    with windows_artifact.open('rb') as stream:
        installer_digest = hashlib.file_digest(stream, 'sha256').hexdigest()
    if toolset.get('installer_sha256') != installer_digest or (revision is not None and toolset.get('revision') != revision):
        raise ValueError('Windows provenance belongs to a different installer or revision')
    if not str(toolset.get('visualStudioVersion', '')).startswith('17.') or not toolset.get('staticRuntimeInputs') or not toolset.get('nativeCompilerRecords'):
        raise ValueError('Missing official Windows toolset inventory')
    linux_artifact = next(artifact for artifact, _, name in planned if name.endswith('.AppImage'))
    companions = list((source / 'linux-x86_64').rglob('notdot-corresponding-source.tar.gz'))
    if len(companions) != 1:
        raise ValueError("Expected one complete Linux corresponding-source companion")
    linux_source_record(companions[0], linux_artifact, revision)
    debs = list((source / 'linux-x86_64').rglob('*.deb'))
    if len(debs) != 1 or debs[0].is_symlink() or not 0 < debs[0].stat().st_size <= LIMIT:
        raise ValueError('Expected one valid Linux Debian package')
    if output.exists() and any(output.iterdir()):
        raise ValueError("Output must be a new or empty staging directory")
    output.mkdir(parents=True, exist_ok=True)
    for artifact, signature, filename in planned:
        shutil.copyfile(artifact, output / filename)
        shutil.copyfile(signature, output / (filename + '.sig'))
    shutil.copyfile(toolsets[0], output / f'notdot_{version}_windows-x86_64-toolset.json')
    shutil.copyfile(terms[0], output / 'Microsoft-VS2022-distributable-list.html')
    shutil.copyfile(images[0], output / f'notdot_{version}_darwin-universal.dmg')
    shutil.copyfile(debs[0], output / f'notdot_{version}_linux-x86_64.deb')
    shutil.copyfile(companions[0], output / f'notdot_{version}_linux-x86_64-corresponding-source.tar.gz')
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
    parser.add_argument('--revision', required=True, help='exact validated release commit')
    args = parser.parse_args()
    stage(args.version, args.input, args.output, args.revision)
