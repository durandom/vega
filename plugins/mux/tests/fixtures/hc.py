#!/usr/bin/python3
import json, os, sys
from pathlib import Path
root=Path(os.environ['HOME'])
a=sys.argv[1:]
assert a == ['connect']
with open(root/'calls','a') as f: f.write(json.dumps(a)+'\n')
state=root/'terminals.json'
ts=json.loads(state.read_text()) if state.exists() else {}
parent={'address':'t7','name':'parent','state':'live','pid':int(os.environ['TEST_PARENT_PID']),'status':{'app':'codex','state':'idle'},'cwd':str(root),'labels':{'herdr.space':'s1'}}
def save(): state.write_text(json.dumps(ts))
def err(kind):
 if request: print(json.dumps({'jsonrpc':'2.0','id':request['id'],'error':{'code':-32000,'message':kind,'data':{'kind':kind,'hint':'inspect target'}}}))
 else: print(json.dumps({'error':{'kind':kind,'message':kind,'hint':'inspect target'}}),file=sys.stderr)
 sys.exit(3)
def get(t):
 if t=='t7': return parent
 for v in ts.values():
  if v['address']==t or (v['name']==t and v['state']=='live'): return v
 err('not_found')
cmd=a[0]
request=None
def terminal(t):
 return dict(dict(argv=[],cwd=str(root),created_ms=0,screen='exact',cols=80,rows=24,labels={}),**t)
parent=terminal(parent)
def emit(v):
 if request and request['method']=='terminal.get':
  if os.environ.get('TEST_BROKEN')=='missing':
   v=dict(v);del v['cwd']
  elif os.environ.get('TEST_BROKEN')=='wrapped': v={'result':v}
 print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':v} if request else v))
if cmd=='connect':
 request=json.loads(sys.stdin.readline())
 if request['method']=='terminal.list':
  assert request['params']=={}
  emit({'hidden_finished':0,'machines':[],'terminals':([] if os.environ.get('TEST_HIDE_PARENT') else [parent])+list(ts.values())})
  sys.exit(0)
 method=request['method']
 p=request['params']
 cmd=method.split('.')[1]
 if cmd=='get': a=['get',p['terminal']]
 elif cmd=='read': a=['read',p['terminal'],'--lines',str(p.get('lines',0))]
 elif cmd=='keys': a=['keys',p['terminal']]+p['keys']
 elif cmd=='send': a=['send',p['terminal'],p['text']]
 elif cmd=='label': a=['label',p['terminal']]+[k+'='+v for k,v in p['labels'].items()]
 elif cmd in ['kill','rm']: a=[cmd,p['terminal']]
 elif method=='machine.list': cmd='machine';a=['machine','ls']
 elif cmd=='exec': a=['exec','--on',p['on'],'--timeout',str(p['timeout']),'--']+p['argv']
 elif cmd=='run': pass
 else: raise AssertionError(method)
 if cmd!='run':
  with open(root/'calls','a') as f: f.write(json.dumps(a)+'\n')
if cmd=='run':
 p=request['params']
 with open(root/'calls','a') as f: f.write(json.dumps(['run',p])+'\n')
 n=p['name']
 if any(t['name']==n and t['state']=='live' for t in ts.values()): err('name_taken')
 i='t'+str(int((root/'counter').read_text())+1 if (root/'counter').exists() else 8)
 (root/'counter').write_text(i[1:])
 if p.get('on'): i=p['on']+'/'+i
 labels=p['labels']
 assert p['argv'][0]==labels['vega.kind'] or p['argv'][0].endswith('hc-toolbox-shell')
 t=terminal({'address':i,'argv':p['argv'],'name':n,'state':'live','pid':int(os.environ['TEST_PARENT_PID']),'status':{'app':labels['vega.kind'],'state':'idle'},'cwd':p['cwd'],'labels':labels,'turn':0})
 ts[i]=t;save()
 if os.environ.get('TEST_UNDELIVERED'): t['first_message']={'sent':False,'reason':'blocked'}
 emit(t)
elif cmd=='exec':
 if os.environ.get('TEST_PREPARE_FAIL'): err('unavailable')
 assert a[a.index('--')+1:a.index('--')+3]==['vega-mux','prepare']
 if (root/'remote-home').exists():
  import subprocess
  e=dict(os.environ, HOME=str(root/'remote-home'), PATH=str(root/'remote-home/bin')+':'+os.environ['PATH'])
  for k in ['HC_TERMINAL_ID','HERDR_PANE_ID','XDG_CONFIG_HOME','XDG_STATE_HOME']: e.pop(k,None)
  r=subprocess.run([os.environ['TEST_MUX_EXE']]+a[a.index('--')+2:],env=e,capture_output=True)
  out,errb,rc=r.stdout,r.stderr,r.returncode
 else: out,errb,rc=(json.dumps({'schema':1,'trust':None})+'\n').encode(),b'',0
 # machine.exec wraps the program's output: base64 stdout/stderr plus its exit code.
 import base64
 emit({'code':rc,'ms':1,'signal':None,'stderr':base64.b64encode(errb).decode(),'stdout':base64.b64encode(out).decode(),'timed_out':False,'truncated':False})
elif cmd=='get': emit(get(a[1]))
elif cmd=='label':
 t=get(a[1])
 for pair in a[2:]:
  k,v=pair.split('=',1)
  if v: t['labels'][k]=v
  else: t['labels'].pop(k,None)
 save();emit(t)
elif cmd=='machine': emit(json.loads(os.environ.get('TEST_MACHINES','{"machines":[]}')))
elif cmd=='read':
 t=get(a[1]);q=os.environ.get('TEST_DIALOG','')
 sel=int((root/'selected').read_text()) if (root/'selected').exists() else 1
 text='recent output'
 if q=='trust': text='Do you trust the files in this folder?\n1. Yes, I trust this folder\n2. No, exit'
 elif q=='question' and not (root/'answered').exists(): text='Which colour?\n'+('❯' if sel==1 else ' ')+' 1. Red\n'+('❯' if sel==2 else ' ')+' 2. Blue\nEnter to select · ↑/↓ to navigate'
 emit({'text':text,'complete':True,'how':'screen'})
elif cmd=='keys':
 get(a[1])
 for k in a[2:]:
  if k=='down': (root/'selected').write_text('2')
  elif k=='enter': (root/'answered').touch()
 emit({'turn':0})
elif cmd=='send':
 t=get(a[1])
 if t['status']['state']=='blocked': err('blocked')
 emit({'turn':0})
elif cmd=='kill':
 t=get(a[1]);t['state']='finished';save();emit(t)
elif cmd=='rm': ts.pop(a[1],None);save();emit({'removed':a[1]})
else: err('unexpected_'+cmd)
