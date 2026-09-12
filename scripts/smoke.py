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


def terminal(command):
    for key in (b"q", b"\x1b", b"\x03"):
        pid, master = pty.fork()
        if pid == 0:
            fcntl.ioctl(1, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))
            os.environ["TERM"] = "xterm-256color"
            os.execvp(command[0], command)
        before = termios.tcgetattr(master)
        output = b""
        deadline = time.monotonic() + 15
        sent = False
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
                if b"foundation." in output and not sent:
                    fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
                    os.kill(pid, signal.SIGWINCH)
                    os.write(master, b"x" + key)
                    sent = True
                done, status = os.waitpid(pid, os.WNOHANG)
                if done:
                    reaped = True
                    assert os.waitstatus_to_exitcode(status) == 0, output
                    assert sent and b"available" in output and b"PkgDeck" in output, output
                    after = termios.tcgetattr(master)
                    mask = termios.ECHO | termios.ICANON
                    assert before[3] & mask == after[3] & mask, "Terminal mode was not restored"
                    break
            else:
                raise AssertionError(f"TUI did not exit: {output!r}")
        finally:
            if not reaped:
                os.kill(pid, signal.SIGKILL)
                os.waitpid(pid, 0)
            os.close(master)


def gui(command):
    result = subprocess.run(command + ["--smoke-test"], capture_output=True, text=True, timeout=30)
    assert result.returncode == 0, result.stderr
    assert "PKGDECK_GUI_READY" in result.stderr, result.stderr
    assert "failed to load" not in result.stderr.lower(), result.stderr


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("kind", choices=("terminal", "gui"))
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    assert args.command
    {"terminal": terminal, "gui": gui}[args.kind](args.command)
