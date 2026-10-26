#!/usr/bin/env python3
import json,pathlib,secrets
from rpc import Client
root=pathlib.Path.cwd(); local=root/'.local'; local.mkdir(exist_ok=True)
c=Client(); report={}; marker='notdot-'+secrets.token_hex(8)
try:
 r=c.call('thread/start',{'model':'gpt-6.1-terra','cwd':str(root),'config':{'model_reasoning_effort':'medium'},'developerInstructions':'This is a bridge feasibility probe. Only perform explicitly requested read-only connector operations. Do not modify external state or local files.'})
 tid=r['thread']['id']; report['start']={k:v for k,v in r.items() if k!='thread'}; report['threadId']=tid
 c.call('thread/name/set',{'threadId':tid,'name':'notdot bridge proof'})
 print('Thread:',tid,flush=True)
 report['firstTurn']=c.turn(tid,'Remember this exact bridge marker for our next turn: '+marker+'. Reply with the marker only. Do not use tools.')
 print('First turn:',report['firstTurn']['turn']['status'],flush=True)
 local.joinpath('first-events.json').write_text(json.dumps(c.events,indent=2)); c.close(); c=Client()
 report['resume']=c.call('thread/resume',{'threadId':tid,'model':'gpt-6.1-terra','config':{'model_reasoning_effort':'medium'}})
 report['secondTurn']=c.turn(tid,'What exact bridge marker did I give you? Then use the connected GitHub app to read repository metadata for kabudu/notdot and report its visibility. Use the connected tool, not shell, web search or guesses. Do not modify anything. If unavailable report that clearly.')
 print('Second turn:',report['secondTurn']['turn']['status'],flush=True)
 local.joinpath('second-events.json').write_text(json.dumps(c.events,indent=2))
 local.joinpath('proof.json').write_text(json.dumps(report,indent=2))
 print(json.dumps({'threadId':tid,'marker':marker,'messages':[e['params'] for e in c.events if e.get('method')=='item/completed']},indent=2))
finally: c.close()
