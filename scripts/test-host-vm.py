#!/usr/bin/env python3
"""Cache VM dependencies, then test in a fresh overlay with live stage logs."""
import fcntl
import hashlib
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile
import time
import urllib.request
import uuid

ROOT = Path(__file__).resolve().parent.parent
IMAGE_URL = 'https://cloud-images.ubuntu.com/releases/26.04/release-20260823/ubuntu-26.04-server-cloudimg-amd64.img'
IMAGE_SHA256 = '8196be9d7958059cb56c6c75c80fdf6cee8a8885bc149ea791d7db1c7ef93035'


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def recipe_key(source):
    return hashlib.sha256(IMAGE_SHA256.encode() + source).hexdigest()[:16]


def boot(disk, cache, phase, script, token):
    with tempfile.TemporaryDirectory(prefix='pkgdeck-vm-seed-') as directory:
        work = Path(directory)
        (work / 'meta-data').write_text(f'instance-id: pkgdeck-{phase}-{uuid.uuid4()}\nlocal-hostname: pkgdeck-test\n')
        (work / 'user-data').write_text(f'''#cloud-config
users: []
write_files:
  - path: /etc/pkgdeck-disposable-vm
    content: host-execution-test
runcmd:
  - [mkdir, -p, /mnt/pkgdeck, /mnt/pkgdeck-bin]
  - [mount, -t, 9p, -o, "trans=virtio,version=9p2000.L,ro", pkgdeck, /mnt/pkgdeck]
  - [mount, -t, 9p, -o, "trans=virtio,version=9p2000.L,ro", pkdbin, /mnt/pkgdeck-bin]
  - [sh, -c, "python3 -u /mnt/pkgdeck/scripts/vm/{script} > /dev/virtio-ports/pkgdeck.results 2>&1; poweroff"]
''')
        seed = work / 'seed.iso'
        subprocess.run(['genisoimage', '-quiet', '-output', str(seed), '-volid', 'cidata', '-joliet', '-rock', str(work / 'user-data'), str(work / 'meta-data')], check=True)
        logs = cache / 'logs' / f'{time.time_ns()}-{phase}'
        logs.mkdir(parents=True)
        results = logs / 'results.log'
        binary_dir = Path(os.environ.get('CARGO_TARGET_DIR', ROOT / 'target')).resolve() / 'debug'
        kvm = os.access('/dev/kvm', os.R_OK | os.W_OK)
        command = ['qemu-system-x86_64', '-accel', 'kvm' if kvm else 'tcg', '-cpu', 'host' if kvm else 'max',
                   '-m', '2048', '-smp', '2', '-display', 'none', '-monitor', 'none', '-serial', 'stdio', '-no-reboot',
                   '-drive', f'file={disk},format=qcow2,if=virtio', '-drive', f'file={seed},media=cdrom,readonly=on',
                   '-virtfs', f'local,path={ROOT},mount_tag=pkgdeck,security_model=none,readonly=on',
                   '-virtfs', f'local,path={binary_dir},mount_tag=pkdbin,security_model=none,readonly=on',
                   '-device', 'virtio-serial-pci', '-chardev', f'file,id=results,path={results}',
                   '-device', 'virtserialport,chardev=results,name=pkgdeck.results',
                   '-netdev', 'user,id=net0', '-device', 'virtio-net-pci,netdev=net0']
        print(f'VM {phase}; logs: {logs}', flush=True)
        with (logs / 'serial.log').open('w') as serial:
            child = subprocess.Popen(command, stdout=serial, stderr=subprocess.STDOUT)
            offset = 0
            def drain():
                nonlocal offset
                if results.exists():
                    with results.open('rb') as stream:
                        stream.seek(offset)
                        data = stream.read()
                        offset += len(data)
                    if data:
                        print(data.decode(errors='replace'), end='', flush=True)
            try:
                deadline = time.monotonic() + 900
                while child.poll() is None:
                    drain()
                    if time.monotonic() > deadline:
                        raise TimeoutError(f'VM {phase} exceeded 900 seconds; inspect {logs}')
                    time.sleep(0.25)
                drain()
            finally:
                if child.poll() is None:
                    child.terminate()
                    try:
                        child.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        child.kill()
                        child.wait()
        output = results.read_text(errors='replace') if results.exists() else ''
        if child.returncode != 0 or token not in output or 'PKGDECK_HOST_VM_FAIL' in output:
            raise RuntimeError(f'VM {phase} failed; inspect {logs}')


def main():
    if os.uname().machine != 'x86_64':
        raise SystemExit('The real VM fixture currently requires an x86_64 host')
    for tool in ['qemu-system-x86_64', 'qemu-img', 'genisoimage']:
        if not shutil.which(tool):
            raise SystemExit(f'Required tool missing: {tool}')
    binaries = Path(os.environ.get('CARGO_TARGET_DIR', ROOT / 'target')).resolve() / 'debug'
    for binary in ['pkd', 'examples/apt-probe']:
        assert (binaries / binary).is_file(), f'Missing {binary}; run scripts/verify.sh vm'
    cache = Path(os.environ.get('PKGDECK_VM_CACHE', ROOT / 'build/host-vm')).resolve()
    cache.mkdir(parents=True, exist_ok=True)
    with (cache / 'cache.lock').open('w') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        image = cache / 'base.img'
        if not image.exists():
            download = cache / 'base.img.part'
            with urllib.request.urlopen(IMAGE_URL, timeout=120) as response, download.open('wb') as output:
                shutil.copyfileobj(response, output)
            assert digest(download) == IMAGE_SHA256, 'VM image checksum mismatch'
            download.replace(image)
        assert digest(image) == IMAGE_SHA256, 'VM image checksum mismatch'
        key = recipe_key((ROOT / 'scripts/vm/prepare.py').read_bytes())
        prepared = cache / f'prepared-{key}.qcow2'
        checksum = prepared.with_suffix('.sha256')
        if not prepared.exists() or not checksum.exists() or digest(prepared) != checksum.read_text().strip():
            staging = cache / f'prepared-{key}.part'
            staging.unlink(missing_ok=True)
            try:
                subprocess.run(['qemu-img', 'create', '-f', 'qcow2', '-F', 'qcow2', '-b', str(image), str(staging), '12G'], check=True)
                boot(staging, cache, 'prepare', 'prepare.py', 'PKGDECK_PREPARE_PASS')
                staging.replace(prepared)
                checksum.write_text(digest(prepared) + '\n')
            finally:
                staging.unlink(missing_ok=True)
        else:
            print(f'Using verified prepared VM cache: {prepared}', flush=True)
        with tempfile.TemporaryDirectory(prefix='pkgdeck-host-vm-') as directory:
            disk = Path(directory) / 'test.qcow2'
            subprocess.run(['qemu-img', 'create', '-f', 'qcow2', '-F', 'qcow2', '-b', str(prepared), str(disk)], check=True)
            boot(disk, cache, 'lifecycle', 'guest.py', 'PKGDECK_HOST_VM_PASS')
        print('Disposable VM passed; test overlay removed', flush=True)


if __name__ == '__main__':
    # Let the verifier's timeout unwind cleanup instead of orphaning QEMU.
    def interrupted(_signal, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    main()
