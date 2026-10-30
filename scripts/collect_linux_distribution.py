#!/usr/bin/env python3
"""Collect exact Debian sources/notices for an AppDir on its original builder.

Run before discarding the build environment. No licence inference or written
source offer is generated. Unknown/modified ELF files fail closed unless their
exact hash is explicitly identified as a separately covered project artifact.
"""
import argparse
import hashlib
import json
import os
import re
from pathlib import Path
import shutil
import subprocess
import struct
import tarfile
import urllib.request


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run(*args, cwd=None):
    return subprocess.check_output(args, cwd=cwd, text=True).strip()


def load_sections(path):
    """Hash allocated ELF64-LE sections, excluding linuxdeploy's RPATH tables."""
    data = path.read_bytes()
    if data[:6] != b'\x7fELF\x02\x01':
        return None
    offset = struct.unpack_from('<Q', data, 40)[0]
    size, count, names_index = struct.unpack_from('<HHH', data, 58)
    headers = [struct.unpack_from('<IIQQQQIIQQ', data, offset + i * size) for i in range(count)]
    names_header = headers[names_index]
    names = data[names_header[4]:names_header[4] + names_header[5]]
    result = {}
    for item in headers:
        name = names[item[0]:].split(b'\0', 1)[0].decode('ascii')
        if item[2] & 2 and name not in ('.dynamic', '.dynstr'):
            payload = b'' if item[1] == 8 else data[item[4]:item[4] + item[5]]
            if item[1] == 11:  # patchelf may move dynstr offsets and section indices
                strings_header = headers[item[6]]
                strings = data[strings_header[4]:strings_header[4] + strings_header[5]]
                symbols = []
                for offset in range(0, len(payload), 24):
                    n, info, other, section, value, length = struct.unpack_from('<IBBHQQ', payload, offset)
                    section_name = section
                    if 0 < section < count:
                        section_name = names[headers[section][0]:].split(b'\0', 1)[0].decode('ascii')
                    symbols.append([strings[n:].split(b'\0', 1)[0].decode('utf-8'), info, other, section_name, value, length])
                payload = json.dumps(symbols, sort_keys=True).encode()
            result[name] = [item[1], item[2], item[5], hashlib.sha256(payload).hexdigest()]
    return result if '.text' in result and '.note.gnu.build-id' in result else None


def collect(appdir, output, covered, project):
    if output.exists():
        raise ValueError('output must not exist; refuse to mix distribution records')
    if os.geteuid() != 0:
        raise ValueError('run in an isolated Debian build container as root')
    output.mkdir(parents=True)
    packages = {}
    for line in run('dpkg-query', '-W', '-f=${binary:Package}\t${Version}\t${source:Package}\t${source:Version}\n').splitlines():
        fields = line.split('\t')
        if len(fields) == 4:
            packages[fields[0]] = dict(zip(('binary', 'version', 'source', 'source_version'), fields))
    # Index installed paths once; basename narrows the hash comparison.
    installed = {}
    for package in packages:
        for value in run('dpkg-query', '-L', package).splitlines():
            path = Path(value)
            if path.is_file() and not path.is_symlink():
                installed.setdefault(path.name, []).append((package, path))
    binaries, selected, unknown = [], set(), []
    for path in sorted(appdir.rglob('*')):
        if not path.is_file() or path.is_symlink():
            continue
        with path.open('rb') as stream:
            if stream.read(4) != b'\x7fELF':
                continue
        digest = sha(path)
        entry = {'path': str(path.relative_to(appdir)), 'sha256': digest}
        candidates = installed.get(path.name, [])
        matches = [(pkg, host) for pkg, host in candidates if sha(host) == digest]
        patched = False
        if not matches and digest not in covered:
            sections = load_sections(path)
            matches = [(pkg, host) for pkg, host in candidates if sections and load_sections(host) == sections]
            patched = bool(matches)
        if digest in covered:
            entry['separate_coverage'] = digest
            record = covered[digest]
            for kind in ('notice', 'source'):
                origin = Path(record[kind]).resolve()
                if not origin.is_file() or origin.is_symlink():
                    raise ValueError('missing independently covered ' + kind + ': ' + digest)
                destination = output / 'independent-inputs' / digest / (kind + '-' + origin.name)
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(origin, destination)
        elif matches:
            package = matches[0][0]
            entry['package'] = package
            entry['installed_sha256'] = sha(matches[0][1])
            entry['comparison'] = 'allocated-sections-and-build-id-except-dynamic-tables' if patched else 'byte-identical'
            selected.add(package)
        else:
            unknown.append(entry)
        binaries.append(entry)
    if unknown:
        (output / 'unmatched.json').write_text(json.dumps(unknown, indent=2) + '\n')
        raise ValueError('unmatched ELF files: see unmatched.json; do not publish this partial output')
    # Exact application source plus its build recipes; not a promise to supply later.
    if run('git', '-C', str(project), 'status', '--porcelain', '--untracked-files=no'):
        raise ValueError('project tracked files must be committed before source assembly')
    revision = run('git', '-C', str(project), 'rev-parse', 'HEAD')
    subprocess.run(['git', '-C', str(project), 'archive', '--format=tar.gz',
                    '--prefix=notdot-source/', '-o', str(output / 'notdot-source.tar.gz'), revision], check=True)
    # Vendor the actual locked Rust source graph; URLs alone are not the source companion.
    vendor_config = subprocess.check_output(['cargo', 'vendor', '--locked', '--versioned-dirs',
                                            str(output / 'rust-sources')], cwd=project, text=True, timeout=600)
    (output / 'cargo-vendor-config.toml').write_text(vendor_config.replace(str(output / 'rust-sources'), './rust-sources'))
    native_manifest = json.loads((project / 'src-tauri/resources/notices/manifest.json').read_text())
    native_root = output / 'static-native-sources'
    native_root.mkdir()
    for component in native_manifest['native_source_components']:
        digest = component['archive_sha256']
        with urllib.request.urlopen(component['archive_url'], timeout=60) as response:
            payload = response.read(256 * 1024 * 1024 + 1)
        if len(payload) > 256 * 1024 * 1024 or hashlib.sha256(payload).hexdigest() != digest:
            raise ValueError('native source hash/size mismatch: ' + component['component'])
        (native_root / (digest + '.tar.gz')).write_bytes(payload)
    notice_root = output / 'notices'
    notice_root.mkdir()
    shutil.copytree('/usr/share/common-licenses', notice_root / 'common-licenses', symlinks=False)
    sources = {}
    for package in sorted(selected):
        meta = packages[package]
        name = package.split(':')[0]
        copyright_file = Path('/usr/share/doc') / name / 'copyright'
        if not copyright_file.is_file():
            raise ValueError('missing original copyright file for ' + package)
        dest = notice_root / (name + '-copyright')
        shutil.copyfile(copyright_file, dest)
        source, version = meta['source'], meta['source_version']
        if not source or not version:
            raise ValueError('missing source identity for ' + package)
        key = source + '=' + version
        if key not in sources:
            directory = output / 'sources' / source
            directory.mkdir(parents=True, exist_ok=True)
            subprocess.run(['apt-get', 'source', '--download-only', '--only-source', key], cwd=directory, check=True, timeout=600)
            dsc = list(directory.glob('*.dsc'))
            if len(dsc) != 1:
                raise ValueError('expected one source control file for ' + key)
            # dpkg-source verifies the .dsc checksums before extracting the full
            # Debian source tree (upstream source plus patches/build recipes).
            subprocess.run(['dpkg-source', '--no-check', '-x', dsc[0].name, 'verification-tree'], cwd=directory, check=True, timeout=600)
            shutil.rmtree(directory / 'verification-tree')
            sources[key] = {'package': source, 'version': version}
    record = {'schema_version': 1, 'notdot_revision': revision, 'binaries': binaries,
              'binary_packages': [packages[p] for p in sorted(selected)],
              'sources': [sources[k] for k in sorted(sources)],
              'files': {str(p.relative_to(output)): sha(p) for p in sorted(output.rglob('*')) if p.is_file()}}
    (output / 'manifest.json').write_text(json.dumps(record, indent=2, sort_keys=True) + '\n')
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('appdir', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--separately-covered', type=Path, required=True,
                        help='reviewed JSON: exact ELF sha256 -> {notice: file, source: archive}; both copied into companion')
    parser.add_argument('--project', type=Path, required=True, help='clean checkout of exact packaged notdot revision')
    args = parser.parse_args()
    covered = json.loads(args.separately_covered.read_text())
    if not isinstance(covered, dict) or any(not re.fullmatch(r'[0-9a-f]{64}', k) or not isinstance(v, dict) or
            set(v) != {'notice', 'source'} or any(not isinstance(p, str) or not p for p in v.values())
            for k, v in covered.items()):
        parser.error('invalid separate-coverage manifest')
    record = collect(args.appdir.resolve(), args.output.resolve(), covered, args.project.resolve())
    archive = args.output.with_suffix('.tar.gz')
    with tarfile.open(archive, 'w:gz') as stream:
        stream.add(args.output, arcname=args.output.name)
    print(json.dumps({'elf_files': len(record['binaries']), 'source_packages': len(record['sources']),
                      'archive': str(archive), 'sha256': sha(archive)}))


if __name__ == '__main__':
    main()
