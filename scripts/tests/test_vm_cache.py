"""Exercise cache reuse and failed preparation using synthetic disk contents."""
import hashlib
import importlib.util
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('vm_launcher', ROOT / 'scripts/test-host-vm.py')
vm = importlib.util.module_from_spec(spec)
spec.loader.exec_module(vm)


class VMCacheTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='pkgdeck-cache-test-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        cache = self.root / 'cache'
        cache.mkdir()
        (cache / 'base.img').write_bytes(b'synthetic cloud image')
        binaries = self.root / 'target/debug'
        (binaries / 'examples').mkdir(parents=True)
        for name in ['pkd', 'examples/apt-probe']:
            (binaries / name).touch()
        for patch in [
            mock.patch.dict(os.environ, {'PKGDECK_VM_CACHE': str(cache), 'CARGO_TARGET_DIR': str(self.root / 'target')}),
            mock.patch.object(vm, 'IMAGE_SHA256', hashlib.sha256(b'synthetic cloud image').hexdigest()),
            mock.patch.object(vm.os, 'uname', return_value=SimpleNamespace(machine='x86_64')),
            mock.patch.object(vm.shutil, 'which', return_value='/synthetic/tool'),
            mock.patch.object(vm.subprocess, 'run', side_effect=self.create_disk),
        ]:
            patch.start()
            self.addCleanup(patch.stop)
        self.cache = cache

    @staticmethod
    def create_disk(args, **_kwargs):
        # Only disk creation is replaced; cache validation and cleanup run normally.
        output = args[-2] if args[-1] == '12G' else args[-1]
        Path(output).write_bytes(b'synthetic prepared disk')

    def test_failed_preparation_never_publishes_a_cache_entry(self):
        with mock.patch.object(vm, 'boot', side_effect=RuntimeError('synthetic preparation failure')):
            with self.assertRaisesRegex(RuntimeError, 'synthetic preparation failure'):
                vm.main()
        self.assertEqual(list(self.cache.glob('prepared-*')), [])

    def test_reuse_and_corruption_trigger_the_correct_preparation(self):
        with mock.patch.object(vm, 'boot') as boot:
            vm.main()
            self.assertEqual([call.args[2] for call in boot.call_args_list], ['prepare', 'lifecycle'])
            boot.reset_mock()
            vm.main()
            self.assertEqual([call.args[2] for call in boot.call_args_list], ['lifecycle'])
            next(self.cache.glob('prepared-*.qcow2')).write_bytes(b'corrupted synthetic image')
            boot.reset_mock()
            vm.main()
            self.assertEqual([call.args[2] for call in boot.call_args_list], ['prepare', 'lifecycle'])
        self.assertEqual(list(self.cache.glob('*.part')), [])


if __name__ == '__main__':
    unittest.main()
