#!/usr/bin/env python3
"""Synchronise original brand artwork and optionally rebuild app icon formats.

python3 scripts/sync_brand_assets.py --icons
Requires rsvg-convert and the pinned Tauri CLI for icon generation only.
"""
from pathlib import Path
import argparse
import shutil
import subprocess
import tempfile
root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--icons', action='store_true')
args = parser.parse_args()
shutil.copy2(root/'assets/logomark.svg',root/'docs/site-assets/logomark.svg')
shutil.copy2(root/'crates/notdot-orb/src/orb.frag',root/'docs/site-assets/orb.frag')
if args.icons:
    with tempfile.TemporaryDirectory(prefix='notdot-icons-') as folder:
        work=Path(folder)
        subprocess.run(['rsvg-convert','-w','1024','-h','1024',str(root/'assets/logomark.svg'),'-o',str(work/'source.png')],check=True)
        subprocess.run(['cargo','tauri','icon',str(work/'source.png'),'--output',str(work/'icons')],cwd=root,check=True)
        for name in ('32x32.png','64x64.png','128x128.png','128x128@2x.png','icon.png','icon.ico','icon.icns'):
            shutil.copy2(work/'icons'/name,root/'src-tauri/icons'/name)
print('Brand artwork and fluid material are in sync.')
