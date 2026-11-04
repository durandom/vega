# Fake supported Claude plugin CLI, only for temp-HOME unit tests.
import json
import os
import pathlib
import sys

home = pathlib.Path(os.environ['HOME'])
assert os.environ['CLAUDE_CONFIG_DIR'] == str(home / '.claude')
assert home.name == 'home'
args = sys.argv[1:]
with (home / 'claude-calls').open('a') as log:
    log.write(' '.join(args) + '\n')
if (home / 'fail-claude').exists():
    sys.exit(1)
state = home / 'fake-plugins.json'
items = json.loads(state.read_text()) if state.exists() else []
if args == ['plugin', 'list', '--json']:
    print(json.dumps(items))
elif args[:3] == ['plugin', 'marketplace', 'add']:
    pass
elif args[:2] in [['plugin', 'install'], ['plugin', 'update'], ['plugin', 'enable']]:
    plugin = home / '.local/share/vega/claude-marketplace/plugins/vega-hooks'
    items = [{'id': 'vega-hooks@vega-local', 'scope': 'user', 'enabled': True,
              'installPath': str(plugin)}]
    if (home / 'inactive-claude').exists():
        items[0]['enabled'] = False
    state.write_text(json.dumps(items))
    settings = home / '.claude/settings.json'
    value = json.loads(settings.read_text()) if settings.exists() else {}
    value.setdefault('enabledPlugins', {})['vega-hooks@vega-local'] = True
    value['cliWritten'] = True
    settings.write_text(json.dumps(value))
else:
    sys.exit(2)
