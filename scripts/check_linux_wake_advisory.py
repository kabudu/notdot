#!/usr/bin/env python3
"""Accept one completed Linux wake-accuracy miss; reject all other failures."""
import argparse
import os
from pathlib import Path
import re

TEST = 'wake::tests::bundled_detector_accepts_48khz_wake_fixture'
MARKER = '48 kHz wake fixture must activate'


def classify(text, exit_code):
    text = re.sub(r'\x1b\[[0-?]*[ -/]*[@-~]', '', text)
    # A footer can precede an abort in a destructor. That is never an accuracy waiver.
    if re.search(r'signal:|SIGABRT|SIGSEGV|SIGBUS|SIGILL|SIGTRAP|core dumped|process abort|\baborted\b|segmentation fault|illegal instruction|bus error|trace/breakpoint trap|free\(\):|malloc\(\):|double free|memory corruption', text, re.IGNORECASE):
        raise ValueError('Native/process failure is not covered by the accuracy policy')
    footers = re.findall(r'^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored; \d+ measured; \d+ filtered out; finished in [0-9.]+s$', text, re.MULTILINE)
    cases = re.findall(r'^test (\S+) \.\.\. (ok|FAILED)$', text, re.MULTILINE)
    if text.splitlines().count('running 1 test') != 1 or len(footers) != 1 or len(cases) != 1 or cases[0][0] != TEST:
        raise ValueError('The exact single advisory test did not complete')
    panics = re.findall(r"^thread '[^']+'(?: \(\d+\))? panicked at [^\n]+:\d+:\d+:\n([^\n]*)$", text, re.MULTILINE)
    if exit_code == 0 and footers[0] == ('ok', '1', '0', '0') and cases[0][1] == 'ok' and not panics:
        return 'passed'
    expected_panic = re.search(rf"^thread '{re.escape(TEST)}'(?: \(\d+\))? panicked at [^\n]*wake\.rs:\d+:\d+:\n{re.escape(MARKER)}$", text, re.MULTILINE)
    if exit_code == 101 and footers[0] == ('FAILED', '0', '1', '0') and cases[0][1] == 'FAILED' and panics == [MARKER] and expected_panic:
        return 'accuracy-miss'
    raise ValueError('Failure is not the completed, explicitly waived accuracy assertion')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('log', type=Path)
    parser.add_argument('exit_code', type=int)
    args = parser.parse_args()
    try:
        outcome = classify(args.log.read_text(encoding='utf-8'), args.exit_code)
    except (OSError, ValueError) as error:
        raise SystemExit(str(error)) from None
    if outcome == 'passed':
        print('The exact Linux 48 kHz wake test passed.')
        return
    report = os.environ.get('LINUX_WAKE_REPORT', 'docs/validation/2026-10-29-linux.md')
    print(f'::warning::The completed 48 kHz recognition assertion missed. The maintainer 0.1 Linux wake-reliability policy covers this accuracy result only; see {report}')
    if summary := os.environ.get('GITHUB_STEP_SUMMARY'):
        with open(summary, 'a', encoding='utf-8') as stream:
            stream.write(f'### Linux wake-accuracy advisory\n\nThe exact 48 kHz recognition assertion completed with a miss. The maintainer 0.1 policy follows the accepted general Linux wake-reliability limitation. Other errors remain blocking. [Linux evidence and policy]({report}).\n')


if __name__ == '__main__':
    main()
