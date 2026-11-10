#!/usr/bin/python3
import json, os, sys
from pathlib import Path
root = Path(os.environ['HOME'])
with (root/'app-calls').open('a') as f:
    f.write(' '.join(sys.argv[1:])+'\n')
if (root/'app-unavailable').exists():
    if (root/'app-unavailable').read_text() == 'structured':
        print(json.dumps({'error': {'code': 'no_window'}}))
    else:
        print('no Herdr window at '+str(root/'config/herdr-app/herdr-app.sock'), file=sys.stderr)
    sys.exit(1)
if sys.argv[1:] == ['ls'] and (root/'app-ls.json').exists():
    print((root/'app-ls.json').read_text())
    sys.exit(0)
if sys.argv[1:3] == ['do', 'space.select'] and (root/'app-select-fails').exists():
    print('{"error":"no such space"}')
    sys.exit(1)
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
