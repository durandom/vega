# Executed by an explicit Python interpreter, never a real core.
import json, os, pathlib, sys, time
home = pathlib.Path(os.environ['HOME'])
args = sys.argv[1:]
if args[0] == '--json': args.pop(0)
def emit(v): print(json.dumps(v), flush=True)
def load(): return json.loads((home/'hc.json').read_text())
def get(p):
    for t in load()['terminals']:
        if t['address'] == p:
            t['pid'] = int(os.environ['FAKE_PID'])
            return t
    emit({'error': {'kind':'not_found', 'message':'closed', 'hint':'list'}})
    sys.exit(3)
if args[0] == 'ls': emit(load())
elif args[0] == 'get': emit(get(args[1]))
elif args[0] == 'read': emit({'text':'completed work', 'complete':True, 'how':'screen'})
elif args[0] == 'send':
    assert '--force' not in args
    p, text = args[-2:]
    t = get(p)
    state = t.get('status',{}).get('state')
    # Simulate a status change between get and send.
    if (home/'race').exists(): state = (home/'race').read_text()
    if state == 'blocked' or ('--if' in args and state != args[args.index('--if')+1]):
        emit({'error':{'kind':'blocked' if state == 'blocked' else 'status_mismatch','message':'changed','hint':'wait'}})
        sys.exit(9)
    with (home/'prompts').open('a') as f: f.write(json.dumps({'pane':p,'args':args,'text':text})+'\n')
    emit({'started':True,'status':state,'turn':1})
elif args[0] == 'events':
    if (home/'poll-mode').exists(): sys.exit(1)
    # Parent-death check ensures test cleanup cannot leave a fake watcher.
    parent = os.getppid()
    emit(dict(load(), seq=0))
    cursor = home/'cursor'
    offset = int(cursor.read_text()) if cursor.exists() else 0
    while os.getppid() == parent:
        lines = (home/'events').read_text().splitlines() if (home/'events').exists() else []
        for line in lines[offset:]:
            offset += 1
            cursor.write_text(str(offset))
            emit(json.loads(line))
        time.sleep(.05)
else:
    sys.exit(1)
