#!/usr/bin/env python3
"""Install pinned official build tools without compiling them or trusting Cargo caches."""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import shutil
import stat
import subprocess
import tarfile
import tempfile
from urllib.request import Request, urlopen
import zipfile

VERSIONS = {'trunk': '0.22.1', 'cargo-tauri': '2.12.4'}
ORIGINS = {'trunk': 'https://github.com/trunk-rs/trunk/releases/download/v0.22.1/',
           'cargo-tauri': 'https://github.com/tauri-apps/tauri/releases/download/tauri-cli-v2.12.4/'}
MAX_ARCHIVE = 16 * 1024 * 1024
MAX_BINARY = 64 * 1024 * 1024


def host_platform():
    machine = {'amd64': 'x86_64', 'x86_64': 'x86_64', 'aarch64': 'arm64', 'arm64': 'arm64'}.get(platform.machine().lower())
    return f'{platform.system()}-{machine}'


def download(url):
    with urlopen(Request(url, headers={'User-Agent': 'notdot-release-tools'}), timeout=60) as response:
        return response.read(MAX_ARCHIVE + 1)


def executable_bytes(data, entry):
    if len(data) != entry['archive_bytes'] or len(data) > MAX_ARCHIVE or hashlib.sha256(data).hexdigest() != entry['sha256']:
        raise ValueError('Release-tool archive size or SHA-256 mismatch')
    stream = io.BytesIO(data)
    if entry['url'].endswith('.zip'):
        with zipfile.ZipFile(stream) as archive:
            matches = [item for item in archive.infolist() if PurePosixPath(item.filename).name == entry['executable'] and not item.is_dir()]
            if len(matches) != 1:
                raise ValueError('Expected exactly one release-tool executable')
            item = matches[0]
            if stat.S_ISLNK(item.external_attr >> 16) or item.file_size > MAX_BINARY:
                raise ValueError('Unsafe release-tool archive entry')
            name = item.filename
            with archive.open(item) as binary:
                payload = binary.read(MAX_BINARY + 1)
    else:
        with tarfile.open(fileobj=stream, mode='r:gz') as archive:
            matches = [item for item in archive.getmembers() if PurePosixPath(item.name).name == entry['executable']]
            if len(matches) != 1 or not matches[0].isfile() or matches[0].size > MAX_BINARY:
                raise ValueError('Expected exactly one regular release-tool executable')
            name = matches[0].name
            with archive.extractfile(matches[0]) as binary:
                payload = binary.read(MAX_BINARY + 1)
    if PurePosixPath(name).is_absolute() or '..' in PurePosixPath(name).parts or len(payload) > MAX_BINARY or not payload:
        raise ValueError('Unsafe release-tool archive entry')
    return payload


def prepare_tool(tool, entry, directory, fetch=download):
    if tool not in VERSIONS or entry['version'] != VERSIONS[tool] or not entry['url'].startswith(ORIGINS[tool]) or not re.fullmatch(r'[0-9a-f]{64}', entry['sha256']):
        raise ValueError('Unexpected release-tool origin or version pin')
    if entry['executable'] not in (tool, tool + '.exe'):
        raise ValueError('Unexpected executable name')
    binary = directory / entry['executable']
    binary.write_bytes(executable_bytes(fetch(entry['url']), entry))
    binary.chmod(0o755)
    version = subprocess.check_output([str(binary), '--version'], text=True, timeout=20).strip()
    expected = ('trunk ' if tool == 'trunk' else 'tauri-cli ') + VERSIONS[tool]
    if version != expected:
        raise ValueError(f'Unexpected {tool} version')
    print(version)
    return binary


def install(destination, selected=None):
    manifest = json.loads(Path(__file__).with_name('release_tools.json').read_text(encoding='utf-8'))
    selected = selected or host_platform()
    if selected not in manifest['platforms']:
        raise ValueError('No pinned release tools for this host platform')
    destination = destination.resolve()
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='notdot-release-tools-', dir=destination.parent) as temporary:
        staged = [prepare_tool(tool, manifest['platforms'][selected][tool], Path(temporary)) for tool in VERSIONS]
        destination.mkdir(exist_ok=True)
        for binary in staged:
            shutil.copy2(binary, destination / binary.name)
    if path_file := os.environ.get('GITHUB_PATH'):
        with open(path_file, 'a', encoding='utf-8') as output:
            output.write(str(destination) + '\n')
    print('Verified prebuilt release tools installed.')


def verify_path(destination):
    destination = destination.resolve()
    suffix = '.exe' if platform.system() == 'Windows' else ''
    for tool in VERSIONS:
        found = shutil.which(tool)
        if not found or Path(found).resolve() != destination / (tool + suffix):
            raise ValueError('The verified release-tool directory must precede cached tools')
    for command, expected in [(['trunk', '--version'], 'trunk 0.22.1'),
                              (['cargo', 'tauri', '--version'], 'tauri-cli 2.12.4')]:
        if subprocess.check_output(command, text=True, timeout=20).strip() != expected:
            raise ValueError('Unexpected release tool selected through PATH')
    print('PATH selects the verified pinned release tools.')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output-dir', type=Path)
    parser.add_argument('--verify-path', action='store_true')
    args = parser.parse_args()
    destination = args.output_dir or Path(os.environ['RUNNER_TEMP']) / 'notdot-release-tools'
    if args.verify_path:
        verify_path(destination)
    else:
        install(destination)
