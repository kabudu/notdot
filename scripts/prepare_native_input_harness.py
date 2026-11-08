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
adapter=page.read_text().replace('  function emit(phase,token)', '  window.nativeInputLog=log;\n  function emit(phase,token)')
page.write_text('''const nativeInputApi={invoke:window.__TAURI__.core.invoke,listen:window.__TAURI__.event.listen,emitTo:window.__TAURI__.event.emitTo,window:window.__TAURI__.window,dpi:window.__TAURI__.dpi};
''' + adapter + '''
const syntheticInvoke=window.__TAURI__.core.invoke,syntheticListen=window.__TAURI__.event.listen;
window.__TAURI__.core.invoke=(command,args)=>command==='orb_input_mode'?nativeInputApi.invoke('record_orb_input_mode',args):command==='report_visual'?nativeInputApi.invoke('record_visual_report',{report:{...args.report,at:performance.now()}}):syntheticInvoke(command,args);
window.__TAURI__.event.listen=(name,callback)=>name==='orb-control'?nativeInputApi.listen(name,callback):syntheticListen(name,callback);
window.__TAURI__.event.emitTo=nativeInputApi.emitTo;
window.__TAURI__.window=nativeInputApi.window;
window.__TAURI__.dpi=nativeInputApi.dpi;
document.addEventListener('DOMContentLoaded',()=>{
 const output=document.querySelector('#harness-results'),orb=document.querySelector('#orb');
 document.querySelector('#run-silent-checks').hidden=true;
 const report=()=>{const log=window.nativeInputLog;output.textContent='State: '+(orb.dataset.state||'starting')+'\\nCaptures: '+log.filter(x=>x.event==='capture').length+' | Cancelled: '+log.filter(x=>x.event==='cancel').length+' | Model requests: '+log.filter(x=>x.event==='message').length;};
 new MutationObserver(report).observe(orb,{attributes:true,attributeFilter:['data-state']});report();
});
''')
print('Prepared isolated native input acceptance assets.')
