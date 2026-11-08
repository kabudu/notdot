#!/usr/bin/env python3
"""Prepare graphics lifecycle checks around a fresh production WASM build.

trunk build --release --dist .local/shader-qa
python3 scripts/prepare_shader_harness.py
python3 -m http.server 8105 --bind 127.0.0.1 --directory .local/shader-qa
Open /shader.html and choose Run shader acceptance. All IPC/audio is synthetic.
"""
from pathlib import Path
import argparse
import subprocess
import sys

root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--dist', type=Path, default=root / '.local/shader-qa')
dist = parser.parse_args().dist.resolve()
subprocess.run([sys.executable, str(root / 'scripts/prepare_settings_harness.py'), '--dist', str(dist)], check=True)
page = dist / 'index.html'
page.write_text(page.read_text().replace('<script src="settings-layout-harness.js"></script>', ''))
checks = (root / 'scripts/fixtures/shader-harness.js').read_text()
(dist / 'shader-checks.js').write_text(checks + "\nif(location.search.includes('autorun'))window.addEventListener('load',run);\n")
(dist / 'shader.html').write_text('''<!doctype html><html lang="en"><meta charset="utf-8"><title>Shader acceptance</title><body style="background:#182535;color:white;font:15px system-ui"><button>Run shader acceptance</button><pre style="max-width:calc(100% - 480px);white-space:pre-wrap"></pre><iframe title="Production orb under test" src="index.html" width="460" height="460" style="position:fixed;right:0;top:50px;background:transparent;border:0"></iframe><script src="shader-checks.js"></script></body></html>''')
print('Prepared production shader checks. Open /shader.html.')
