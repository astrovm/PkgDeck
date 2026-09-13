#!/usr/bin/env python3
"""Prepare dependencies only, inside a disposable VM or rootless container."""
import argparse
import hashlib
import os
from pathlib import Path
import subprocess

BREW_SHA256 = '705c5e205c4bdd70d79b0b8b3c6bf2a832193e921351bf85c9d26fb10770c801'


def run(*args, **kwargs):
    print('SETUP', *args, flush=True)
    return subprocess.run(args, check=True, timeout=600, text=True, **kwargs)


def prepare(container=False):
    marker = Path('/etc/pkgdeck-disposable-container' if container else '/etc/pkgdeck-disposable-vm')
    assert os.geteuid() == 0 and marker.is_file(), 'Preparation is restricted to disposable guests'
    if container:
        assert Path('/run/.containerenv').exists() or os.environ.get('container') == 'podman', 'Expected Podman build/run namespace'
    else:
        run('systemctl', 'stop', 'apt-daily.timer', 'apt-daily-upgrade.timer', 'apt-daily.service', 'apt-daily-upgrade.service')
    run('apt-get', 'update', '-qq')
    run('apt-get', 'install', '-y', '--no-install-recommends', 'pkexec', 'polkitd', 'sudo', 'python3-apt', 'build-essential', 'git', 'curl', 'jq', 'file', 'procps', 'ca-certificates')
    run('useradd', '-m', 'linuxbrew')
    prefix = Path('/home/linuxbrew/.linuxbrew')
    checkout = prefix / 'Homebrew'
    checkout.mkdir(parents=True)
    archive = Path('/opt/pkgdeck-brew-6.0.22.tar.gz')
    run('curl', '--fail', '--location', '--retry', '3', '--max-time', '120', 'https://github.com/Homebrew/brew/archive/refs/tags/6.0.22.tar.gz', '-o', str(archive))
    assert hashlib.sha256(archive.read_bytes()).hexdigest() == BREW_SHA256, 'Homebrew source checksum mismatch'
    run('tar', '-xzf', str(archive), '--strip-components=1', '-C', str(checkout))
    (prefix / 'bin').mkdir()
    (prefix / 'bin/brew').symlink_to('../Homebrew/bin/brew')
    run('git', '-C', str(checkout), 'init', '-b', 'stable')
    run('git', '-C', str(checkout), 'add', '.')
    run('git', '-C', str(checkout), '-c', 'user.name=Synthetic', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'Pinned Homebrew 6.0.22 fixture')
    run('git', '-C', str(checkout), 'tag', '6.0.22')
    run('git', '-C', str(checkout), 'branch', 'main')
    run('git', 'clone', '--bare', str(checkout), '/opt/brew-origin.git')
    run('git', '-C', str(checkout), 'remote', 'add', 'origin', '/opt/brew-origin.git')
    run('git', '-C', str(checkout), 'fetch', 'origin')
    run('chown', '-R', 'linuxbrew:linuxbrew', '/home/linuxbrew', '/opt/brew-origin.git')
    # Warm only tool/metadata downloads. No test packages or privileged test users
    # are present in the reusable image; each lifecycle creates its own fixtures.
    for args in [('ruby', '-e', 'puts RUBY_VERSION'), ('formulae',)]:
        run('runuser', '-u', 'linuxbrew', '--', 'env', 'HOMEBREW_NO_AUTO_UPDATE=1', 'HOMEBREW_NO_ANALYTICS=1', str(prefix / 'bin/brew'), *args, stdout=subprocess.DEVNULL)
    run('sh', '-c', 'dpkg-query -W > /opt/pkgdeck-os-packages.txt')
    Path('/etc/pkgdeck-prepared').write_text('dependencies-only\n')
    print('PKGDECK_PREPARE_PASS', flush=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--container', action='store_true')
    args = parser.parse_args()
    prepare(args.container)
