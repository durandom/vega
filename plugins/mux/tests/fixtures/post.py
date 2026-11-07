#!/usr/bin/python3
# Mirror the real post qualified-address guard; capture mux -> post argv.
import json, os, sys
from pathlib import Path
root=Path(os.environ['HOME'])
a=sys.argv[1:]
with open(root/'post-calls','a') as f: f.write(json.dumps(a)+'\n')
assert a[:2]==['post','send']
host=a[a.index('--host')+1] if '--host' in a else None
target=a[-2]
if '/' in target and host is None:
 print('error{code,message,hint}:\n  usage,qualified address needs registry host,use --host')
 sys.exit(2)
(root/('mail-'+(host or 'local')+'-'+target.rsplit('/',1)[-1])).write_text(a[-1])
print('sent: queued')
