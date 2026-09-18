#!/usr/bin/env bash
set -euo pipefail
trap 'echo "guest FAILED at line $LINENO: $BASH_COMMAND" >&2' ERR
[[ $EUID == 0 && -f /etc/pkgdeck-prepared ]]
if [[ ${1:-} == --container ]]; then
    [[ -f /run/.containerenv && -f /etc/pkgdeck-disposable-container ]]
else
    [[ $(cat /etc/pkgdeck-disposable-vm) == host-execution-test ]]
    systemctl stop apt-daily.timer apt-daily-upgrade.timer apt-daily.service apt-daily-upgrade.service
fi
# Build the helper against this guest's APT ABI; frontends remain the host-built binaries.
mkdir -p /opt/pkgdeck-bin
cp /mnt/pkgdeck-bin/pkd /opt/pkgdeck-bin/
cd /mnt/pkgdeck
scripts/build-apt.sh /opt/pkgdeck-bin
useradd -m pkgdeck-test
printf 'pkgdeck-test ALL=(root) NOPASSWD: /usr/bin/apt-get\n' >/etc/sudoers.d/pkgdeck-fixture
chmod 440 /etc/sudoers.d/pkgdeck-fixture
if [[ ${1:-} != --container ]]; then ln -s /mnt/pkgdeck-bin/pkgdeck-tools /mnt/pkgdeck-tools; fi
source scripts/vm/lifecycle.sh
apt_fixture
if [[ ${1:-} != --container ]]; then
    useradd -m pkgdeck-denied
    doctor=$(runuser -u pkgdeck-test -- /opt/pkgdeck-bin/pkd doctor)
    [[ $doctor == *'Runtime: Native'* && $doctor == *'Host architecture: x86_64'* && $doctor == *'APT: /usr/bin/apt-get'* ]]
    cat >/etc/polkit-1/rules.d/00-pkgdeck-fixture.rules <<'RULE'
polkit.addRule(function(action, subject) {
    if (action.id == "org.freedesktop.policykit.exec" && action.lookup("program") == "/usr/bin/apt-get") {
        return subject.user == "pkgdeck-test" ? polkit.Result.YES : polkit.Result.NO;
    }
});
RULE
    systemctl restart polkit
    probe() {
        local user=$1 auth=$2 action=$3 expected=${4:-} output
        if output=$(runuser -u "$user" -- /mnt/pkgdeck-bin/examples/apt-probe "$auth" "$action" pkgdeck-fixture 2>&1); then
            [[ -z $expected ]]
        else [[ -n $expected && $output == *"$expected"* ]]; fi
        echo "PASS $user $auth $action ${expected:-success}"
    }
    probe root sudo install 'unprivileged user'
    probe pkgdeck-denied sudo install 'authorization denied'
    probe pkgdeck-denied polkit install 'authorization denied'
    # POSIX record locks, as used by dpkg (not flock locks).
    /mnt/pkgdeck-bin/pkgdeck-tools apt-lock-probe
    for auth in sudo polkit; do
        probe pkgdeck-test "$auth" install
        [[ $(dpkg-query -W '-f=${db:Status-Status} ${Version}' pkgdeck-fixture) == 'installed 1.0' && $(cat /usr/share/pkgdeck-fixture/version) == 1.0 ]]
        probe pkgdeck-test "$auth" remove
        [[ ! -e /usr/share/pkgdeck-fixture/version ]]
    done
fi
apt_lifecycle
brew_lifecycle
if [[ ${1:-} == --container ]]; then echo PKGDECK_CONTAINER_PASS; else echo PKGDECK_HOST_VM_PASS; fi
