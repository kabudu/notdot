#!/usr/bin/env python3
"""Hold ownership while Desktop reads, then verify another companion turn."""
import json,pathlib,time
from rpc import Client
p=pathlib.Path('.local/viewer-go');p.unlink(missing_ok=True)
c=Client();tid='01a17a27-e2a4-7412-929a-d8e07fd2a4db'
try:
 r=c.call('thread/resume',{'threadId':tid,'model':'gpt-6.1-terra','config':{'model_reasoning_effort':'medium'}})
 print('Companion owns thread; waiting for desktop read',flush=True)
 deadline=time.monotonic()+120
 while not p.exists():
  if time.monotonic()>deadline:raise TimeoutError('Desktop read signal not received')
  time.sleep(.2)
 c.turn(tid,'Reply with exactly: Companion retained ownership after desktop viewing. Do not use tools.')
 pathlib.Path('.local/viewer-events.json').write_text(json.dumps(c.events,indent=2))
 for e in c.events:
  i=e.get('params',{}).get('item',{})
  if e.get('method')=='item/completed' and i.get('phase')=='final_answer':print(i['text'])
finally:c.close()
