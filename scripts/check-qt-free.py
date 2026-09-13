#!/usr/bin/env python3
"""Check the actual terminal dependency graph and executable for Qt leakage."""
import os
from pathlib import Path
import subprocess

tree = subprocess.check_output(['cargo', 'tree', '--locked', '-p', 'pkd'], text=True)
assert 'cxx-qt' not in tree and 'qt-build' not in tree, 'Qt dependency leaked into pkd'
binary = Path(os.environ.get('CARGO_TARGET_DIR', 'target')) / 'debug/pkd'
links = subprocess.check_output(['ldd', str(binary)], text=True)
assert 'qt' not in links.lower(), 'pkd links Qt'
print('Terminal dependency graph and binary are Qt-free')
