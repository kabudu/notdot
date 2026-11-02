#!/usr/bin/env python3
"""Prepare test-only service/audio adapters around an already built production WASM UI.

Run: trunk build --dist .local/continuity-ui
     python3 scripts/prepare_conversation_harness.py
     python3 -m http.server 8095 --bind 127.0.0.1 --directory .local/continuity-ui
Open http://localhost:8095 and click Run silent integration checks.
This is a UI integration simulation, not microphone/provider/hardware acceptance.
"""
from pathlib import Path
root = Path(__file__).resolve().parents[1]
dist = root / '.local/continuity-ui'
page = dist / 'index.html'
text = page.read_text()
marker = '<script src="conversation-harness.js"></script>'
assert marker not in text, 'Rebuild with Trunk before preparing the harness again'
assert '<head>' in text
page.write_text(text.replace('<head>', '<head>' + marker, 1))
(dist / 'conversation-harness.js').write_text((root / 'scripts/fixtures/conversation-harness.js').read_text())
print('Prepared silent, synthetic-adapter harness; no production files changed.')
