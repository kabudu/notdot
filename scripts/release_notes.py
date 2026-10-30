#!/usr/bin/env python3
"""Extract the reviewed product notes for one version from CHANGELOG.md."""
import argparse
from pathlib import Path
import re


def release_notes(changelog: str, version: str) -> str:
    if not re.fullmatch(r'(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)', version):
        raise ValueError('Use a stable major.minor.patch version')
    heading = re.compile(r'^## \[' + re.escape(version) + r'\] - \d{4}-\d{2}-\d{2}$', re.MULTILINE)
    matches = list(heading.finditer(changelog))
    if len(matches) != 1:
        raise ValueError(f'Expected one dated changelog entry for {version}')
    body = changelog[matches[0].end():]
    following = re.search(r'^## ', body, re.MULTILINE)
    if following:
        body = body[:following.start()]
    body = body.strip()
    if not body:
        raise ValueError('Release notes must not be empty')
    if '\u2014' in body:
        raise ValueError('Use plain punctuation in release notes')
    if len(re.findall(r'^- ', body, re.MULTILINE)) > 7:
        raise ValueError('Keep release highlights to seven or fewer')
    return body + '\n'


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('version')
    parser.add_argument('--changelog', type=Path, default=Path('CHANGELOG.md'))
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    notes = release_notes(args.changelog.read_text(encoding='utf-8'), args.version)
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(notes, encoding='utf-8')
    else:
        print(notes, end='')


if __name__ == '__main__':
    main()
