#!/usr/bin/env python3
"""Runs only inside scripts/test-host-vm.py's disposable VM."""
import fcntl
import os
from pathlib import Path
import subprocess


def run(*args, **kwargs):
    print("FIXTURE", *args, flush=True)
    return subprocess.run(args, check=True, text=True, timeout=120, **kwargs)


def probe(user, authorization, action, expected=None):
    result = subprocess.run([
        'runuser', '-u', user, '--', '/mnt/pkgdeck-bin/examples/apt-probe',
        authorization, action, 'pkgdeck-fixture',
    ], capture_output=True, text=True, timeout=40)
    if expected:
        assert result.returncode != 0 and expected in result.stderr, result
    else:
        assert result.returncode == 0, result
    print(f'PASS {user} {authorization} {action} {expected or "success"}', flush=True)


def main():
    assert Path('/etc/pkgdeck-disposable-vm').read_text().strip() == 'host-execution-test'
    assert os.getuid() == 0
    run('systemctl', 'stop', 'apt-daily.timer', 'apt-daily-upgrade.timer', 'apt-daily.service', 'apt-daily-upgrade.service')
    assert Path('/etc/pkgdeck-prepared').is_file(), 'Prepare the reusable VM dependencies first'
    for user in ['pkgdeck-test', 'pkgdeck-denied']:
        run('useradd', '-m', user)
    doctor = run('runuser', '-u', 'pkgdeck-test', '--', '/mnt/pkgdeck-bin/pkd', 'doctor', capture_output=True)
    assert 'Runtime: Native' in doctor.stdout and 'Host architecture: x86_64' in doctor.stdout, doctor.stdout
    assert 'APT: /usr/bin/apt-get' in doctor.stdout, doctor.stdout
    print('PASS native host architecture and APT detection', flush=True)
    # Only this disposable guest grants authorization to the synthetic test user.
    sudoers = Path('/etc/sudoers.d/pkgdeck-fixture')
    sudoers.write_text('pkgdeck-test ALL=(root) NOPASSWD: /usr/bin/apt-get\n')
    sudoers.chmod(0o440)
    rules = Path('/etc/polkit-1/rules.d/00-pkgdeck-fixture.rules')
    rules.write_text('''polkit.addRule(function(action, subject) {
        if (action.id == "org.freedesktop.policykit.exec" &&
            action.lookup("program") == "/usr/bin/apt-get") {
            return subject.user == "pkgdeck-test" ? polkit.Result.YES : polkit.Result.NO;
        }
    });\n''')
    run('systemctl', 'restart', 'polkit')
    import lifecycle
    lifecycle.apt_fixture()
    probe('root', 'sudo', 'install', 'unprivileged user')
    probe('pkgdeck-denied', 'sudo', 'install', 'authorization denied')
    probe('pkgdeck-denied', 'polkit', 'install', 'authorization denied')
    with open('/var/lib/dpkg/lock-frontend', 'r+') as lock:
        fcntl.lockf(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        probe('pkgdeck-test', 'sudo', 'install', 'APT is busy')
    for authorization in ['sudo', 'polkit']:
        probe('pkgdeck-test', authorization, 'install')
        result = run('dpkg-query', '-W', '-f=${db:Status-Status} ${Version}', 'pkgdeck-fixture', capture_output=True)
        assert result.stdout == 'installed 1.0', result.stdout
        assert Path('/usr/share/pkgdeck-fixture/version').read_text() == '1.0\n'
        probe('pkgdeck-test', authorization, 'remove')
        assert not Path('/usr/share/pkgdeck-fixture/version').exists()
        result = subprocess.run(['dpkg-query', '-W', '-f=${db:Status-Status}', 'pkgdeck-fixture'], capture_output=True, text=True)
        assert result.stdout != 'installed', result
    lifecycle.apt()
    lifecycle.homebrew()
    print('PKGDECK_HOST_VM_PASS', flush=True)


if __name__ == '__main__':
    try:
        main()
    except Exception:
        import traceback
        traceback.print_exc()
        print('PKGDECK_HOST_VM_FAIL', flush=True)
    finally:
        subprocess.run(['poweroff'])
