#!/usr/bin/env python3
"""Create a disposable QEMU guest; never run authorization/package tests on the host.
Requires qemu-system-x86_64, qemu-img, genisoimage, and a built apt-probe example.
The guest sees the checkout read-only, has no host credentials, and is deleted on exit.
"""
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
IMAGE_URL = 'https://cloud-images.ubuntu.com/releases/26.04/release-20260823/ubuntu-26.04-server-cloudimg-amd64.img'
IMAGE_SHA256 = '8196be9d7958059cb56c6c75c80fdf6cee8a8885bc149ea791d7db1c7ef93035'


def main():
    for tool in ['qemu-system-x86_64', 'qemu-img', 'genisoimage']:
        if not shutil.which(tool):
            raise SystemExit(f'Required tool missing: {tool}')
    assert (ROOT / 'target/debug/examples/apt-probe').is_file(), 'Run cargo build -p pkgdeck-core --example apt-probe first'
    assert (ROOT / 'target/debug/pkd').is_file(), 'Run cargo build -p pkd first'
    cache = ROOT / 'build/host-vm'
    cache.mkdir(parents=True, exist_ok=True)
    image = cache / 'base.img'
    if not image.exists():
        urllib.request.urlretrieve(IMAGE_URL, image)
    with image.open('rb') as stream:
        assert hashlib.file_digest(stream, 'sha256').hexdigest() == IMAGE_SHA256, 'VM image checksum mismatch'
    with tempfile.TemporaryDirectory(prefix='pkgdeck-host-vm-') as directory:
        work = Path(directory)
        disk = work / 'disk.qcow2'
        subprocess.run(['qemu-img', 'create', '-f', 'qcow2', '-F', 'qcow2', '-b', str(image), str(disk), '12G'], check=True)
        (work / 'meta-data').write_text('instance-id: pkgdeck-host-execution\nlocal-hostname: pkgdeck-test\n')
        (work / 'user-data').write_text('''#cloud-config
users: []
write_files:
  - path: /etc/pkgdeck-disposable-vm
    content: host-execution-test
runcmd:
  - [mkdir, -p, /mnt/pkgdeck]
  - [mount, -t, 9p, -o, "trans=virtio,version=9p2000.L,ro", pkgdeck, /mnt/pkgdeck]
  - [sh, -c, "python3 /mnt/pkgdeck/scripts/vm/guest.py > /dev/virtio-ports/pkgdeck.results 2>&1; poweroff"]
''')
        seed = work / 'seed.iso'
        subprocess.run(['genisoimage', '-quiet', '-output', str(seed), '-volid', 'cidata', '-joliet', '-rock', str(work / 'user-data'), str(work / 'meta-data')], check=True)
        log_path = cache / 'serial.log'
        results_path = cache / 'results.log'
        with log_path.open('w') as log:
            command = ['qemu-system-x86_64', '-accel', 'kvm' if os.access('/dev/kvm', os.R_OK | os.W_OK) else 'tcg',
                '-m', '2048', '-smp', '2', '-display', 'none', '-monitor', 'none', '-serial', 'stdio', '-no-reboot',
                '-drive', f'file={disk},format=qcow2,if=virtio', '-drive', f'file={seed},media=cdrom,readonly=on',
                '-virtfs', f'local,path={ROOT},mount_tag=pkgdeck,security_model=none,readonly=on',
                '-device', 'virtio-serial-pci', '-chardev', f'file,id=results,path={results_path}',
                '-device', 'virtserialport,chardev=results,name=pkgdeck.results',
                '-netdev', 'user,id=net0', '-device', 'virtio-net-pci,netdev=net0']
            child = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT)
            try:
                child.wait(timeout=900)
            finally:
                if child.poll() is None:
                    child.terminate()
                    try:
                        child.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        child.kill()
                        child.wait()
        output = results_path.read_text(errors='replace')
        for line in output.splitlines():
            if line.startswith('PASS ') or 'PKGDECK_HOST_VM_' in line:
                print(line)
        assert child.returncode == 0 and 'PKGDECK_HOST_VM_PASS' in output and 'PKGDECK_HOST_VM_FAIL' not in output, f'Guest validation failed; inspect {results_path} and {log_path}'
        print(f'Disposable VM passed; log: {log_path}')


if __name__ == '__main__':
    main()
