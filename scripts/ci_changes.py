#!/usr/bin/env python3
"""Fail-closed content classification and the single required CI result gate."""
import argparse
import hashlib
import json
import os
from pathlib import PurePosixPath
import re
import subprocess
from urllib.request import Request, urlopen

ROOT_CONTENT = {'README.md', 'CHANGELOG.md', 'CONTRIBUTING.md', 'SECURITY.md', 'CODE_OF_CONDUCT.md', 'ROADMAP.md'}
DOC_SUFFIXES = {'.md', '.html', '.css', '.js'}
SITE_SUFFIXES = DOC_SUFFIXES | {'.svg', '.png', '.jpg', '.jpeg', '.gif', '.webp', '.ico', '.mp4', '.webm', '.vtt', '.woff', '.woff2', '.txt'}
PLATFORMS = ('release-tools', 'windows-native', 'windows-metadata', 'macos', 'linux-native')
SHA = re.compile(r'[0-9a-f]{40}')
RAW = re.compile(rb':([0-7]{6}) ([0-7]{6}) [0-9a-f]{40} [0-9a-f]{40} ([A-Z])')


def content_path(name):
    path = PurePosixPath(name)
    if str(path) != name or path.is_absolute() or '..' in path.parts or '\\' in name or any(ord(c) < 32 or ord(c) == 127 for c in name):
        return False
    if name in ROOT_CONTENT:
        return True
    if name == 'docs/release-notes.md' or name == 'docs/releases' or name.startswith('docs/releases/'):
        return False  # Release inputs must never enter the content-only path.
    if not name.startswith('docs/'):
        return False
    if name == 'docs/.nojekyll':
        return True
    return path.suffix in (SITE_SUFFIXES if name.startswith('docs/site-assets/') else DOC_SUFFIXES)


def requires_full(raw):
    if not raw:
        return True
    fields = raw.split(b'\0')
    if fields.pop() != b'' or len(fields) % 2:
        raise ValueError('Malformed Git change list')
    for header, encoded in zip(fields[::2], fields[1::2]):
        match = RAW.fullmatch(header)
        if not match:
            raise ValueError('Unknown Git change record')
        old_mode, new_mode, status = match.groups()
        name = encoded.decode('utf-8')
        # Deletions, type changes, renames (--no-renames yields D+A), executable
        # mode changes, symlinks and submodules all require the native suites.
        if status not in (b'A', b'M') or new_mode != b'100644' or old_mode != {b'A': b'000000', b'M': b'100644'}.get(status) or not content_path(name):
            return True
    return False


def classify(event, base, head, force_full=False):
    if force_full or event not in ('pull_request', 'push'):
        return True
    if not SHA.fullmatch(base) or not SHA.fullmatch(head) or int(base, 16) == 0 or int(head, 16) == 0:
        return True
    raw = subprocess.check_output(['git', 'diff', '--raw', '-z', '--no-renames', '--abbrev=40', f'{base}...{head}', '--'])
    return requires_full(raw)


def native_fingerprint():
    """Hash native inputs in the checked-out merge tree, including modes/paths."""
    records = subprocess.check_output(['git', 'ls-tree', '-rz', '--full-tree', 'HEAD']).split(b'\0')
    digest = hashlib.sha256()
    for record in records:
        if not record:
            continue
        header, encoded = record.split(b'\t', 1)
        mode, kind, _ = header.split(b' ')
        if mode == b'100644' and kind == b'blob' and content_path(encoded.decode('utf-8')):
            continue
        digest.update(record + b'\0')
    return digest.hexdigest()


def github_json(path):
    request = Request('https://api.github.com/' + path, headers={
        'Authorization': 'Bearer ' + os.environ['GH_TOKEN'],
        'Accept': 'application/vnd.github+json',
        'X-GitHub-Api-Version': '2022-11-28',
    })
    with urlopen(request, timeout=5) as response:
        raw = response.read(1024 * 1024 + 1)
    if len(raw) > 1024 * 1024:
        raise ValueError('CI metadata exceeded its size limit')
    return json.loads(raw)


def reusable_native_run(fingerprint, head):
    """Reuse only an ancestral, successful run of the identical native policy.

    The artifact name attests the tested merge tree, so changes from the PR's
    base branch invalidate reuse too. Checking the ancestor diff independently
    prevents a changed workflow from supplying a forged fingerprint artifact.
    Missing/expired evidence and API errors always select full validation.
    """
    repo = os.environ.get('GITHUB_REPOSITORY', '')
    if not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', repo) or not SHA.fullmatch(head):
        return None
    try:
        name = 'native-validation-' + fingerprint
        artifacts = github_json(f'repos/{repo}/actions/artifacts?name={name}&per_page=10')['artifacts']
        for artifact in artifacts[:3]:
            previous = artifact['workflow_run']['head_sha']
            if artifact['expired'] or artifact['name'] != name or previous == head or not SHA.fullmatch(previous):
                continue
            if subprocess.run(['git', 'merge-base', '--is-ancestor', previous, head], capture_output=True).returncode:
                continue
            changes = subprocess.check_output(['git', 'diff', '--raw', '-z', '--no-renames', '--abbrev=40', f'{previous}..{head}', '--'])
            if requires_full(changes):
                continue
            run_id = artifact['workflow_run']['id']
            if not isinstance(run_id, int) or isinstance(run_id, bool) or run_id <= 0:
                continue
            run = github_json(f'repos/{repo}/actions/runs/{run_id}')
            if (run['status'] == 'completed' and run['conclusion'] == 'success'
                    and run['event'] in ('pull_request', 'push')
                    and run['path'] == '.github/workflows/ci.yml'
                    and run['head_sha'] == previous
                    and str(run['head_repository']['id']) == os.environ.get('GITHUB_REPOSITORY_ID')):
                return run_id
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
        pass
    return None


def validate_gate(needs):
    changes = needs['changes']
    full = changes['outputs'].get('full')
    if changes['result'] != 'success' or full not in ('true', 'false') or needs['content']['result'] != 'success':
        raise ValueError('Change classification and content checks must succeed')
    expected = 'success' if full == 'true' else 'skipped'
    if any(needs[job]['result'] != expected for job in PLATFORMS):
        raise ValueError(f'Native platform jobs must all be {expected}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=['classify', 'gate', 'fingerprint'])
    args = parser.parse_args()
    if args.mode == 'fingerprint':
        print(native_fingerprint())
        return
    if args.mode == 'gate':
        validate_gate(json.loads(os.environ['CI_NEEDS']))
        print('Required validation passed.')
        return
    base, head = os.environ.get('CI_BASE', ''), os.environ.get('CI_HEAD', '')
    force = os.environ.get('CI_FORCE_FULL', 'false')
    if force not in ('true', 'false'):
        raise ValueError('Invalid force_full input')
    full = classify(os.environ.get('CI_EVENT', ''), base, head, force == 'true')
    fingerprint = native_fingerprint()
    reused = None
    if full and force == 'false' and os.environ.get('CI_EVENT') == 'pull_request' and os.environ.get('GH_TOKEN'):
        reused = reusable_native_run(fingerprint, head)
        if reused:
            full = False
    with open(os.environ['GITHUB_OUTPUT'], 'a', encoding='utf-8') as output:
        output.write(f'full={str(full).lower()}\n')
        output.write(f'native={fingerprint}\n')
    if reused:
        print(f'Native inputs unchanged; reusing successful validation run {reused}.')
    print('Full native validation required.' if full else 'Content-only validation required.')


if __name__ == '__main__':
    main()
