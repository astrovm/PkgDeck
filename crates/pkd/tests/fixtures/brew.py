#!/usr/bin/python3
"""Synthetic manager: only writes its test HOME, never invokes a package manager."""
import json
import os
from pathlib import Path
import sys

assert 'LD_PRELOAD' not in os.environ
assert os.environ['HOMEBREW_NO_AUTO_UPDATE'] == '1'
assert os.environ['HOMEBREW_NO_INSTALL_CLEANUP'] == '1'
home = Path(os.environ['HOME'])
state_path = home / 'state.json'
state = json.loads(state_path.read_text()) if state_path.exists() else {'installed': None, 'candidate': '1.0'}
args = sys.argv[1:]
if (home / 'fail').exists():
    print('synthetic native failure', file=sys.stderr)
    sys.exit(1)
if args == ['--prefix']:
    print(home)
elif args == ['formulae']:
    print('fixture')
elif args[0] == 'info':
    formula = dict(full_name='fixture', desc='Synthetic fixture', homepage='',
                   versions={'stable': state['candidate']}, revision=0,
                   installed=[{'version': state['installed']}] if state['installed'] else [],
                   outdated=bool(state['installed'] and state['installed'] != state['candidate']), dependencies=[])
    print(json.dumps({'formulae': [] if '--installed' in args and not state['installed'] else [formula]}))
else:
    if args[0] == 'update':
        assert args == ['update']
        state['candidate'] = '2.0'
    else:
        expected = ['--formula', '--force', '--', 'fixture'] if args[0] == 'uninstall' else ['--formula', '--', 'fixture']
        assert args[1:] == expected, args
        state['installed'] = None if args[0] == 'uninstall' else state['candidate']
    state_path.write_text(json.dumps(state))
    print('synthetic native progress')
