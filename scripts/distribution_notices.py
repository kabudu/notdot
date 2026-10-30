#!/usr/bin/env python3
"""Generate/check the binary notice index from retained, hash-pinned inputs.

Offline and stdlib-only. Original texts are reviewable inputs; this script does
not infer copyright holders, select a licence on behalf of upstream, or fetch
and execute anything. A changed lockfile requires an updated notice inventory.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
BUNDLE = ROOT / 'src-tauri/resources/notices'


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def terms(expression):
    return set(re.findall(r'[A-Za-z0-9][A-Za-z0-9.+-]*', expression)) - {'AND', 'OR', 'WITH'}


def validate(bundle, lock):
    manifest = json.loads((bundle / 'manifest.json').read_text())
    if manifest.get('schema_version') != 1:
        raise ValueError('unsupported notice manifest schema')
    if digest(lock) != manifest['cargo_lock_sha256']:
        raise ValueError('Cargo.lock changed; update the retained notice inventory before packaging')
    texts = manifest['texts']
    for sha, item in texts.items():
        if not re.fullmatch(r'[0-9a-f]{64}', sha) or item['file'] != 'texts/' + sha + '.txt':
            raise ValueError('unsafe notice text path')
        p = bundle / item['file']
        if p.is_symlink() or p.parent.is_symlink() or not p.is_file():
            raise ValueError('missing or symlinked notice: ' + sha)
        if digest(p) != sha or item['sha256'] != sha or p.stat().st_size != item['bytes']:
            raise ValueError('notice checksum mismatch: ' + sha)
        p.read_text(encoding='utf-8')
        if not item['origins']:
            raise ValueError('notice provenance missing: ' + sha)
    for name in ('LICENSE', 'NOTICE'):
        if digest(ROOT / name) not in texts:
            raise ValueError('project ' + name + ' changed; update its retained notice')
    ids = set()
    for p in manifest['packages']:
        if p['id'] in ids:
            raise ValueError('duplicate package: ' + p['id'])
        ids.add(p['id'])
        if not p['original_notices'] or terms(p['license_expression']) != set(p['standard_terms']):
            raise ValueError('incomplete licence mapping: ' + p['name'])
        for sha in p['original_notices'] + list(p['standard_terms'].values()):
            if sha not in texts:
                raise ValueError('unresolved notice reference: ' + p['name'])
    for scope in manifest['targets'].values():
        if not set(scope['packages']) <= ids:
            raise ValueError('target graph references an unknown package')
    for component in manifest['native_source_components']:
        if not component['original_notices'] or not re.fullmatch(r'[0-9a-f]{64}', component['archive_sha256']):
            raise ValueError('native source provenance missing')
        if not set(component['original_notices']) <= set(texts):
            raise ValueError('native notice missing')
        if not set(component.get('standard_terms', {}).values()) <= set(texts):
            raise ValueError('native standard terms missing')
    std = manifest['rust_standard_library']
    if std['file'] != 'rust-1.97.0-standard-library.html' or digest(bundle / std['file']) != std['sha256']:
        raise ValueError('Rust standard-library notice mismatch')
    if not set(manifest['already_retained_model_native_notices']) <= set(texts):
        raise ValueError('retained model notice missing')
    return manifest


def link(sha):
    return '[text](texts/' + sha + '.txt)'


def render(manifest):
    lines = ['# Licences and notices', '',
             'notdot’s own source and original artwork are licensed under Apache-2.0. '
             'The components below retain their own licences; they are not relicensed by notdot.', '',
             'Original copyright notices and full standard licence terms are retained below. '
             'Every declared licence choice is included; the standard texts may contain their '
             'original template placeholders. Those placeholders are not new copyright claims. '
             'Upstream declarations and copyright lines remain in the original notice files.', '',
             'The index uses a conservative union of the locked native-app and WASM dependency graphs. '
             'Presence here does not claim every source file was linked on every platform. '
             'The optional Windows helper belongs to the included shared wake-runtime package.', '',
             'Source URLs, published revisions, archive checksums and text checksums are in '
             '[manifest.json](manifest.json). Package source archives are linked there, including '
             'the published MPL-2.0 selectors source. No dependency is relicensed by notdot.', '',
             '[Rust 1.97.0 standard-library notices](rust-1.97.0-standard-library.html)', '',
             '## Rust packages', '', '| Package | Declared licence | Original notices | Full standard terms |',
             '| --- | --- | --- | --- |']
    for p in sorted(manifest['packages'], key=lambda x: (x['name'], x['version'], x['id'])):
        originals = ', '.join(link(s) for s in p['original_notices'])
        standard = ', '.join('[' + id + '](texts/' + sha + '.txt)' for id, sha in sorted(p['standard_terms'].items()))
        lines.append('| ' + p['name'] + ' ' + p['version'] + ' | ' + p['license_expression'] + ' | ' + originals + ' | ' + standard + ' |')
    lines += ['', '## Additional static native source notices', '']
    for p in manifest['native_source_components']:
        lines.append('- ' + p['component'] + ': ' + ', '.join(link(s) for s in p['original_notices']))
    lines += ['', '## Models and previously retained native notices', '']
    lines += ['- ' + link(s) for s in manifest['already_retained_model_native_notices']]
    lines += ['', '## Separate public-distribution checks', '']
    lines += ['- ' + item for item in manifest['remaining_distribution_gates']]
    return '\n'.join(lines) + '\n'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--write', action='store_true', help='regenerate INDEX.md from pinned manifest/text inputs')
    args = parser.parse_args()
    try:
        manifest = validate(BUNDLE, ROOT / 'Cargo.lock')
        expected = render(manifest)
        index = BUNDLE / 'INDEX.md'
        if args.write:
            index.write_text(expected, encoding='utf-8')
        elif not index.is_file() or index.read_text(encoding='utf-8') != expected:
            raise ValueError('notice index is stale; run scripts/distribution_notices.py --write')
        print('Notices checked: {} Rust packages, {} retained/standard texts, {} static native source components.'.format(
            len(manifest['packages']), len(manifest['texts']), len(manifest['native_source_components'])))
    except (OSError, ValueError, KeyError, TypeError) as error:
        print('Notice validation failed: ' + str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
