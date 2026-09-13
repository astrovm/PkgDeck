"""Behavior checks for the verification entry point; never manage packages."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class VerificationTests(unittest.TestCase):
    def test_help_and_bad_arguments_do_not_start_builds(self):
        for args, expected in [(['--help'], 0), (['unknown'], 2), (['fast', '--unknown'], 2)]:
            result = subprocess.run([str(ROOT / 'scripts/verify.sh'), *args], capture_output=True, text=True)
            self.assertEqual(result.returncode, expected)
            self.assertIn('Usage:', result.stdout + result.stderr)

    def test_failure_is_logged_and_stops_subsequent_stages(self):
        with tempfile.TemporaryDirectory(prefix='pkgdeck-verify-') as directory:
            root = Path(directory)
            cargo = root / 'cargo'
            cargo.write_text('#!/bin/sh\necho synthetic-format-error\nexit 23\n')
            cargo.chmod(0o755)
            env = dict(os.environ, PATH=f'{root}:' + os.environ['PATH'], PKGDECK_LOG_ROOT=str(root / 'logs'))
            result = subprocess.run([str(ROOT / 'scripts/verify.sh'), 'fast'], env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 23)
            self.assertIn('FAIL format', result.stderr)
            self.assertIn('synthetic-format-error', (root / 'logs/latest/format.log').read_text())
            self.assertFalse((root / 'logs/latest/tests.log').exists())

    def test_timeout_is_reported_and_logged(self):
        with tempfile.TemporaryDirectory(prefix='pkgdeck-timeout-') as directory:
            root = Path(directory)
            cargo = root / 'cargo'
            cargo.write_text('#!/bin/sh\necho synthetic-slow-stage\nexec sleep 30\n')
            cargo.chmod(0o755)
            env = dict(os.environ, PATH=f'{root}:' + os.environ['PATH'], PKGDECK_LOG_ROOT=str(root / 'logs'), PKGDECK_STAGE_TIMEOUT='0.1s')
            result = subprocess.run([str(ROOT / 'scripts/verify.sh'), 'fast'], env=env, capture_output=True, text=True, timeout=5)
            self.assertEqual(result.returncode, 124)
            self.assertIn('synthetic-slow-stage', (root / 'logs/latest/format.log').read_text())


if __name__ == '__main__':
    unittest.main()
