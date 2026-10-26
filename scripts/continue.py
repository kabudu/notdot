#!/usr/bin/env python3
"""Resume a completed desktop proof conversation; never run against an active turn."""
import json,sys,pathlib
from rpc import Client
if len(sys.argv)!=2:
    raise SystemExit('Usage: python3 scripts/continue.py THREAD_ID')
c=Client()
try:
    r=c.call('thread/resume',{'threadId':sys.argv[1],'model':'gpt-6.1-terra','config':{'model_reasoning_effort':'medium'}})
    print('Model:',r['model'],'effort:',r['reasoningEffort'],flush=True)
    c.turn(sys.argv[1],'What exact desktop marker did I most recently ask you to remember? Reply with the marker only. Do not use tools.')
    for e in c.events:
        if e.get('method')=='item/completed':
            i=e['params']['item']
            if i.get('type')=='agentMessage' and i.get('phase')=='final_answer': print(i['text'])
    pathlib.Path('.local').mkdir(exist_ok=True)
    pathlib.Path('.local/desktop-continuation.json').write_text(json.dumps(c.events,indent=2))
finally: c.close()
