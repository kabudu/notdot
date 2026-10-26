#!/usr/bin/env python3
"""Probe documented lifecycle calls from an independent client."""
import json,pathlib
from rpc import Client
c=Client();tid='01a17a27-e2a4-7412-929a-d8e07fd2a4db';r={}
try:
 for method in ['thread/loaded/list','thread/archive','thread/unarchive','thread/resume']:
  try:
   params={} if method=='thread/loaded/list' else {'threadId':tid}
   if method=='thread/resume':params.update(model='gpt-6.1-terra',config={'model_reasoning_effort':'medium'})
   result=c.call(method,params);r[method]={'ok':True,'model':result.get('model'),'reasoningEffort':result.get('reasoningEffort')}
  except Exception as e:r[method]={'error':str(e)}
  print(method,r[method],flush=True)
 pathlib.Path('.local/ownership.json').write_text(json.dumps(r,indent=2))
finally:c.close()
