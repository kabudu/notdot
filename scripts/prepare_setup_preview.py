#!/usr/bin/env python3
"""Prepare a silent, synthetic setup lifecycle preview of the compiled WASM UI.

Build with trunk first, then serve the printed directory on loopback. The fixture
never accesses accounts, keys, microphones or speakers. Browser results are
rendered in #harness-results; this is UI integration, not native authentication.
"""
from pathlib import Path
import shutil

ROOT = Path(__file__).resolve().parents[1]

def prepare(output):
    source = ROOT / 'ui-dist'
    if not (source / 'index.html').is_file():
        raise ValueError('Run trunk build --release first')
    output.mkdir(parents=True, exist_ok=True)
    shutil.copytree(source, output, dirs_exist_ok=True)
    shutil.copyfile(ROOT / 'scripts/fixtures/setup-preview.js', output / 'setup-preview.js')
    page = output / 'index.html'
    page.write_text(page.read_text().replace('<head>', '<head><script src="setup-preview.js"></script>', 1))
    return output

if __name__ == '__main__':
    print(prepare(ROOT / '.local/setup-preview'))
