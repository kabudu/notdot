#!/usr/bin/env python3
"""Prepare hash-bound source inputs for a Tauri AppImage; unknown launchers fail.

This retrieves official release metadata and original source archives. It never
executes a downloaded launcher or modifies the AppImage. Run on its builder.
"""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import tarfile
import urllib.request

from collect_linux_distribution import sha


def fetch(url):
    with urllib.request.urlopen(url, timeout=60) as response:
        payload = response.read(256 * 1024 * 1024 + 1)
    if len(payload) > 256 * 1024 * 1024:
        raise ValueError('download exceeds source input bound')
    return payload


def sections(blob):
    if blob[:6] != b'\x7fELF\x02\x01':
        raise ValueError('unsupported launcher ELF')
    offset = struct.unpack_from('<Q', blob, 40)[0]
    size, count, names_index = struct.unpack_from('<HHH', blob, 58)
    headers = [struct.unpack_from('<IIQQQQIIQQ', blob, offset + i * size) for i in range(count)]
    h = headers[names_index]
    names = blob[h[4]:h[4] + h[5]]
    return {names[h[0]:].split(b'\0', 1)[0].decode('ascii'): (h[4], h[5]) for h in headers}


def runtime_equivalent(actual, upstream):
    if len(actual) != len(upstream):
        return False
    # appimagetool writes only these defined runtime metadata slots.
    left, right = bytearray(actual), bytearray(upstream)
    for name in ('.digest_md5', '.upd_info', '.sha256_sig', '.sig_key'):
        a, b = sections(actual).get(name), sections(upstream).get(name)
        if a != b:
            return False
        if a:
            offset, size = a
            left[offset:offset + size] = right[offset:offset + size] = bytes(size)
    return left == right


def prepare_component(root, repo, asset_name, actual, runtime=False):
    release = json.loads(fetch('https://api.github.com/repos/' + repo + '/releases/tags/continuous'))
    revision = release['target_commitish']
    if len(revision) != 40 or any(c not in '0123456789abcdef' for c in revision):
        raise ValueError('upstream release does not identify an immutable source revision')
    assets = [a for a in release['assets'] if a['name'] == asset_name]
    if len(assets) != 1:
        raise ValueError('upstream launcher asset unavailable')
    official = fetch(assets[0]['browser_download_url'])
    expected = assets[0].get('digest')
    if expected and expected != 'sha256:' + hashlib.sha256(official).hexdigest():
        raise ValueError('official release digest mismatch')
    if not (runtime_equivalent(actual, official) if runtime else actual == official):
        raise ValueError('packaged launcher does not match recorded official release: ' + asset_name)
    component = root / asset_name
    component.mkdir()
    (component / 'release.json').write_text(json.dumps(release, indent=2) + '\n')
    (component / 'source.tar.gz').write_bytes(fetch('https://codeload.github.com/' + repo + '/tar.gz/' + revision))
    notice = fetch('https://raw.githubusercontent.com/' + repo + '/' + revision + '/LICENSE')
    (component / 'NOTICE').write_bytes(notice)
    if runtime:
        # Exact static dependency versions recovered from the official x64 build
        # log linked by this release. LGPL relinking uses source + build recipes;
        # no bit-identical compiler reproduction is required.
        if revision != 'fedfad51acc2860b2916a1eef32ba4a500ff31ef':
            raise ValueError('new runtime release needs reviewed static dependency notices')
        notices = [
            ('musl-1.2.5-COPYRIGHT', 'https://git.musl-libc.org/cgit/musl/plain/COPYRIGHT?h=v1.2.5', 'f9bc4423732350eb0b3f7ed7e91d530298476f8fec0c6c427a1c04ade22655af'),
            ('mimalloc-2.1.7-LICENSE', 'https://raw.githubusercontent.com/microsoft/mimalloc/v2.1.7/LICENSE', '19c99805e7a44a34b297a75d1edea9985e300066dfc024d5c99d4236d4573b5d'),
            ('zstd-1.5.6-LICENSE', 'https://raw.githubusercontent.com/facebook/zstd/v1.5.6/LICENSE', '7055266497633c9025b777c78eb7235af13922117480ed5c674677adc381c9d8'),
            ('zstd-1.5.6-COPYING', 'https://raw.githubusercontent.com/facebook/zstd/v1.5.6/COPYING', 'f9c375a1be4a41f7b70301dd83c91cb89e41567478859b77eef375a52d782505'),
            ('zlib-1.3.2-zlib.h', 'https://raw.githubusercontent.com/madler/zlib/v1.3.2/zlib.h', '818667d6ab6a37fe7469cb06a7f0cb2c2cb2f2c948a03e5accf1a4a74bf3020a')]
        for name, url, digest in notices:
            payload = fetch(url)
            if hashlib.sha256(payload).hexdigest() != digest:
                raise ValueError('runtime original notice checksum mismatch: ' + name)
            (component / name).write_bytes(payload)
        (component / 'static-dependency-notices.json').write_text(json.dumps(
            {'build_log': 'https://github.com/AppImage/type2-runtime/actions/runs/39320578745',
             'notices': [{'file': name, 'source': url, 'sha256': digest} for name, url, digest in notices]}, indent=2) + '\n')
        # These are hash-pinned by the recorded runtime's own dependency recipe.
        inputs = [
            ('fuse-3.15.0.tar.xz', 'https://github.com/libfuse/libfuse/releases/download/fuse-3.15.0/fuse-3.15.0.tar.xz',
             '70589cfd5e1cff7ccd6ac91c86c01be340b227285c5e200baa284e401eea2ca0'),
            ('squashfuse-0.5.2.tar.gz', 'https://github.com/vasi/squashfuse/archive/0.5.2.tar.gz',
             'db0238c5981dabbd80ee09ae15387f390091668ca060a7bc38047912491443d3')]
        with tarfile.open(component / 'source.tar.gz') as archive:
            recipe_member = next(m for m in archive.getmembers() if m.name.endswith('/scripts/common/install-dependencies.sh'))
            recipe = archive.extractfile(recipe_member).read().decode()
        for name, url, digest in inputs:
            if digest not in recipe:
                raise ValueError('runtime recipe changed; review exact dependency sources')
            payload = fetch(url)
            if hashlib.sha256(payload).hexdigest() != digest:
                raise ValueError('runtime dependency source checksum mismatch')
            (component / name).write_bytes(payload)
    archive_path = root / (asset_name + '-corresponding-source.tar.gz')
    with tarfile.open(archive_path, 'w:gz') as archive:
        archive.add(component, arcname=asset_name)
    return {'notice': str((component / 'NOTICE').resolve()), 'source': str(archive_path.resolve())}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--appdir', type=Path, required=True)
    parser.add_argument('--appimage', type=Path, required=True)
    parser.add_argument('--squashfs-offset', type=int, required=True)
    parser.add_argument('--project', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        parser.error('output must not exist')
    args.output.mkdir(parents=True)
    covered = {}
    app = args.appdir / 'usr/bin/notdot'
    source = args.output / 'notdot-source.tar.gz'
    subprocess.run(['git', '-c', 'safe.directory=' + str(args.project.resolve()), '-C', str(args.project), 'archive', '--format=tar.gz', '--prefix=notdot-source/',
                    '-o', str(source.resolve()), 'HEAD'], check=True)
    covered[sha(app)] = {'notice': str((args.project / 'NOTICE').resolve()), 'source': str(source.resolve())}
    apprun = args.appdir / 'AppRun.wrapped'
    if apprun.is_file():
        covered[sha(apprun)] = prepare_component(args.output, 'AppImage/AppImageKit', 'AppRun-x86_64', apprun.read_bytes())
    with args.appimage.open('rb') as stream:
        runtime = stream.read(args.squashfs_offset)
    covered[hashlib.sha256(runtime).hexdigest()] = prepare_component(
        args.output, 'AppImage/type2-runtime', 'runtime-x86_64', runtime, runtime=True)
    (args.output / 'independently-covered.json').write_text(json.dumps(covered, indent=2) + '\n')
    print(args.output / 'independently-covered.json')


if __name__ == '__main__':
    main()
