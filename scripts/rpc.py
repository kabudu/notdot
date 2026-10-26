#!/usr/bin/env python3
"""Small JSONL app-server probe. Raw evidence stays in ignored .local/."""
import json, subprocess, threading, queue, time, pathlib

class Client:
    def __init__(self, proxy=False):
        self.p = subprocess.Popen(['codex','app-server'] + (['proxy'] if proxy else ['--stdio']), stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
        self.q=queue.Queue(); self.seq=0; self.events=[]
        threading.Thread(target=self._read,daemon=True).start()
        self.call('initialize',{'clientInfo':{'name':'notdot_bridge_probe','version':'0.1.0'},'capabilities':{'experimentalApi':True}})
        self.send({'method':'initialized','params':{}})
    def _read(self):
        for line in self.p.stdout:
            try: self.q.put(json.loads(line))
            except ValueError: pass
    def send(self,msg):
        self.p.stdin.write(json.dumps(msg)+'\n'); self.p.stdin.flush()
    def receive(self,timeout=90):
        msg=self.q.get(timeout=timeout); self.events.append(msg)
        if 'id' in msg and 'method' in msg:
            self.send({'id':msg['id'],'error':{'code':-32601,'message':'Probe does not grant approvals or implement host tools'}})
        return msg
    def call(self,method,params):
        self.seq+=1; ident=self.seq; self.send({'id':ident,'method':method,'params':params})
        while True:
            msg=self.receive()
            if msg.get('id')==ident and 'method' not in msg:
                if 'error' in msg: raise RuntimeError(json.dumps(msg['error']))
                return msg['result']
    def turn(self,thread,prompt):
        result=self.call('turn/start',{'threadId':thread,'model':'gpt-6.1-terra','effort':'medium','input':[{'type':'text','text':prompt}]})
        deadline=time.monotonic()+180
        while time.monotonic()<deadline:
            msg=self.receive()
            if msg.get('method')=='turn/completed': return msg['params']
        raise TimeoutError('Turn did not complete')
    def close(self):
        self.p.terminate()
        try: self.p.wait(timeout=5)
        except subprocess.TimeoutExpired: self.p.kill()

if __name__=='__main__':
    c=Client(); results={}
    try:
        for method,params in [('model/list',{}),('app/installed',{}),('app/list',{}),('thread/list',{'limit':5})]:
            try: results[method]=c.call(method,params)
            except Exception as e: results[method]={'error':str(e)}
        pathlib.Path('.local/discovery.json').write_text(json.dumps(results,indent=2))
        models=results.get('model/list',{}).get('data',[])
        print(json.dumps({'models':[m for m in models if 'terra' in str(m).lower()], 'results':{k:list(v.keys()) for k,v in results.items()}},indent=2))
    finally: c.close()
