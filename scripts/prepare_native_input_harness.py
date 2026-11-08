#!/usr/bin/env python3
"""Wrap a fresh Trunk build for the no-credentials native input example.

trunk build --release --dist .local/native-input-qa
python3 scripts/prepare_native_input_harness.py
python3 -m http.server 8107 --bind 127.0.0.1 --directory .local/native-input-qa
cargo run -p notdot --example orb-input-acceptance

The example only registers the native input-layout command. All model, voice,
settings and profile calls are synthetic; it has no keychain or microphone code.
"""
from pathlib import Path
import argparse
import subprocess
import sys
root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--dist', type=Path, default=root / '.local/native-input-qa')
dist = parser.parse_args().dist.resolve()
subprocess.run([sys.executable, str(root / 'scripts/prepare_shader_harness.py'), '--dist', str(dist)], check=True)
page = dist / 'conversation-harness.js'
page.write_text('''const nativeInputApi={invoke:window.__TAURI__.core.invoke,listen:window.__TAURI__.event.listen,emitTo:window.__TAURI__.event.emitTo,window:window.__TAURI__.window,dpi:window.__TAURI__.dpi};
''' + page.read_text() + '''
const syntheticInvoke=window.__TAURI__.core.invoke,syntheticListen=window.__TAURI__.event.listen;
window.__TAURI__.core.invoke=(command,args)=>command==='orb_input_mode'?nativeInputApi.invoke('record_orb_input_mode',args):syntheticInvoke(command,args);
window.__TAURI__.event.listen=(name,callback)=>name==='orb-control'?nativeInputApi.listen(name,callback):syntheticListen(name,callback);
window.__TAURI__.event.emitTo=nativeInputApi.emitTo;
window.__TAURI__.window=nativeInputApi.window;
window.__TAURI__.dpi=nativeInputApi.dpi;
''')
print('Prepared isolated native input acceptance assets.')
