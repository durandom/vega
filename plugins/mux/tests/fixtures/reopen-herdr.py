#!/usr/bin/env python3
import sys, os, json
from pathlib import Path

args = sys.argv[1:]
root = Path(os.environ['XDG_STATE_HOME'])
kind = os.environ['VEGA_TEST_KIND']
live = root / 'running'

def emit(value):
    print(json.dumps({'result': value}))

def fail(code):
    print(json.dumps({'error': {'code': code, 'message': 'fixture failure'}}))
    sys.exit(1)

agent = json.loads(live.read_text()) if live.exists() else None
if args[:2] == ['agent', 'get']:
    if not agent or args[2] not in [agent['name'], agent['pane_id']]:
        fail('agent_not_found')
    emit({'agent': agent})
elif args[:2] == ['agent', 'list']:
    emit({'agents': [agent] if agent else []})
elif args[:2] == ['pane', 'get']:
    if not agent or args[2] != agent['pane_id']:
        fail('pane_not_found')
    if os.environ['VEGA_TEST_RESUME'] == '0' and os.environ['VEGA_TEST_DIALOG'] == '0' and not (root / 'prompted').exists():
        agent.pop('agent_session', None)
    emit({'pane': agent})
elif args[:2] == ['pane', 'list']:
    emit({'panes': [{'pane_id': 'w0:p1', 'workspace_id': 'w0', 'agent': 'codex'}]})
elif args[:2] == ['pane', 'process-info']:
    emit({'process_info': {'shell_pid': int(os.environ['VEGA_TEST_PID'])}})
elif args[:2] == ['pane', 'read']:
    print('Ready')
elif args[:2] == ['workspace', 'get']:
    emit({'workspace': {}})
elif args[:2] == ['pane', 'close']:
    key = 'local_' + args[2].replace(':', '_')
    assert not (root / 'vega/parents' / key).exists()
    assert not (root / 'vega/modes' / key).exists()
    live.unlink(missing_ok=True)
    emit({})
elif args[:2] == ['agent', 'prompt']:
    (root / 'prompted').touch()
    emit({})
elif args[:2] == ['agent', 'wait']:
    emit({})
elif args[:2] == ['workspace', 'create']:
    assert Path(args[args.index('--cwd') + 1]).resolve() == root.resolve()
    pane = os.environ['VEGA_TEST_NEXT_PANE']
    emit({'workspace': {'workspace_id': pane.split(':')[0]}, 'root_pane': {'pane_id': pane}})
elif args[:2] == ['agent', 'start']:
    pane = args[args.index('--pane') + 1]
    key = 'local_' + pane.replace(':', '_')
    expected = json.loads(os.environ['VEGA_TEST_PARENT'])
    parent_file = root / 'vega/parents' / key
    if expected:
        parent = json.loads(parent_file.read_text())
        assert parent['parent'] == expected['pane'] and parent['host'] == expected['host']
    else:
        assert not parent_file.exists()
    mode = json.loads((root / 'vega/modes' / key).read_text())
    assert mode['mode'] == os.environ['VEGA_TEST_MODE']
    tail = args[args.index('--') + 1:]
    if os.environ['VEGA_TEST_RESUME'] == '1':
        assert tail[-2:] == (['resume', 'saved-session'] if kind == 'codex' else ['--resume', 'saved-session'])
    else:
        assert 'resume' not in tail and '--resume' not in tail
    live.write_text(json.dumps({'name': 'fix', 'pane_id': pane, 'workspace_id': pane.split(':')[0],
                               'agent': kind, 'agent_session': {'value': 'saved-session'}}))
    if os.environ['VEGA_TEST_DIALOG'] == '1':
        fail('timeout')
    emit({})
else:
    raise Exception('unexpected call: ' + repr(args))
