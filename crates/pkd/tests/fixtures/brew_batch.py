#!/usr/bin/python3
"""Synthetic batch manager; writes only its test HOME."""
import json
import os
from pathlib import Path
import sys
import time

home = Path(os.environ['HOME'])
path = home / 'state.json'
state = json.loads(path.read_text())
args = sys.argv[1:]
if args == ['--prefix']:
    print(home)
elif args[0] == 'info':
    if (home / 'query-fails').exists():
        sys.exit('synthetic query failure')
    print(json.dumps({'formulae': [dict(full_name=name, desc='Synthetic batch fixture',
        homepage='', versions={'stable': '2'}, revision=0,
        installed=[{'version': version}], outdated=version != '2', dependencies=[])
        for name, version in state.items() if '--installed' in args or name in args]}))
elif args[0] == 'upgrade':
    assert args[1:4] == ['--formula', '--', args[-1]], args
    name = args[-1]
    assert name in state and state[name] != '2', args
    with (home / 'attempts').open('a') as log:
        log.write(name + '\n')
    if (home / 'slow').exists():
        (home / 'started').touch()
        time.sleep(2)
    if name == 'fixture-b' and (home / 'fail').exists():
        sys.exit('synthetic upgrade failure')
    state[name] = '2'
    path.write_text(json.dumps(state))
else:
    raise AssertionError(args)
