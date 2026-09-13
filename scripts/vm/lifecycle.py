"""Shared synthetic lifecycles for disposable VM and Podman guests."""
import hashlib
import io
import json
import os
import sys
from pathlib import Path
import subprocess
import tarfile


def run(*args, **kwargs):
    print("LIFECYCLE", *args, flush=True)
    kwargs.setdefault("timeout", 240)
    try:
        return subprocess.run(args, check=True, text=True, **kwargs)
    except subprocess.CalledProcessError as error:
        print(error.stdout or "", error.stderr or "", flush=True)
        raise


def cli(source, *args):
    user = 'pkgdeck-test' if source == 'apt' else 'linuxbrew'
    result = run('runuser', '-u', user, '--', 'env',
                 'PATH=/home/linuxbrew/.linuxbrew/bin:/usr/bin:/bin',
                 '/mnt/pkgdeck-bin/pkd', '--json', '--yes', '--from', source,
                 *args, capture_output=True, timeout=240)
    value = json.loads(result.stdout)
    assert value['schema_version'] == 1 and value['exit_code'] == 0, value
    return value['data']


def write(source, operation, name=''):
    if os.environ.get('PKGDECK_FRONTEND') not in ('tui', 'gui'):
        return cli(source, operation, *([name] if name else []))
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
    from tui_driver import write as tui_write
    user = 'pkgdeck-test' if source == 'apt' else 'linuxbrew'
    if os.environ.get('PKGDECK_FRONTEND') == 'gui':
        from gui_driver import desktop
        print('GUI', source, operation, name, flush=True)
        with desktop(['runuser', '-u', user, '--', 'env', '-u', 'XDG_CONFIG_HOME',
                      'PATH=/home/linuxbrew/.linuxbrew/bin:/usr/bin:/bin',
                      '/mnt/pkgdeck-app/AppRun', '--from', source, '--auth', 'sudo']) as gui:
            gui.write(operation, name)
        return
    print('TUI', source, operation, name, flush=True)
    tui_write(['runuser', '-u', user, '--', 'env',
               'PATH=/home/linuxbrew/.linuxbrew/bin:/usr/bin:/bin',
               '/mnt/pkgdeck-bin/pkd', '--from', source], operation, name)


def apt_fixture():
    package = Path('/tmp/pkgdeck-package')
    (package / 'DEBIAN').mkdir(parents=True)
    (package / 'usr/share/pkgdeck-fixture').mkdir(parents=True)
    (package / 'usr/share/pkgdeck-fixture/version').write_text('1.0\n')
    control = 'Package: pkgdeck-fixture\nVersion: 1.0\nArchitecture: all\nMaintainer: Synthetic <fixture@example.invalid>\nDescription: Disposable host execution fixture\n'
    (package / 'DEBIAN/control').write_text(control)
    repo = Path('/opt/pkgdeck-fixture-repo')
    repo.mkdir()
    deb = repo / 'pkgdeck-fixture_1.0_all.deb'
    run('dpkg-deb', '--build', '--root-owner-group', str(package), str(deb))
    data = deb.read_bytes()
    (repo / 'Packages').write_text(control + f'Filename: {deb.name}\nSize: {len(data)}\nSHA256: {hashlib.sha256(data).hexdigest()}\n\n')
    source = Path('/etc/apt/sources.list.d/pkgdeck-fixture.list')
    source.write_text(f'deb [trusted=yes] file:{repo} ./\n')
    run('apt-get', 'update', '-qq', '-o', f'Dir::Etc::sourcelist={source}', '-o', 'Dir::Etc::sourceparts=-')


def apt():
    name = 'pkgdeck-fixture'
    assert cli('apt', 'sources')['sources'][0]['availability'] == {'Ok': 'available'}
    assert any(p['id']['name'] == name for p in cli('apt', 'search', name)['packages'])
    assert cli('apt', 'info', name)['package']['candidate_version'] == '1.0'
    write('apt', 'install', name)
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
    write('apt', 'update')
    package = next(p for p in cli('apt', 'list')['packages'] if p['id']['name'] == name)
    assert package['installed_version'] == '1.0' and package['candidate_version'] == '2.0' and package['update'] == 'available', package
    write('apt', 'upgrade', name)
    assert run('dpkg-query', '-W', '-f=${Version}', name, capture_output=True).stdout == '2.0'
    assert Path('/usr/share/pkgdeck-fixture/version').read_text() == '2.0\n'
    write('apt', 'remove', name)
    assert not Path('/usr/share/pkgdeck-fixture/version').exists()
    assert not any(p['id']['name'] == name for p in cli('apt', 'list')['packages'])
    print('PASS package lifecycle APT detect/search/details/install/update/upgrade/remove 1.0 → 2.0', flush=True)


def brew(*args):
    return run('runuser', '-u', 'linuxbrew', '--', 'env',
               'HOMEBREW_NO_AUTO_UPDATE=1', 'HOMEBREW_NO_ANALYTICS=1',
               '/home/linuxbrew/.linuxbrew/bin/brew', *args,
               capture_output=True, timeout=240)


def homebrew():
    assert Path('/etc/pkgdeck-prepared').is_file(), 'Missing prepared dependencies'
    prefix = Path('/home/linuxbrew/.linuxbrew')
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
    write('homebrew', 'install', name)
    assert json.loads(brew('info', '--json=v2', name).stdout)['formulae'][0]['installed'][0]['version'] == '1.0'
    version('2.0')
    run('chown', '-R', 'linuxbrew:linuxbrew', str(tap))
    write('homebrew', 'update')
    package = next(p for p in cli('homebrew', 'list')['packages'] if p['id']['name'] == name)
    assert package['installed_version'] == '1.0' and package['candidate_version'] == '2.0' and package['update'] == 'available', package
    write('homebrew', 'upgrade', name)
    installed = json.loads(brew('info', '--json=v2', name).stdout)['formulae'][0]['installed']
    assert {'1.0', '2.0'}.issubset({item['version'] for item in installed}), installed
    assert cli('homebrew', 'info', name)['package']['installed_version'] == '2.0'
    assert run(str(prefix / 'bin/pkgdeck-fixture'), capture_output=True).stdout == '2.0\n'
    write('homebrew', 'remove', name)
    assert not (prefix / 'bin/pkgdeck-fixture').exists()
    assert json.loads(brew('info', '--json=v2', name).stdout)['formulae'][0]['installed'] == []
    print('PASS package lifecycle Homebrew detect/search/details/install/update/upgrade/remove 1.0 → 2.0', flush=True)
