# Executed by an explicit Python interpreter, never a real core.
import json, os, pathlib, sys, time
home = pathlib.Path(os.environ['HOME'])
args = sys.argv[1:]
assert args == ['connect']
request=None
def emit(v): print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':v} if request else v), flush=True)
def error(kind):
    print(json.dumps({'jsonrpc':'2.0','id':request['id'],'error':{'code':-32000,'message':kind,'data':{'kind':kind,'hint':'wait'}}}),flush=True)
    sys.exit(9)
def load(): return json.loads((home/'hc.json').read_text())
def get(p):
    inventory = load()
    if '/' in p and (home/'remote-hc.json').exists():
        assert request['method'] == 'terminal.get'
        with (home/'remote-lookups').open('a') as f: f.write(json.dumps(request)+'\n')
        inventory = json.loads((home/'remote-hc.json').read_text())
        if inventory.get('unavailable'): error('unavailable')
    for t in inventory['terminals']:
        if t['address'] == p:
            t['pid'] = int(os.environ['FAKE_PID'])
            return t
    error('not_found')
if args[0] == 'connect':
    request = json.loads(sys.stdin.readline())
    p=request['params']
    method=request['method']
    if method == 'events.subscribe': args=['events']
    elif method == 'terminal.list':
        emit(dict(load(), hidden_finished=0, machines=[]))
        sys.exit(0)
    elif method == 'terminal.get': args=['get',p['terminal']]
    elif method == 'terminal.read': args=['read',p['terminal']]
    elif method == 'terminal.send': args=['send']+(['--if',p['if']] if 'if' in p else [])+[p['terminal'],p['text']]
    elif method == 'terminal.label': args=['label',p['terminal']]
    else: raise AssertionError(method)
if args[0] == 'get': emit(get(args[1]))
elif args[0] == 'label': emit(get(args[1]))
elif args[0] == 'read':
    screen = home/'hc-screen'
    emit({'text':screen.read_text() if screen.exists() else 'completed work', 'complete':True, 'how':'screen'})
elif args[0] == 'send':
    assert '--force' not in args
    p, text = args[-2:]
    fail = home / ('theme-timeout-' + p)
    if text.startswith('/config theme=') and fail.exists():
        fail.unlink()
        error('timeout')
    t = get(p)
    state = t.get('status',{}).get('state')
    # Simulate a status change between get and send.
    if (home/'race').exists(): state = (home/'race').read_text()
    if state == 'blocked' or ('--if' in args and state != args[args.index('--if')+1]):
        error('blocked' if state == 'blocked' else 'status_mismatch')
    with (home/'prompts').open('a') as f: f.write(json.dumps({'pane':p,'args':args,'text':text})+'\n')
    emit({'started':True,'status':state,'turn':1})
elif args[0] == 'events':
    if (home/'poll-mode').exists(): sys.exit(1)
    # Parent-death check ensures test cleanup cannot leave a fake watcher.
    parent = os.getppid()
    broken=home/'broken-subscription'
    snapshot=dict(load(), seq=0)
    if broken.exists():
        if broken.read_text()=='wrong-id': request['id']+=1
        else: del snapshot['seq']
    emit(snapshot)
    request=None
    cursor = home/'cursor'
    offset = int(cursor.read_text()) if cursor.exists() else 0
    while os.getppid() == parent:
        # Keep a broken subscription alive so silence/EOF cannot mask a missing guard.
        if broken.exists(): emit({'jsonrpc':'2.0','method':'heartbeat','params':{'seq':0}})
        lines = (home/'events').read_text().splitlines(keepends=True) if (home/'events').exists() else []
        for line in lines[offset:]:
            # Rust's Value formatter may append a JSON line in several writes.
            # Only a newline commits the event; do not parse/consume a partial one.
            if not line.endswith('\n'): break
            event = json.loads(line)
            offset += 1
            cursor.write_text(str(offset))
            emit(event)
        time.sleep(.05)
else:
    sys.exit(1)
