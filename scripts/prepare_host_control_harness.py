#!/usr/bin/env python3
"""Wrap a fresh Trunk build in the synthetic browser/computer-control checks.

trunk build --release --dist .local/host-qa
python3 scripts/prepare_host_control_harness.py
python3 -m http.server 8102 --bind 127.0.0.1 --directory .local/host-qa
Open /index.html#host-checks. The results appear at the bottom of the page and
in window.hostChecks. Then, on /index.html, call hostFixture.ask('…') and press
Tab, Shift+Tab and Escape with real key presses (CDP or a person): focus stays
in the dialog and Escape answers "Don't allow" once.
No microphone, credentials, browser automation or external service is used.
"""
from pathlib import Path
import argparse

root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--dist', type=Path, default=root / '.local/host-qa')
dist = parser.parse_args().dist
page = dist / 'index.html'
text = page.read_text()
assert 'conversation-harness.js' not in text, 'Rebuild with Trunk first'
text = text.replace('<head>', '<head><script src="conversation-harness.js"></script><script src="host-control-harness.js"></script>', 1)
page.write_text(text)
for name in ('conversation-harness.js', 'host-control-harness.js'):
    (dist / name).write_text((root / 'scripts/fixtures' / name).read_text())
print('Prepared synthetic host-control checks. Open /index.html#host-checks.')
