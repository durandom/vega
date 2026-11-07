#!/usr/bin/python3
import json, os, sys
from pathlib import Path
root=Path(os.environ['HOME'])
a=sys.argv[1:]
if a[0]=='--json': a=a[1:]
with open(root/'calls','a') as f: f.write(json.dumps(a)+'\n')
state=root/'terminals.json'
ts=json.loads(state.read_text()) if state.exists() else {}
parent={'address':'t7','name':'parent','state':'live','pid':int(os.environ['TEST_PARENT_PID']),'status':{'app':'codex','state':'idle'},'cwd':str(root),'labels':{'herdr.space':'s1'}}
def save(): state.write_text(json.dumps(ts))
def err(kind): print(json.dumps({'error':{'kind':kind,'message':kind,'hint':'inspect target'}}),file=sys.stderr);sys.exit(3)
def get(t):
 if t=='t7': return parent
 for v in ts.values():
  if v['address']==t or (v['name']==t and v['state']=='live'): return v
 err('not_found')
cmd=a[0]
if cmd=='connect':
 request=json.loads(sys.stdin.readline());assert request['method']=='terminal.run'
 p=request['params']
 with open(root/'calls','a') as f: f.write(json.dumps(['run',p])+'\n')
 n=p['name']
 if any(t['name']==n and t['state']=='live' for t in ts.values()): err('name_taken')
 i='t'+str(int((root/'counter').read_text())+1 if (root/'counter').exists() else 8)
 (root/'counter').write_text(i[1:])
 if p.get('on'): i=p['on']+'/'+i
 labels=p['labels']
 assert p['argv'][0]==labels['vega.kind']
 t={'address':i,'argv':p['argv'],'name':n,'state':'live','pid':int(os.environ['TEST_PARENT_PID']),'status':{'app':p['argv'][0],'state':'idle'},'cwd':p['cwd'],'labels':labels,'turn':0}
 ts[i]=t;save()
 if os.environ.get('TEST_UNDELIVERED'): t['first_message']={'sent':False,'reason':'blocked'}
 print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':t}))
elif cmd=='exec':
 if os.environ.get('TEST_PREPARE_FAIL'): err('unavailable')
 assert a[a.index('--')+1:a.index('--')+3]==['vega-mux','prepare']
 if (root/'remote-home').exists():
  import subprocess
  e=dict(os.environ, HOME=str(root/'remote-home'))
  for k in ['HC_TERMINAL_ID','HERDR_PANE_ID','XDG_CONFIG_HOME','XDG_STATE_HOME']: e.pop(k,None)
  r=subprocess.run([os.environ['TEST_MUX_EXE']]+a[a.index('--')+2:],env=e,capture_output=True)
  out,errb,rc=r.stdout,r.stderr,r.returncode
 else: out,errb,rc=(json.dumps({'schema':1,'trust':None})+'\n').encode(),b'',0
 # Real `hc exec --json` wraps the program's output: base64 stdout/stderr plus its exit code.
 import base64
 print(json.dumps({'code':rc,'ms':1,'signal':None,'stderr':base64.b64encode(errb).decode(),'stdout':base64.b64encode(out).decode(),'timed_out':False,'truncated':False}))
elif cmd=='get': print(json.dumps(get(a[1])))
elif cmd=='label':
 t=get(a[1])
 for pair in a[2:]:
  k,v=pair.split('=',1)
  if v: t['labels'][k]=v
  else: t['labels'].pop(k,None)
 save();print('{}')
elif cmd=='ls': print(json.dumps({'terminals':([] if os.environ.get('TEST_HIDE_PARENT') else [parent])+list(ts.values())}))
elif cmd=='machine': print(os.environ.get('TEST_MACHINES','{"machines":[]}'))
elif cmd=='read':
 t=get(a[1]);q=os.environ.get('TEST_DIALOG','')
 sel=int((root/'selected').read_text()) if (root/'selected').exists() else 1
 text='recent output'
 if q=='trust': text='Do you trust the files in this folder?\n1. Yes, I trust this folder\n2. No, exit'
 elif q=='question' and not (root/'answered').exists(): text='Which colour?\n'+('❯' if sel==1 else ' ')+' 1. Red\n'+('❯' if sel==2 else ' ')+' 2. Blue\nEnter to select · ↑/↓ to navigate'
 print(json.dumps({'text':text,'complete':True,'how':'screen'}))
elif cmd=='keys':
 get(a[1])
 for k in a[2:]:
  if k=='down': (root/'selected').write_text('2')
  elif k=='enter': (root/'answered').touch()
 print('{}')
elif cmd=='send':
 t=get(a[1])
 if t['status']['state']=='blocked': err('blocked')
 print('{}')
elif cmd=='kill': get(a[1])['state']='finished';save();print('{}')
elif cmd=='rm': ts.pop(a[1],None);save();print('{}')
else: err('unexpected_'+cmd)
