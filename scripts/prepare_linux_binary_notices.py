#!/usr/bin/env python3
"""Retain original build-environment package notices before AppImage bundling.

This copies public package copyright files only. The source collector separately
verifies the exact subset used by the image. No package software is copied here.
"""
import hashlib
import json
from pathlib import Path
import shutil
import sys

ROOT = Path(__file__).resolve().parents[1]
MARKER = 'notdot-generated-distro-copyright-v1'


def prepare(output, documents):
    if output.is_symlink():
        raise ValueError('refuse a symlinked generated notice directory')
    if output.exists():
        marker = output / '.notdot-generated'
        if not marker.is_file() or marker.read_text() != MARKER:
            raise ValueError('refuse to replace an unmarked directory')
        shutil.rmtree(output)
    output.mkdir(parents=True)
    (output / '.notdot-generated').write_text(MARKER)
    files = {}
    for directory in sorted(documents.iterdir()):
        copyright_file = directory / 'copyright'
        if not copyright_file.is_file():
            continue
        target = output / directory.name / 'copyright'
        target.parent.mkdir()
        shutil.copyfile(copyright_file, target)
        files[str(target.relative_to(output))] = hashlib.sha256(target.read_bytes()).hexdigest()
    if not files:
        raise ValueError('no distro package copyright files found')
    (output / 'manifest.json').write_text(json.dumps({'schema_version': 1,
        'scope': 'Conservative build-environment original notices; selected redistributed packages are identified in the corresponding-source manifest.',
        'files': files}, indent=2, sort_keys=True) + '\n')
    return len(files)


if __name__ == '__main__':
    if sys.platform != 'linux':
        raise SystemExit('Linux build-environment notice preparation requires Linux')
    count = prepare(ROOT / '.local/linux-binary-notices', Path('/usr/share/doc'))
    print(f'Prepared {count} original distro copyright files for AppImage packaging')
