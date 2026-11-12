#!/usr/bin/env python3
"""Refuse branch changes that carry personal or machine-specific data.

python3 scripts/check_private_data.py [--base origin/master]

Scans added lines in `git diff <base>...HEAD` plus staged and unstaged
changes, and the messages of every commit in <base>..HEAD. Generic patterns
only: absolute home paths, public IPv4 addresses, e-mail addresses and
private evidence folders. Maintainers may list extra literal strings, one
per line, in the ignored file .local/private-patterns.txt; that list is
never committed. Exit status 1 lists each finding without echoing matches
from the private list.
"""
from pathlib import Path
import argparse
import ipaddress
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
ALLOWED_EMAILS = {'noreply@anthropic.com', 'noreply@github.com'}
PATTERNS = [
    ('absolute home path', re.compile(r'(?<![\w.])(/Users/|/home/)[A-Za-z0-9._-]+/|[A-Za-z]:\\\\?Users\\\\?[A-Za-z0-9._-]+')),
    ('private evidence folder', re.compile(r'\.local/(mobile-design|evidence)\b')),
    ('macOS temp path', re.compile(r'/private/(tmp|var)/[A-Za-z0-9._-]+/')),
]
EMAIL = re.compile(r'[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}')
IPV4 = re.compile(r'(?<![\d.])(\d{1,3}(?:\.\d{1,3}){3})(?![\d.])')


def git(*args):
    return subprocess.run(['git', *args], cwd=ROOT, check=True, capture_output=True, text=True).stdout


def public_ip(text):
    try:
        address = ipaddress.ip_address(text)
    except ValueError:
        return False
    documentation = [ipaddress.ip_network(n) for n in ('192.0.2.0/24', '198.51.100.0/24', '203.0.113.0/24')]
    return address.is_global and not any(address in n for n in documentation)


def findings(lines, private):
    for where, line in lines:
        for label, pattern in PATTERNS:
            if pattern.search(line):
                yield where, label
        for email in EMAIL.findall(line):
            if email.lower() not in ALLOWED_EMAILS and not email.lower().endswith(('@example.com', '@example.org', '@users.noreply.github.com')):
                yield where, 'e-mail address'
        for candidate in IPV4.findall(line):
            if public_ip(candidate):
                yield where, 'public IPv4 address'
        lowered = line.lower()
        for entry in private:
            if entry in lowered:
                yield where, 'entry from .local/private-patterns.txt'


def added_lines(diff):
    path = '?'
    for line in diff.splitlines():
        if line.startswith('+++ '):
            path = line[6:] if line.startswith('+++ b/') else line[4:]
        elif line.startswith('+') and not line.startswith('+++'):
            # This file names the patterns it looks for.
            if path != 'scripts/check_private_data.py':
                yield path, line[1:]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base', default='origin/master')
    base = parser.parse_args().base
    private_file = ROOT / '.local/private-patterns.txt'
    private = [l.strip().lower() for l in private_file.read_text().splitlines() if l.strip()] if private_file.exists() else []
    lines = list(added_lines(git('diff', f'{base}...HEAD')))
    lines += list(added_lines(git('diff', 'HEAD')))
    for commit in git('rev-list', f'{base}..HEAD').split():
        message = git('log', '-1', '--format=%B', commit)
        lines += [(f'commit {commit[:10]} message', l) for l in message.splitlines()]
    found = sorted(set(findings(lines, private)))
    for where, label in found:
        print(f'{where}: {label}', file=sys.stderr)
    print(f'private-data check: {len(lines)} lines scanned, {len(found)} finding(s)'
          + (f', {len(private)} private patterns' if private else ', no private pattern file'))
    return 1 if found else 0


if __name__ == '__main__':
    sys.exit(main())
