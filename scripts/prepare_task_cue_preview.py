#!/usr/bin/env python3
"""Prepare a silent cue gallery and real-renderer pixel contrast checks.

Build debug WASM with Trunk, then run this script. Release WASM deliberately
omits the acceptance export. All service, capture and audio adapters are fake.
"""
import argparse
from pathlib import Path
root=Path(__file__).resolve().parents[1]
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--dist',type=Path,default=root/'.local/continuity-ui')
dist=parser.parse_args().dist
page=dist/'index.html';html=page.read_text();marker='<script src="conversation-harness.js"></script>'
if marker not in html:page.write_text(html.replace('<head>','<head>'+marker,1))
adapter=(root/'scripts/fixtures/conversation-harness.js').read_text()
needle="document.addEventListener('DOMContentLoaded',()=>{"
preview="""document.addEventListener('DOMContentLoaded',()=>{const query=new URLSearchParams(location.search),cue=query.get('cue');if(['working','attention','finished','review'].includes(cue)){settings.hue=Number(query.get('hue')||195);settings.followup_enabled=false;state.initiative={tasks:{cue,description:'Preview: '+cue}};document.body.style.background=query.get('background')==='light'?'#eef2f6':'#0a1018';if(cue==='finished')setInterval(()=>{state.initiative.tasks.cue='none';setTimeout(()=>{state.initiative.tasks.cue='finished'},600)},8500);return;}"""
assert needle in adapter
(dist/'conversation-harness.js').write_text(adapter.replace(needle,preview,1))
(dist/'cue-contrast.js').write_text((root/'scripts/fixtures/cue-contrast.js').read_text())
(dist/'cue-contrast.html').write_text('''<!doctype html><html lang="en"><meta charset="utf-8"><title>Cue contrast acceptance</title><body style="background:#101a28;color:white;font:14px system-ui"><h1>Cue contrast acceptance</h1><p>Real production shader, fallback and cue renderer. Checks integrated light against adjacent core regions at maximum speech-wave brightness, including a white desktop background.</p><button>Run contrast checks</button><pre>Ready</pre><iframe title="Synthetic production WASM host" src="./" width="10" height="10" style="position:absolute;left:-100px;border:0"></iframe><script src="cue-contrast.js"></script></body></html>''')
(dist/'task-cues.html').write_text('''<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>notdot task cues</title><style>
:root{color-scheme:dark;font-family:system-ui;background:#0a1018;color:#edf4fa}*{box-sizing:border-box}body{margin:0;padding:32px;max-width:1160px;margin-inline:auto}h1{font-size:32px;margin:0 0 8px}p{color:#b1c2d0;line-height:1.55}.grid{display:grid;grid-template-columns:1fr 1fr;gap:20px}article{border:1px solid #263b4d;border-radius:22px;overflow:hidden;background:#0a1018}iframe{display:block;width:100%;height:320px;border:0;pointer-events:none}article div{padding:0 24px 22px}h2{font-size:20px;margin:0 0 8px}article p{margin:0;font-size:15px}.controls{display:flex;flex-wrap:wrap;gap:24px;margin-block:24px}select{font:inherit;padding:8px;border-radius:8px;margin-left:8px}.note{font-size:14px;margin-top:24px}body.light{background:#eef2f6;color:#122333}body.light p{color:#42556b}body.light article{background:#eef2f6;border-color:#8fa0b1}@media(max-width:650px){body{padding:20px}.grid{grid-template-columns:1fr}iframe{height:310px}}</style>
<h1>Task cues</h1><p>Four patterns made from the orb’s own light and fluid motion.</p><div class="controls"><label>Orb colour<select id="theme"><option value="195">Blue</option><option value="280">Purple</option><option value="135">Green</option><option value="35">Amber</option><option value="330">Pink</option></select></label><label>Background<select id="background"><option value="dark">Dark</option><option value="light">Light</option></select></label></div><div class="grid">
<article><iframe title="Working orb cue" data-cue="working" src="./?cue=working" tabindex="-1"></iframe><div><h2>Working</h2><p>A slow coloured current moves through the core.</p></div></article>
<article><iframe title="Needs attention orb cue" data-cue="attention" src="./?cue=attention" tabindex="-1"></iframe><div><h2>Needs attention</h2><p>A paired breath of light moves through the core.</p></div></article>
<article><iframe title="Successful result orb cue" data-cue="finished" src="./?cue=finished" tabindex="-1"></iframe><div><h2>Reported success</h2><p>A soft bloom fills the orb, then settles. Repeated here for comparison.</p></div></article>
<article><iframe title="Result needs review orb cue" data-cue="review" src="./?cue=review" tabindex="-1"></iframe><div><h2>Review the result</h2><p>A calm cluster of light stays low in the orb until you review the result.</p></div></article>
</div><p class="note">Silent preview using the app’s orb renderer. Listening and speaking take priority in the app. Shapes and spoken updates supplement colour.</p><script>const theme=document.getElementById('theme'),background=document.getElementById('background');function update(){document.body.classList.toggle('light',background.value==='light');for(const f of document.querySelectorAll('iframe'))f.src='./?cue='+f.dataset.cue+'&hue='+theme.value+'&background='+background.value;}theme.onchange=background.onchange=update;</script></html>''')
print('Prepared /task-cues.html and /cue-contrast.html')
