#!/usr/bin/env python3
"""Behavioral smoke checks; uses disposable PTYs and never manages packages."""
import argparse
import errno
import fcntl
import os
import pty
import select
import signal
import struct
import subprocess
import termios
import time
import tempfile
from pathlib import Path


def terminal(command):
    # Either redirected stream must prevent TUI startup, even if the other is a TTY.
    master, slave = pty.openpty()
    try:
        for stdin, stdout in ((subprocess.DEVNULL, slave), (slave, subprocess.PIPE)):
            result = subprocess.run(command, stdin=stdin, stdout=stdout,
                                    stderr=subprocess.PIPE, timeout=10)
            assert result.returncode == 2, result.stderr
            assert b"requires a terminal" in result.stderr, result.stderr
    finally:
        os.close(slave)
        os.close(master)
    for key in (b"q", b"\x1b", b"\x03", None):
        pid, master = pty.fork()
        if pid == 0:
            fcntl.ioctl(1, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))
            os.environ["TERM"] = "xterm-256color"
            os.execvp(command[0], command)
        before = termios.tcgetattr(master)
        output = b""
        deadline = time.monotonic() + 15
        sent = False
        resized = False
        input_started = False
        reaped = False
        try:
            while time.monotonic() < deadline:
                ready, _, _ = select.select([master], [], [], 0.1)
                if ready:
                    try:
                        output += os.read(master, 65536)
                    except OSError as error:
                        if error.errno != errno.EIO:
                            raise
                if b"Search" in output and b"\x1b[?25l" in output and not input_started:
                    # Wait for a key-driven redraw before resizing: this proves the
                    # event reader has installed its signal handler and consumed input.
                    os.write(master, b" ")
                    input_started = True
                    output = b""
                elif input_started and not resized and b"\x1b[?25l" in output:
                    # TIOCSWINSZ sends SIGWINCH to the foreground process group.
                    fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
                    resized = True
                elif resized and not sent and b"\x1b[2J" in output and b"\x1b[?25l" in output.rsplit(b"\x1b[2J", 1)[1]:
                    if key is None:
                        os.kill(pid, signal.SIGTERM)
                    else:
                        os.write(master, key)
                    sent = True
                done, status = os.waitpid(pid, os.WNOHANG)
                if done:
                    reaped = True
                    assert os.waitstatus_to_exitcode(status) == 0, output
                    assert sent and b"Results" in output and b"PkgDeck" in output, output
                    after = termios.tcgetattr(master)
                    mask = termios.ECHO | termios.ICANON
                    assert before[3] & mask == after[3] & mask, "Terminal mode was not restored"
                    break
            else:
                raise AssertionError(f"TUI did not exit after {key!r} (sent={sent}): {output!r}")
        finally:
            if not reaped:
                os.kill(pid, signal.SIGKILL)
                os.waitpid(pid, 0)
            os.close(master)


def terminal_interactions(command):
    from tui_driver import Terminal
    terminal = Terminal(command, {'SNAP': '/synthetic-disabled-runtime'})
    try:
        terminal.wait('Press /', 15)
        terminal.send(b'4')
        terminal.wait('Select a source', 15)
        terminal.send(b'\x1b[B')
        terminal.wait('> homebrew', 15)
        terminal.send(b'/synthetic\r')
        terminal.wait('PkgDeck - Search', 15)
        terminal.wait('disabled', 15)
    finally:
        terminal.close()

    # A synthetic executable exercises worker dispatch without touching a real
    # package manager. Its state lives only in this temporary directory.
    from tui_driver import write
    with tempfile.TemporaryDirectory(prefix='pkgdeck-tui-') as directory:
        brew = Path(directory) / 'brew'
        fixture = Path(__file__).resolve().parents[1] / 'crates/pkd/tests/fixtures/brew.py'
        brew.write_text(fixture.read_text())
        brew.chmod(0o755)
        invocation = ['/usr/bin/env', '-u', 'SNAP', '-u', 'FLATPAK_ID',
                      f'HOME={directory}', f'PATH={directory}', *command, '--from', 'homebrew', '--arch', os.uname().machine]
        for operation in ('install', 'update', 'upgrade', 'remove'):
            write(invocation, operation, 'fixture')
        brew.write_text("#!/usr/bin/python3\nimport time, pathlib\npathlib.Path(" + repr(str(Path(directory) / 'started')) + ").touch()\ntime.sleep(30)\n")
        terminal = Terminal(invocation)
        try:
            terminal.wait('Press /', 15)
            terminal.send(b'/fixture\r')
            deadline = time.monotonic() + 10
            while not (Path(directory) / 'started').exists():
                assert time.monotonic() < deadline
                time.sleep(0.02)
            terminal.send(b'\x1b')
            terminal.wait('cancelled', 15)
        finally:
            terminal.close()


def gui(command):
    with tempfile.TemporaryDirectory(prefix="pkgdeck-gui-smoke-") as directory:
        result = subprocess.run(command + ["--smoke-test"], env=dict(os.environ, XDG_CONFIG_HOME=directory, SNAP="/synthetic-disabled-runtime"), capture_output=True, text=True, timeout=30)
    assert result.returncode == 0, result.stderr
    assert "PKGDECK_GUI_READY" in result.stderr, result.stderr
    assert "failed to load" not in result.stderr.lower(), result.stderr


def gui_failure(command):
    with tempfile.TemporaryDirectory(prefix="pkgdeck-qml-fixture-") as directory:
        module = Path(directory) / "org/kde/kirigami"
        module.mkdir(parents=True)
        (module / "qmldir").write_text("module org.kde.kirigami\nApplicationWindow 1.0 Broken.qml\n")
        (module / "Broken.qml").write_text("import QtQuick\nItem { pkgdeckMissingProperty: true }\n")
        env = dict(os.environ, QML_IMPORT_PATH=directory + ":" + os.environ.get("QML_IMPORT_PATH", ""))
        result = subprocess.run(command, env=env, capture_output=True, text=True, timeout=15)
        assert result.returncode == 1, result.stderr
        assert "failed to load component" in result.stderr, result.stderr


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("kind", choices=("terminal", "terminal-interactions", "gui", "gui-failure"))
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    assert args.command
    {"terminal": terminal, "terminal-interactions": terminal_interactions, "gui": gui, "gui-failure": gui_failure}[args.kind](args.command)
