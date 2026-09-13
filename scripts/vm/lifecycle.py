"""Synthetic versioned lifecycle acceptance tests, imported only by the VM guest."""
import hashlib
import io
import json
from pathlib import Path
import subprocess
import tarfile
import urllib.request


def run(*args, **kwargs):
    try:
        return subprocess.run(args, check=True, text=True, **kwargs)
    except subprocess.CalledProcessError as error:
        print(error.stdout or "", error.stderr or "", flush=True)
        raise


def cli(source, *args):
    user = 'pkgdeck-test' if source == 'apt' else 'linuxbrew'
    result = run('runuser', '-u', user, '--', 'env',
                 'PATH=/home/linuxbrew/.linuxbrew/bin:/usr/bin:/bin',
                 '/mnt/pkgdeck/target/debug/pkd', '--json', '--yes', '--from', source,
                 *args, capture_output=True, timeout=240)
    value = json.loads(result.stdout)
    assert value['schema_version'] == 1 and value['exit_code'] == 0, value
    return value['data']


def apt():
    name = 'pkgdeck-fixture'
    assert cli('apt', 'sources')['sources'][0]['availability'] == {'Ok': 'available'}
    assert any(p['id']['name'] == name for p in cli('apt', 'search', name)['packages'])
    assert cli('apt', 'info', name)['package']['candidate_version'] == '1.0'
    cli('apt', 'install', name)
    assert run('dpkg-query', '-W', '-f=${Version}', name, capture_output=True).stdout == '1.0'
    package = Path('/tmp/pkgdeck-package')
    control = (package / 'DEBIAN/control').read_text().replace('Version: 1.0', 'Version: 2.0')
    (package / 'DEBIAN/control').write_text(control)
    (package / 'usr/share/pkgdeck-fixture/version').write_text('2.0\n')
    repo = Path('/opt/pkgdeck-fixture-repo')
    deb = repo / 'pkgdeck-fixture_2.0_all.deb'
    run('dpkg-deb', '--build', '--root-owner-group', str(package), str(deb))
    data = deb.read_bytes()
    (repo / 'Packages').write_text(control + f'Filename: {deb.name}\nSize: {len(data)}\nSHA256: {hashlib.sha256(data).hexdigest()}\n\n')
    cli('apt', 'update')
    package = next(p for p in cli('apt', 'list')['packages'] if p['id']['name'] == name)
    assert package['installed_version'] == '1.0' and package['candidate_version'] == '2.0' and package['update'] == 'available', package
    cli('apt', 'upgrade', name)
    assert run('dpkg-query', '-W', '-f=${Version}', name, capture_output=True).stdout == '2.0'
    assert Path('/usr/share/pkgdeck-fixture/version').read_text() == '2.0\n'
    cli('apt', 'remove', name)
    assert not Path('/usr/share/pkgdeck-fixture/version').exists()
    assert not any(p['id']['name'] == name for p in cli('apt', 'list')['packages'])
    print('PASS CLI APT detect/search/details/install/update/upgrade/remove 1.0 → 2.0', flush=True)


def brew(*args):
    return run('runuser', '-u', 'linuxbrew', '--', 'env',
               'HOMEBREW_NO_AUTO_UPDATE=1', 'HOMEBREW_NO_ANALYTICS=1',
               '/home/linuxbrew/.linuxbrew/bin/brew', *args,
               capture_output=True, timeout=240)


def homebrew():
    run('apt-get', 'install', '-y', 'build-essential', 'git', 'curl', 'file', 'procps', 'ca-certificates')
    run('useradd', '-m', 'linuxbrew')
    prefix = Path('/home/linuxbrew/.linuxbrew')
    checkout = prefix / 'Homebrew'
    checkout.mkdir(parents=True)
    # Fixed tool release; all installation and trust changes stay inside this guest.
    archive = Path('/tmp/brew.tar.gz')
    urllib.request.urlretrieve('https://github.com/Homebrew/brew/archive/refs/tags/6.0.22.tar.gz', archive)
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
    tap = Path('/opt/fixture-tap')
    (tap / 'Formula').mkdir(parents=True)
    run('git', '-c', f'safe.directory={tap}', '-C', str(tap), 'init', '-b', 'main')
    def version(number):
        archive = Path(f'/opt/pkgdeck-fixture-{number}.tar.gz')
        data = f'#!/bin/sh\nprintf "{number}\\n"\n'.encode()
        with tarfile.open(archive, 'w:gz') as output:
            info = tarfile.TarInfo('pkgdeck-fixture')
            info.size = len(data)
            info.mode = 0o755
            output.addfile(info, io.BytesIO(data))
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        (tap / 'Formula/pkgdeck-fixture.rb').write_text(f'''class PkgdeckFixture < Formula
  desc "Synthetic PkgDeck lifecycle fixture"
  homepage "https://example.invalid/pkgdeck"
  url "file://{archive}"
  version "{number}"
  sha256 "{digest}"
  license "MIT"
  def install
    bin.install "pkgdeck-fixture"
  end
end
''')
        run('git', '-c', f'safe.directory={tap}', '-C', str(tap), 'add', '.')
        run('git', '-c', f'safe.directory={tap}', '-C', str(tap), '-c', 'user.name=Synthetic', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', f'Fixture {number}')
    version('1.0')
    run('chown', '-R', 'linuxbrew:linuxbrew', str(tap))
    brew('tap', 'pkgdeck/fixtures', str(tap))
    brew('trust', '--tap', 'pkgdeck/fixtures')
    name = 'pkgdeck/fixtures/pkgdeck-fixture'
    assert cli('homebrew', 'sources')['sources'][0]['availability'] == {'Ok': 'available'}
    assert any(p['id']['name'] == name for p in cli('homebrew', 'search', 'pkgdeck-fixture')['packages'])
    assert cli('homebrew', 'info', name)['package']['candidate_version'] == '1.0'
    cli('homebrew', 'install', name)
    assert json.loads(brew('info', '--json=v2', name).stdout)['formulae'][0]['installed'][0]['version'] == '1.0'
    version('2.0')
    run('chown', '-R', 'linuxbrew:linuxbrew', str(tap))
    cli('homebrew', 'update')
    package = next(p for p in cli('homebrew', 'list')['packages'] if p['id']['name'] == name)
    assert package['installed_version'] == '1.0' and package['candidate_version'] == '2.0' and package['update'] == 'available', package
    cli('homebrew', 'upgrade', name)
    installed = json.loads(brew('info', '--json=v2', name).stdout)['formulae'][0]['installed']
    assert {'1.0', '2.0'}.issubset({item['version'] for item in installed}), installed
    assert cli('homebrew', 'info', name)['package']['installed_version'] == '2.0'
    assert run(str(prefix / 'bin/pkgdeck-fixture'), capture_output=True).stdout == '2.0\n'
    cli('homebrew', 'remove', name)
    assert not (prefix / 'bin/pkgdeck-fixture').exists()
    assert json.loads(brew('info', '--json=v2', name).stdout)['formulae'][0]['installed'] == []
    print('PASS CLI Homebrew detect/search/details/install/update/upgrade/remove 1.0 → 2.0', flush=True)
