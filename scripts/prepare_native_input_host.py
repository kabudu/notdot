#!/usr/bin/env python3
"""Prepare a small native input test host without voice/model dependencies.

Uses the same Tauri version and orb_input.rs as the app. Its only command is
input layout; it never initializes a profile, credentials, a model or audio.
Build with cargo build --manifest-path .local/native-input-host/Cargo.toml.
"""
from pathlib import Path
import argparse
import json
import shutil
import tomllib
root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--output', type=Path, default=root / '.local/native-input-host')
output = parser.parse_args().output.resolve()
output.mkdir(parents=True, exist_ok=True)
(output / 'src').mkdir(exist_ok=True)
(output / 'web').mkdir(exist_ok=True)
(output / 'icons').mkdir(exist_ok=True)
(output / 'capabilities').mkdir(exist_ok=True)
packages = tomllib.loads((root / 'Cargo.lock').read_text())['package']
version = lambda name: next(p['version'] for p in packages if p['name'] == name)
(output / 'Cargo.toml').write_text(f'''[package]
name = "notdot-input-acceptance"
version = "0.0.0"
edition = "2024"
publish = false
[workspace]
[dependencies]
tauri = {{ version = "={version('tauri')}", features = ["macos-private-api"] }}
serde_json = "={version('serde_json')}"
[build-dependencies]
tauri-build = "={version('tauri-build')}"
[patch.crates-io]
glib = {{ path = "vendor/glib" }}
''')
(output / 'build.rs').write_text('fn main(){tauri_build::build()}\n')
main = (root / 'src-tauri/examples/orb-input-acceptance.rs').read_text()
main = main.replace('#[path = "../src/orb_input.rs"]\n', '')
(output / 'src/main.rs').write_text(main)
shutil.copy2(root / 'src-tauri/src/orb_input.rs', output / 'src/orb_input.rs')
shutil.copytree(root / 'vendor/glib', output / 'vendor/glib', dirs_exist_ok=True)
shutil.copy2(root / 'src-tauri/icons/32x32.png', output / 'icons/32x32.png')
shutil.copy2(root / 'src-tauri/icons/icon.ico', output / 'icons/icon.ico')
shutil.copy2(root / 'src-tauri/capabilities/main.json', output / 'capabilities/main.json')
config = json.loads((root / 'src-tauri/tauri.conf.json').read_text())
config.update(productName='notdot Input Acceptance', identifier='local.notdot.input-acceptance')
config['build'] = {'devUrl':'http://localhost:8107','frontendDist':'web'}
config['bundle'] = {'active':False,'icon':['icons/32x32.png','icons/icon.ico']}
config.pop('plugins',None)
(output / 'tauri.conf.json').write_text(json.dumps(config,indent=2)+'\n')
print(output)
