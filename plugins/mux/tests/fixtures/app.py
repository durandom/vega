#!/usr/bin/python3
import json, os, sys
from pathlib import Path
root = Path(os.environ['HOME'])
with (root/'app-calls').open('a') as f:
    f.write(' '.join(sys.argv[1:])+'\n')
if (root/'app-unavailable').exists() and not os.environ.get('TEST_REMOTE_APP'):
    if (root/'app-unavailable').read_text() == 'structured':
        print(json.dumps({'error': {'code': 'no_window'}}))
    else:
        print('no Herdr window at '+str(root/'config/herdr-app/herdr-app.sock'), file=sys.stderr)
    sys.exit(1)
if sys.argv[1:] == ['ls'] and (root/'app-ls.json').exists():
    snapshot = json.loads((root/'app-ls.json').read_text())
    ts = json.loads((root/'terminals.json').read_text()) if (root/'terminals.json').exists() else {}
    if not os.environ.get('TEST_NO_ADOPTION'):
        for t in ts.values():
            labels=t.get('labels',{})
            if 'herdr.tab' not in labels: continue
            for space in snapshot.get('spaces',[]):
                if space['id'].split('/')[-1]==labels.get('herdr.space'):
                    space.setdefault('tabs',[]).append({'id':labels['herdr.tab'],'terminals':[{'terminal':t['address']}]})
    print(json.dumps(snapshot))
    sys.exit(0)
if sys.argv[1:3] == ['do', 'space.select'] and (root/'app-select-fails').exists():
    print('{"error":"no such space"}')
    sys.exit(1)
if sys.argv[1:3] in [['do', 'space.new'], ['do', 'tab.new']] and len(sys.argv)==4:
    failure=(root/'app-failure').read_text() if (root/'app-failure').exists() else ''
    if failure=='mutation':
        print('{"error":"placement failed"}'); sys.exit(1)
    if failure=='wrong-namespace':
        print('{"space":"other/s1","tab":"other/tb1"}'); sys.exit(0)
    args=json.loads(sys.argv[3])
    assert args['focus'] is False
    snapshot=json.loads((root/'app-ls.json').read_text())
    space=args.get('space', (args.get('machine','')+'/' if args.get('machine') else '')+'s2')
    machine=space.split('/')[0] if '/' in space else ''
    prefix=machine+'/' if machine else ''
    if 'space' not in args:
        snapshot['spaces'].append({'id':space,'machine':args['machine'],'folder':args['folder'],'tabs':[]})
        (root/'app-ls.json').write_text(json.dumps(snapshot))
    path=root/'terminals.json'
    terminals=json.loads(path.read_text()) if path.exists() else {}
    address=prefix+'t0'
    stored_address=address if '/' in address else os.environ.get('TEST_REMOTE_APP','remote')+'/'+address
    terminals[stored_address]={'address':stored_address,'name':None,'state':'live','argv':['pwsh.exe' if ':' in args['folder'] else '/bin/zsh'],
      'created_ms':0,'screen':'exact','cols':80,'rows':24,'cwd':args['folder'],
      'labels':{'herdr.space':space.split('/')[-1],'herdr.tab':'tb2'},'status':None}
    path.write_text(json.dumps(terminals))
    result={'tab':prefix+'tb2','terminal':address}
    if sys.argv[2]=='space.new': result['space']=space
    print(json.dumps(result))
    sys.exit(0)
if sys.argv[1:3] != ['do', 'space.new']:
    print('{}')
    sys.exit(0)
path = root/'terminals.json'
terminals = json.loads(path.read_text()) if path.exists() else {}
terminals['t0'] = {'address':'t0','name':None,'state':'live','argv':['/bin/zsh','-l'],
    'created_ms':0,'screen':'exact','cols':80,'rows':24,'cwd':str(root/'worktree'),
    'foreground':{'pid':1,'name':'zsh','argv':['-zsh'],'cwd':str(root/'worktree')},
    'labels':{'herdr.space':'s2','herdr.tab':'tb2'},'status':None}
path.write_text(json.dumps(terminals))
# Real space.new returns its initial shell as well as the new space and tab.
print(json.dumps({'space':'s2','tab':'tb2','terminal':'t0'}))
