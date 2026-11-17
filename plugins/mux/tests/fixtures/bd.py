#!/usr/bin/python3
import json, os, pathlib, sys, time
root = pathlib.Path(os.environ['HOME'])
with (root / 'bd-calls').open('a') as f:
    f.write(json.dumps(sys.argv[1:]) + '\n')
(root / 'bd-cwd').write_text(os.getcwd())
mode = os.environ.get('TEST_BD_MODE', '')
if mode == 'hang':
    time.sleep(30)
if mode == 'fail' or (mode == 'update-fail' and sys.argv[1] == 'update'):
    sys.exit(1)
if sys.argv[1] == 'show':
    assert sys.argv[1:] == ['show', 'work-1', '--json']
    if mode == 'invalid':
        print('invalid')
    else:
        print(json.dumps([] if mode == 'missing' else [dict(id='work-1', issue_type=os.environ.get('TEST_BD_TYPE', 'task'), labels=json.loads(os.environ.get('TEST_BD_LABELS', '[]')))]))
else:
    assert sys.argv[1:4] == ['update', 'work-1', '--append-notes']
    print('{}')
