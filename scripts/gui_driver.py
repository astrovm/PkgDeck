#!/usr/bin/env python3
"""Exercise the real GUI under an isolated X server; never uses the user's display."""
import contextlib
import os
from pathlib import Path
import subprocess
import tempfile
import time


class Desktop:
    def __init__(self):
        read, write = os.pipe()
        self.server = subprocess.Popen(['Xvfb', '-displayfd', str(write), '-screen', '0', '1280x900x24', '-nolisten', 'tcp', '-ac'], pass_fds=(write,), stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        os.close(write)
        import select
        if not select.select([read], [], [], 10)[0]:
            self.server.terminate()
            self.server.wait(timeout=5)
            os.close(read)
            raise AssertionError('Xvfb did not start')
        with os.fdopen(read) as stream:
            number = stream.readline().strip()
        if not number.isdigit():
            raise AssertionError(self.server.communicate(timeout=5)[1].decode())
        self.display = ':' + number
        self.env = dict(os.environ, DISPLAY=self.display, QT_QPA_PLATFORM='xcb', QT_QUICK_BACKEND='software', QT_ACCESSIBILITY= '0')
        self.env.pop('WAYLAND_DISPLAY', None)
        self.directory = tempfile.TemporaryDirectory(prefix='pkgdeck-gui-')
        self.env['XDG_CONFIG_HOME'] = self.directory.name
        self.process = None
        self.window = None

    def xdo(self, *args):
        return subprocess.run(['xdotool', *args], env=self.env, check=True, capture_output=True, text=True, timeout=10).stdout.strip()

    def launch(self, command, env=None):
        self.log = open(Path(self.directory.name) / 'application.log', 'w+')
        self.process = subprocess.Popen(command, env=dict(self.env, **(env or {})), stdout=self.log, stderr=self.log)
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            assert self.process.poll() is None, self.logs()
            result = subprocess.run(['xdotool', 'search', '--name', '^PkgDeck'], env=self.env, capture_output=True, text=True, timeout=5)
            if result.returncode == 0:
                self.window = result.stdout.splitlines()[0]
                self.xdo('windowfocus', '--sync', self.window)
                self.idle()
                return
            time.sleep(0.05)
        raise AssertionError('No GUI window: ' + self.logs())

    def logs(self):
        self.log.flush()
        self.log.seek(0)
        return self.log.read()

    def idle(self):
        # Require consecutive idle frames; this also lets queued key events run.
        deadline = time.monotonic() + 240
        stable = 0
        time.sleep(0.15)
        while time.monotonic() < deadline:
            assert self.process.poll() is None, self.logs()
            title = self.xdo('getwindowname', self.window)
            stable = 0 if 'Working' in title else stable + 1
            if stable >= 5:
                return
            time.sleep(0.05)
        raise AssertionError('GUI worker timed out: ' + self.logs())

    def key(self, key):
        self.xdo('key', '--clearmodifiers', key)
        self.idle()

    def search(self, name):
        self.key('ctrl+f')
        self.xdo('type', '--clearmodifiers', '--delay', '0', name)
        self.key('Return')
        self.key('ctrl+l')
        self.key('Down')

    def write(self, operation, name=''):
        if operation == 'update':
            self.key('ctrl+5'); self.key('ctrl+l'); self.key('Down')
            self.key('ctrl+m')
        else:
            self.search(name)
            self.key({'install': 'ctrl+i', 'remove': 'ctrl+d', 'upgrade': 'ctrl+u'}[operation])
        self.key('alt+y')

    def close(self):
        try:
            if self.process and self.process.poll() is None:
                # Exercise the application close path without needing a window manager.
                self.xdo('key', '--clearmodifiers', 'ctrl+q')
                self.process.wait(timeout=10)
            if self.process:
                assert self.process.returncode == 0, self.logs()
                output = self.logs()
                assert 'TypeError:' not in output and 'ReferenceError:' not in output, output
        finally:
            if self.process and self.process.poll() is None:
                self.process.kill(); self.process.wait(timeout=5)
            if hasattr(self, 'log'): self.log.close()
            self.server.terminate(); self.server.wait(timeout=5)
            self.directory.cleanup()


@contextlib.contextmanager
def desktop(command, env=None):
    gui = Desktop()
    try:
        gui.launch(command, env)
        yield gui
    finally:
        gui.close()


def synthetic(command):
    import json
    with tempfile.TemporaryDirectory(prefix='pkgdeck-gui-fixture-') as directory:
        fixture = Path(directory)
        brew = fixture / 'brew'
        brew.write_text((Path(__file__).resolve().parents[1] / 'crates/pkd/tests/fixtures/brew.py').read_text())
        brew.chmod(0o755)
        env = dict(HOME=directory, PATH=directory, SNAP='', FLATPAK_ID='')
        # Empty sandbox variables still count as markers; explicitly remove them.
        command = ['/usr/bin/env', '-u', 'SNAP', '-u', 'FLATPAK_ID', *command, '--from', 'homebrew', '--auth', 'sudo']
        with desktop(command, env) as gui:
            for operation, installed in [('install', '1.0'), ('update', '1.0'), ('upgrade', '2.0'), ('remove', None)]:
                gui.write(operation, 'fixture')
                assert (fixture / 'state.json').exists(), (operation, gui.logs())
                state = json.loads((fixture / 'state.json').read_text())
                assert state['installed'] == installed, (operation, state, gui.logs())
            gui.key('ctrl+3'); gui.key('ctrl+4')
            gui.xdo('windowsize', gui.window, '400', '520')
            gui.key('ctrl+f')
            gui.xdo('windowsize', gui.window, '1100', '760')
            (fixture / 'fail').touch()
            gui.search('fixture')
            (fixture / 'fail').unlink()
            # Cancel a bounded synthetic read while the GUI remains responsive.
            brew.write_text('#!/usr/bin/python3\nimport time\ntime.sleep(30)\n')
            gui.xdo('key', '--clearmodifiers', 'ctrl+r')
            time.sleep(0.3)
            gui.key('Escape')


if __name__ == '__main__':
    import sys
    synthetic(sys.argv[1:])
