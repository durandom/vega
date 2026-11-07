#!/usr/bin/python3
import json, os, sys
from pathlib import Path
root = Path(os.environ['HOME'])
with (root/'app-calls').open('a') as f:
    f.write(' '.join(sys.argv[1:])+'\n')
if sys.argv[1:3] != ['do', 'space.new']:
    print('{}')
    sys.exit(0)
path = root/'terminals.json'
terminals = json.loads(path.read_text()) if path.exists() else {}
terminals['t0'] = {'address':'t0','name':None,'state':'live','argv':['/bin/zsh','-l'],
    'foreground':{'name':'zsh','argv':['-zsh'],'cwd':str(root/'worktree')},
    'labels':{'herdr.space':'s2','herdr.tab':'tb2'},'status':None}
path.write_text(json.dumps(terminals))
# Real space.new returns its initial shell as well as the new space and tab.
print(json.dumps({'space':'s2','tab':'tb2','terminal':'t0'}))
