#!/usr/bin/env python3
"""Run Qt Quick tests without writing the developer's settings."""
import os
from pathlib import Path
import subprocess
import tempfile

with tempfile.TemporaryDirectory(prefix='pkgdeck-qml-tests-') as directory:
    result = subprocess.run(['qmltestrunner', '-input', str(Path(__file__).resolve().parents[1] / 'crates/pkgdeck/tests/qml')],
                            env=dict(os.environ, XDG_CONFIG_HOME=directory, XDG_DATA_HOME=directory, XDG_DATA_DIRS=directory, QT_QPA_PLATFORM='offscreen', QT_QUICK_BACKEND='software'), timeout=60)
    raise SystemExit(result.returncode)
