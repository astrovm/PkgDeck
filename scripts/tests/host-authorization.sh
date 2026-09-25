#!/usr/bin/env bash
# Mutates APT, sudoers, and polkit only on a disposable GitHub-hosted runner.
set -euo pipefail
trap 'echo "host-authorization FAILED at line $LINENO: $BASH_COMMAND" >&2' ERR
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

[[ $EUID == 0 && ${GITHUB_ACTIONS:-} == true &&
    ${GITHUB_REPOSITORY:-} == astrovm/PkgDeck &&
    ${RUNNER_ENVIRONMENT:-} == github-hosted &&
    ${RUNNER_OS:-} == Linux && ${RUNNER_ARCH:-} == X64 &&
    -n ${GITHUB_WORKSPACE:-} &&
    $(realpath "$GITHUB_WORKSPACE") == "$PWD" ]] || {
    echo 'Host authorization tests require the disposable PkgDeck GitHub-hosted runner.' >&2
    exit 1
}
source /etc/os-release
[[ $ID == ubuntu && $VERSION_ID == 26.04 ]] || {
    echo 'Host authorization tests require Ubuntu 26.04.' >&2
    exit 1
}

fixture=pkgdeck-fixture
marker=/etc/pkgdeck-disposable-ci-runner
repo=/etc/apt/sources.list.d/pkgdeck-fixture.list
rule=/etc/polkit-1/rules.d/00-pkgdeck-fixture.rules
sudoers=/etc/sudoers.d/pkgdeck-fixture
binaries=/opt/pkgdeck-bin
runner=/usr/libexec/pkgdeck-host-runner
for path in "$marker" "$repo" "$rule" "$sudoers" "$binaries" "$runner" /opt/pkgdeck-fixture-repo /tmp/pkgdeck-package; do
    [[ ! -e $path ]] || { echo "Fixture path already exists: $path" >&2; exit 1; }
done
for user in pkgdeck-test pkgdeck-denied; do
    ! id "$user" &>/dev/null || { echo "Fixture user already exists: $user" >&2; exit 1; }
done
! dpkg-query -W "$fixture" &>/dev/null || {
    echo 'Fixture package already exists.' >&2
    exit 1
}
for path in target/debug/pkd target/debug/pkgdeck-tools target/debug/examples/apt-probe target/debug/pkgdeck-host-runner; do
    [[ -x $path ]] || { echo "Missing build artifact: $path" >&2; exit 1; }
done

cleanup() {
    set +e
    if dpkg-query -W -f='${db:Status-Status}' "$fixture" 2>/dev/null | grep -qx installed; then
        apt-get remove -y -qq "$fixture" >/dev/null 2>&1
    fi
    rm -f -- "$rule" "$sudoers" "$repo" "$marker" "$runner"
    userdel -r pkgdeck-denied &>/dev/null
    userdel -r pkgdeck-test &>/dev/null
    rm -rf -- "$binaries" /opt/pkgdeck-fixture-repo /tmp/pkgdeck-package
}
trap cleanup EXIT

printf 'github-hosted-ubuntu-26.04\n' >"$marker"
mkdir -p "$binaries/examples"
cp target/debug/pkd target/debug/pkgdeck-tools "$binaries/"
install -Dm755 target/debug/pkgdeck-host-runner "$runner"
cp target/debug/examples/apt-probe "$binaries/examples/"
scripts/build-apt.sh "$binaries"
useradd -m pkgdeck-test
useradd -m pkgdeck-denied
printf 'pkgdeck-test ALL=(root) NOPASSWD: /usr/bin/apt-get\n' >"$sudoers"
chmod 440 "$sudoers"
source scripts/vm/lifecycle.sh
apt_fixture

doctor=$(runuser -u pkgdeck-test -- "$binaries/pkd" doctor)
[[ $doctor =~ Runtime\ +native && $doctor =~ Architecture\ +x86_64 &&
    $doctor =~ ✓\ APT\ +/usr/bin/apt-get ]]
cat >"$rule" <<'RULE'
polkit.addRule(function(action, subject) {
    if (action.id == "org.freedesktop.policykit.exec" && action.lookup("program") == "/usr/bin/apt-get") {
        return subject.user == "pkgdeck-test" ? polkit.Result.YES : polkit.Result.NO;
    }
});
RULE
systemctl restart polkit

probe() {
    local user=$1 auth=$2 action=$3 expected=${4:-} output
    if output=$(runuser -u "$user" -- "$binaries/examples/apt-probe" "$auth" "$action" "$fixture" 2>&1); then
        [[ -z $expected ]]
    else
        [[ -n $expected && $output == *"$expected"* ]] || {
            printf '%s\n' "$output" >&2
            return 1
        }
    fi
    echo "PASS $user $auth $action ${expected:-success}"
}
probe root sudo install 'unprivileged user'
probe pkgdeck-denied sudo install 'authorization denied'
probe pkgdeck-denied polkit install 'authorization denied'
# POSIX record locks, as used by dpkg (not flock locks).
"$binaries/pkgdeck-tools" apt-lock-probe
for auth in sudo polkit; do
    probe pkgdeck-test "$auth" install
    [[ $(dpkg-query -W '-f=${db:Status-Status} ${Version}' "$fixture") == 'installed 1.0' &&
        $(cat /usr/share/pkgdeck-fixture/version) == 1.0 ]]
    probe pkgdeck-test "$auth" remove
    [[ ! -e /usr/share/pkgdeck-fixture/version ]]
done
printf 'pkgdeck-test ALL=(root) NOPASSWD: %s\n' "$runner" >"$sudoers"
chmod 440 "$sudoers"
cat >"$rule" <<'RULE'
polkit.addRule(function(action, subject) {
    if (action.id == "org.freedesktop.policykit.exec" && action.lookup("program") == "/usr/libexec/pkgdeck-host-runner") {
        return subject.user == "pkgdeck-test" ? polkit.Result.YES : polkit.Result.NO;
    }
});
RULE
systemctl restart polkit
batch_probe() {
    local auth=$1 action=$2 output status
    output=$(mktemp)
    if timeout --signal=TERM --kill-after=5s 75s \
        runuser -u pkgdeck-test -- "$binaries/pkd" --json --yes --auth "$auth" --from apt "$action" "$fixture" >"$output"; then
        status=0
    else
        status=$?
    fi
    if ((status != 0)); then
        cat "$output" >&2
        rm -f "$output"
        echo "Batch $auth $action exited with $status" >&2
        return "$status"
    fi
    if ! jq -e '.exit_code == 0' "$output"; then
        cat "$output" >&2
        rm -f "$output"
        return 1
    fi
    rm -f "$output"
}
for auth in sudo polkit; do
    batch_probe "$auth" install
    [[ $(cat /usr/share/pkgdeck-fixture/version) == 1.0 ]]
    batch_probe "$auth" remove
    [[ ! -e /usr/share/pkgdeck-fixture/version ]]
done
echo PKGDECK_HOST_AUTHORIZATION_PASS
