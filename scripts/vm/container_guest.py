#!/usr/bin/env python3
"""Real package tests in a disposable rootless Podman container; no desktop auth."""
import os
from pathlib import Path
import subprocess
import lifecycle

assert os.geteuid() == 0 and Path('/run/.containerenv').exists()
assert Path('/etc/pkgdeck-disposable-container').is_file()
assert Path('/etc/pkgdeck-prepared').is_file()
subprocess.run(['useradd', '-m', 'pkgdeck-test'], check=True)
sudoers = Path('/etc/sudoers.d/pkgdeck-fixture')
sudoers.write_text('pkgdeck-test ALL=(root) NOPASSWD: /usr/bin/apt-get\n')
sudoers.chmod(0o440)
lifecycle.apt_fixture()
lifecycle.apt()
lifecycle.homebrew()
print('PKGDECK_CONTAINER_PASS', flush=True)
