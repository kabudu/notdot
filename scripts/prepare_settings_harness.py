#!/usr/bin/env python3
"""Wrap a fresh Trunk build in synthetic settings-layout acceptance fixtures.

trunk build --release --dist .local/settings-qa
python3 scripts/prepare_settings_harness.py
python3 -m http.server 8101 --bind 127.0.0.1 --directory .local/settings-qa
Open /runner.html. No microphone, credentials or external service is used.
"""
from pathlib import Path
import argparse

root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--dist', type=Path, default=root / '.local/settings-qa')
dist = parser.parse_args().dist
page = dist / 'index.html'
text = page.read_text()
assert 'conversation-harness.js' not in text, 'Rebuild with Trunk first'
text = text.replace('<head>', '<head><script src="conversation-harness.js"></script>', 1)
text = text.replace('</body>', '<script src="settings-layout-harness.js"></script></body>')
page.write_text(text)
adapter = (root / 'scripts/fixtures/conversation-harness.js').read_text()
marker = '  let enrolIndex='
assert marker in adapter
adapter = adapter.replace(marker, '''  people.people[0].voice_enrolled=true;
  people.people[0].needs_name=false;
  people.people.push({id:'member',role:'member',name:'Alexandertheverylongsyntheticnamewithoutspacesforlayouttesting',voice_enrolled:true,shared_chats:[]});
  state.quota={availability:'available',stale:false,report:{meters:[{used:470,limit:10000,unit:'provider credits',label:'Voice allowance',reset_unix:1800000000,overage:'disabled'}]}};
  window.settingsFixture={state,settings,people};
''' + marker, 1)
marker = "if(cmd==='voice_catalog')return {voices:[{id:'synthetic',name:'Synthetic',description:'Test adapter'}],next_cursor:null};"
assert marker in adapter
adapter = adapter.replace(marker, """if(cmd==='voice_catalog')return {voices:Array.from({length:8},(_,i)=>({id:i?'voice-'+i:'synthetic',name:i?'Test voice '+i:'Synthetic',description:'Preview sample'})).filter(v=>!args.search||v.name.toLowerCase().includes(args.search.toLowerCase())),next_cursor:null};""", 1)

marker = "if(cmd==='model_catalog')return [{model:'gpt-6.1-terra',displayName:'Terra',supportedReasoningEfforts:[{reasoningEffort:'medium',description:'Medium'}]}];"
assert marker in adapter
adapter = adapter.replace(marker, """if(cmd==='model_catalog')return ['gpt-6.1-terra','test-fast','test-long-model'].map((model,i)=>({model,displayName:i===0?'Terra':i===1?'A faster synthetic model':'A synthetic model with a deliberately long display name',supportedReasoningEfforts:['low','medium','high','xhigh','max'].map(reasoningEffort=>({reasoningEffort,description:reasoningEffort}))}));""", 1)
adapter = adapter.replace("if(cmd==='speaker_summary')", """if(cmd==='phones_status')return {available:true,running:true,settings:{enabled:true,android_apps:[],apple_apps:[],apple_categories:[],apple_development:false},profiles:[{id:'owner',name:'Synthetic owner'}],pending:[{offer:'abcd',name:'Synthetic phone',code:'123 456'}],devices:[{id:'device',name:'Synthetic phone',owner:true,step_up:null}],revocations_fresh:false,revocations:{}};
    if(cmd.startsWith('phones_'))return {};
    if(cmd==='speaker_summary')""", 1)
(dist / 'conversation-harness.js').write_text(adapter)
(dist / 'settings-layout-harness.js').write_text((root / 'scripts/fixtures/settings-layout-harness.js').read_text())
(dist / 'runner.html').write_text('''<!doctype html><html lang="en"><meta charset="utf-8"><title>Settings layout checks</title><body style="background:#16202a;color:white;font-family:system-ui"><h1>Settings layout acceptance</h1><button id="run">Run all layouts</button><pre id="output"></pre><div id="host"></div><script>
const variants=[[320,360],[360,360],[450,680],[900,900]];let results=[];
run.onclick=async()=>{results=[];for(const [width,height] of variants){await new Promise(resolve=>{const f=document.createElement('iframe');f.width=width;f.height=height;f.src='index.html';host.replaceChildren(f);const listen=e=>{if(e.source!==f.contentWindow||e.origin!==location.origin)return;results.push(e.data);output.textContent=JSON.stringify(results,null,2);window.removeEventListener('message',listen);resolve();};window.addEventListener('message',listen);});}};
</script></body></html>''')
print('Prepared synthetic settings layout checks. Open /runner.html.')
