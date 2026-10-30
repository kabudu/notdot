#!/usr/bin/env python3
"""Collect exact Debian sources/notices for an AppDir on its original builder.

Run before discarding the build environment. No licence inference or written
source offer is generated. Unknown/modified ELF files fail closed unless their
exact hash is explicitly identified as a separately covered project artifact.
"""
import argparse
import hashlib
import json
import re
from pathlib import Path
import shutil
import subprocess
import struct
import tarfile
import urllib.request
import zipfile
import tempfile


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run(*args, cwd=None):
    return subprocess.check_output(args, cwd=cwd, text=True).strip()


def load_sections(path, relocate_usr=False):
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
            if relocate_usr and name == '.rodata':
                payload = payload.replace(b'/usr', b'././')
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


def copy_independent(record, output, digest):
    for kind in ('notice', 'source'):
        origin = Path(record[kind])
        if origin.is_symlink() or not origin.is_file():
            raise ValueError('independent input must be a regular file: ' + kind)
        if not 0 < origin.stat().st_size <= (256 * 1024 * 1024 if kind == 'source' else 8 * 1024 * 1024):
            raise ValueError('empty or oversized independent input: ' + kind)
        if kind == 'source':
            if zipfile.is_zipfile(origin):
                with zipfile.ZipFile(origin) as archive:
                    members = archive.infolist()
                    if not members or not any(x.file_size for x in members):
                        raise ValueError('empty source archive')
            elif tarfile.is_tarfile(origin):
                with tarfile.open(origin) as archive:
                    members = archive.getmembers()
                    if not members or not any(x.isfile() and x.size for x in members):
                        raise ValueError('empty source archive')
            else:
                raise ValueError('independent source must be a nonempty tar/zip archive')
        target = output / 'independent-inputs' / digest / (kind + '-' + origin.name)
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(origin, target)


def check_symlinks(appdir):
    root = appdir.resolve()
    for path in appdir.rglob('*'):
        if path.is_symlink():
            try:
                target = path.resolve(strict=True)
                target.relative_to(root)
            except (FileNotFoundError, RuntimeError, ValueError) as error:
                raise ValueError('escaping, cyclic or dangling AppDir symlink: ' + str(path)) from error
            if not target.is_file() and not target.is_dir():
                raise ValueError('unsupported AppDir symlink target')


def collect(appdir, output, covered, project, appimage, offset, expected_app_sha256):
    if not appdir.is_dir() or not (appdir / 'usr/bin/notdot').is_file():
        raise ValueError('AppDir must exist and contain usr/bin/notdot')
    check_symlinks(appdir)
    common = json.loads((project / 'third_party/linux-common-licenses.json').read_text())['files']
    for name, digest in common.items():
        if '/' in name or name in ('.', '..'):
            raise ValueError('unsafe common-licence name')
        if sha(project / 'third_party/linux-common-licenses' / name) != digest or \
                sha(appdir / 'usr/share/common-licenses' / name) != digest:
            raise ValueError('AppImage missing or changed full licence text: ' + name)
    if sha(appdir / 'usr/bin/notdot') != expected_app_sha256:
        raise ValueError('notdot executable does not match build-record hash')
    if not appimage.is_file() or offset <= 0 or appimage.stat().st_size <= offset:
        raise ValueError('invalid final AppImage or SquashFS offset')
    with appimage.open('rb') as stream:
        runtime_bytes = stream.read(offset)
        if stream.read(4) != b'hsqs':
            raise ValueError('offset is not the final AppImage SquashFS start')
    runtime_sha = hashlib.sha256(runtime_bytes).hexdigest()
    if runtime_sha not in covered:
        raise ValueError('AppImage runtime prefix lacks exact source/notice coverage: ' + runtime_sha)
    if output.exists():
        raise ValueError('output must not exist; refuse to mix distribution records')
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
            if path.is_file():
                installed.setdefault(path.name, []).append((package, path))
    binaries, selected, unknown = [], set(), []
    for path in sorted(appdir.rglob('*')):
        if not path.is_file() or path.is_symlink():
            continue
        with path.open('rb') as stream:
            is_elf = stream.read(4) == b'\x7fELF'
        relative = str(path.relative_to(appdir))
        platform_data = relative.startswith('usr/share/') and not relative.startswith(
            ('usr/share/doc/', 'usr/share/icons/', 'usr/share/applications/', 'usr/share/common-licenses/'))
        if not is_elf and not platform_data:
            continue
        digest = sha(path)
        entry = {'path': str(path.relative_to(appdir)), 'sha256': digest}
        candidates = installed.get(path.name, [])
        matches = [(pkg, host) for pkg, host in candidates if sha(host) == digest]
        if not is_elf and path.name == 'gschemas.compiled':
            with tempfile.TemporaryDirectory() as temporary:
                target = Path(temporary)
                for source in path.parent.iterdir():
                    if source.is_file() and source.name != 'gschemas.compiled':
                        shutil.copyfile(source, target / source.name)
                subprocess.run(['glib-compile-schemas', '--strict', str(target)], check=True, timeout=30)
                if sha(target / 'gschemas.compiled') != digest:
                    raise ValueError('compiled schemas differ from shipped XML sources')
            entry['generated_from'] = 'shipped schema XML/override files; glib-compile-schemas --strict'
            binaries.append(entry)
            continue
        patched = False
        if not matches and digest not in covered:
            sections = load_sections(path)
            matches = [(pkg, host) for pkg, host in candidates if sections and load_sections(host) == sections]
            patched = bool(matches)
        relocated = False
        if not matches and path.name == 'libwebkit2gtk-4.1.so.0':
            actual_sections = load_sections(path)
            matches = [(pkg, host) for pkg, host in candidates
                       if actual_sections and load_sections(host, relocate_usr=True) == actual_sections]
            relocated = bool(matches)
        if digest in covered:
            entry['separate_coverage'] = digest
            copy_independent(covered[digest], output, digest)
        elif matches:
            package = matches[0][0]
            entry['package'] = package
            entry['installed_sha256'] = sha(matches[0][1])
            entry['comparison'] = ('tauri-webkit-usr-relocation' if relocated else
                                   'allocated-sections-and-build-id-except-dynamic-tables' if patched else 'byte-identical')
            selected.add(package)
        else:
            unknown.append(entry)
        binaries.append(entry)
    if not any(b['path'] == 'usr/bin/notdot' for b in binaries):
        raise ValueError('empty ELF inventory')
    if unknown:
        (output / 'unmatched.json').write_text(json.dumps(unknown, indent=2) + '\n')
        raise ValueError('unmatched ELF files: see unmatched.json; do not publish this partial output')
    copy_independent(covered[runtime_sha], output, runtime_sha)
    # Exact application source plus its build recipes; not a promise to supply later.
    if run('git', '-c', 'safe.directory=' + str(project), '-C', str(project), 'status', '--porcelain', '--untracked-files=no'):
        raise ValueError('project tracked files must be committed before source assembly')
    revision = run('git', '-c', 'safe.directory=' + str(project), '-C', str(project), 'rev-parse', 'HEAD')
    subprocess.run(['git', '-c', 'safe.directory=' + str(project), '-C', str(project), 'archive', '--format=tar.gz',
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
    if any(b.get('comparison') == 'tauri-webkit-usr-relocation' for b in binaries):
        modifications = output / 'platform-modifications'
        modifications.mkdir()
        (modifications / 'relocate-webkit.py').write_text(
            '# Tauri AppImage WebKit relocation, before normal RPATH deployment.\n'
            'from pathlib import Path\nimport sys\n'
            'p = Path(sys.argv[1])\np.write_bytes(p.read_bytes().replace(b"/usr", b"././"))\n')
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
            subprocess.run(['dpkg-source', '-x', dsc[0].name, 'verification-tree'], cwd=directory, check=True, timeout=600)
            shutil.rmtree(directory / 'verification-tree')
            sources[key] = {'package': source, 'version': version}
    record = {'schema_version': 1, 'notdot_revision': revision,
              'appimage_sha256': sha(appimage), 'appimage_bytes': appimage.stat().st_size,
              'runtime_prefix_sha256': runtime_sha, 'squashfs_offset': offset,
              'notdot_binary_sha256': expected_app_sha256, 'binaries': binaries,
              'binary_packages': [packages[p] for p in sorted(selected)],
              'sources': [sources[k] for k in sorted(sources)],
              'data_source_policy': 'usr/share XML/data mapped by exact installed-file hash; compiled schemas regenerated',
              'files': {str(p.relative_to(output)): sha(p) for p in sorted(output.rglob('*')) if p.is_file()}}
    (output / 'manifest.json').write_text(json.dumps(record, indent=2, sort_keys=True) + '\n')
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('appdir', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--separately-covered', type=Path, required=True,
                        help='reviewed JSON: exact ELF sha256 -> {notice: file, source: archive}; both copied into companion')
    parser.add_argument('--appimage', type=Path, required=True, help='exact final AppImage paired with this source companion')
    parser.add_argument('--squashfs-offset', type=int, required=True)
    parser.add_argument('--app-binary-sha256', required=True, help='expected usr/bin/notdot hash from immutable build record')
    parser.add_argument('--project', type=Path, required=True, help='clean checkout of exact packaged notdot revision')
    args = parser.parse_args()
    covered = json.loads(args.separately_covered.read_text())
    if not isinstance(covered, dict) or any(not re.fullmatch(r'[0-9a-f]{64}', k) or not isinstance(v, dict) or
            set(v) != {'notice', 'source'} or any(not isinstance(p, str) or not p for p in v.values())
            for k, v in covered.items()):
        parser.error('invalid separate-coverage manifest')
    record = collect(args.appdir.resolve(), args.output.resolve(), covered, args.project.resolve(),
                     args.appimage.resolve(), args.squashfs_offset, args.app_binary_sha256)
    archive = args.output.with_suffix('.tar.gz')
    with tarfile.open(archive, 'w:gz') as stream:
        stream.add(args.output, arcname=args.output.name)
    print(json.dumps({'elf_files': len(record['binaries']), 'source_packages': len(record['sources']),
                      'archive': str(archive), 'sha256': sha(archive)}))


if __name__ == '__main__':
    main()
