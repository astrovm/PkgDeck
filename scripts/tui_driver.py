"""Bounded PTY driver shared by terminal and isolated native acceptance tests."""
import codecs
import re
import errno
import fcntl
import os
import pty
import select
import signal
import struct
import termios
import time


class Terminal:
    def __init__(self, command, env=None):
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            fcntl.ioctl(1, termios.TIOCSWINSZ, struct.pack('HHHH', 40, 160, 0, 0))
            os.environ.update(env or {})
            os.environ['TERM'] = 'xterm-256color'
            os.execvp(command[0], command)
        self.before = termios.tcgetattr(self.fd)
        self.output = b''
        self.pending = ''
        self.decoder = codecs.getincrementaldecoder('utf-8')('replace')
        self.screen = [[' '] * 160 for _ in range(40)]
        self.row = self.col = 0
        self.reaped = False

    def send(self, keys):
        self.output = b''
        os.write(self.fd, keys)

    def wait(self, text, timeout=240):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if text in '\n'.join(''.join(row) for row in self.screen):
                return
            if select.select([self.fd], [], [], 0.1)[0]:
                try:
                    chunk = os.read(self.fd, 65536)
                    self.output += chunk
                    self.feed(self.decoder.decode(chunk))
                except OSError as error:
                    if error.errno != errno.EIO:
                        raise
                    break
        raise AssertionError(f'Missing {text!r}: {self.output[-12000:]!r}')

    def feed(self, text):
        # Ratatui uses cursor positioning and SGR; preserve blank cells skipped
        # by differential redraws instead of matching escape-filled byte output.
        self.pending += text
        while self.pending:
            if self.pending.startswith('\x1b'):
                match = re.match(r'\x1b\[([0-9;?]*)([@-~])', self.pending)
                if not match:
                    return
                params, command = match.groups()
                values = [int(x or 0) for x in params.lstrip('?').split(';')]
                if command in ('H', 'f'):
                    self.row = max(0, (values[0] or 1) - 1)
                    self.col = max(0, (values[1] if len(values) > 1 else 1) - 1)
                elif command == 'J' and values[0] in (2, 3):
                    self.screen = [[' '] * 160 for _ in range(40)]
                self.pending = self.pending[match.end():]
                continue
            char, self.pending = self.pending[0], self.pending[1:]
            if char == '\r':
                self.col = 0
            elif char == '\n':
                self.row += 1
            elif ord(char) >= 32:
                if self.row < 40 and self.col < 160:
                    self.screen[self.row][self.col] = char
                self.col += 1

    def close(self):
        try:
            self.send(b'q')
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                done, status = os.waitpid(self.pid, os.WNOHANG)
                if done:
                    self.reaped = True
                    assert os.waitstatus_to_exitcode(status) == 0
                    after = termios.tcgetattr(self.fd)
                    mask = termios.ECHO | termios.ICANON
                    assert self.before[3] & mask == after[3] & mask
                    return
                time.sleep(0.02)
            raise AssertionError('TUI did not exit')
        finally:
            if not self.reaped:
                os.kill(self.pid, signal.SIGKILL)
                os.waitpid(self.pid, 0)
            os.close(self.fd)


def write(command, operation, name):
    terminal = Terminal(command, {'DISPLAY': '', 'WAYLAND_DISPLAY': ''})
    try:
        terminal.wait('Press /', 15)
        if operation == 'update':
            terminal.send(b'4')
            terminal.wait('Select a source')
            terminal.send(b'u')
            terminal.wait('Confirm Refresh')
        else:
            terminal.send(b'/' + name.encode() + b'\r')
            terminal.wait('1 packages')
            terminal.send(b'\r')
            terminal.wait('Homepage:')
            terminal.send({'install': b'i', 'upgrade': b'g', 'remove': b'd'}[operation])
            terminal.wait('Confirm ')
        terminal.send(b'y')
        terminal.wait('Completed.')
    finally:
        terminal.close()
