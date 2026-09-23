#!/usr/bin/env bash
# Verified dependency cache; every acceptance run uses a disposable overlay.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
usage='Usage: scripts/test-host-vm.sh [--prepare-only]'
prepare_only=false
if [[ ${1:-} == --prepare-only ]]; then
    prepare_only=true
    shift
fi
if (($#)); then echo "$usage" >&2; exit 2; fi
[[ $(uname -m) == x86_64 ]] || {
    echo 'VM fixture requires x86_64' >&2
    exit 1
}
for tool in qemu-system-x86_64 qemu-img genisoimage curl sha256sum flock; do
    command -v "$tool" >/dev/null || { echo "Missing prerequisite: $tool" >&2; exit 1; }
done
binaries=''
if [[ $prepare_only == false ]]; then
    binaries=$(realpath "${CARGO_TARGET_DIR:-target}/debug")
    for name in pkd pkgdeck-tools examples/apt-probe; do
        [[ -f $binaries/$name ]] || {
            echo "Missing build artifact: $binaries/$name (build the workspace debug binaries first)" >&2
            exit 1
        }
    done
fi
cache=$(realpath -m "${PKGDECK_VM_CACHE:-build/host-vm}")
mkdir -p "$cache"
exec 9>"$cache/cache.lock"
flock 9
image_sha=8196be9d7958059cb56c6c75c80fdf6cee8a8885bc149ea791d7db1c7ef93035
work=$(mktemp -d)
child='' tail_pid='' staging=''
cleanup() {
    if [[ -n $child ]]; then
        kill "$child" 2>/dev/null || true
        wait "$child" 2>/dev/null || true
    fi
    if [[ -n $tail_pid ]]; then
        kill "$tail_pid" 2>/dev/null || true
        wait "$tail_pid" 2>/dev/null || true
    fi
    rm -rf -- "$work"
    if [[ -n $staging ]]; then rm -f -- "$staging"; fi
    rm -f -- "$cache/base.img.part"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
check() { [[ -f $2 ]] && [[ $(sha256sum "$2" | cut -d ' ' -f1) == "$1" ]]; }
if [[ ! -f $cache/base.img ]]; then
    curl -fL --retry 3 --max-time 600 https://cloud-images.ubuntu.com/releases/26.04/release-20260823/ubuntu-26.04-server-cloudimg-amd64.img -o "$cache/base.img.part"
    check "$image_sha" "$cache/base.img.part"
    mv "$cache/base.img.part" "$cache/base.img"
fi
check "$image_sha" "$cache/base.img"
boot() {
    local disk=$1 phase=$2 script=$3 token=$4 logs accel=tcg cpu=max binary_seed=''
    local -a binary_mount=()
    if [[ $phase == lifecycle ]]; then
        binary_seed='  - [mount, -t, 9p, -o, "trans=virtio,version=9p2000.L,ro", pkdbin, /mnt/pkgdeck-bin]'
        binary_mount=(-virtfs "local,path=$binaries,mount_tag=pkdbin,security_model=none,readonly=on")
    fi
    logs="$cache/logs/$(date +%s%N)-$phase"
    mkdir -p "$logs"
    printf 'instance-id: pkgdeck-%s-%s\nlocal-hostname: pkgdeck-test\n' "$phase" "$(date +%s%N)" >"$work/meta-data"
    cat >"$work/user-data" <<SEED
#cloud-config
users: []
write_files:
  - path: /etc/pkgdeck-disposable-vm
    content: host-execution-test
runcmd:
  - [mkdir, -p, /mnt/pkgdeck, /mnt/pkgdeck-bin]
  - [mount, -t, 9p, -o, "trans=virtio,version=9p2000.L,ro", pkgdeck, /mnt/pkgdeck]
$binary_seed
  - [sh, -c, "bash /mnt/pkgdeck/scripts/vm/$script > /dev/virtio-ports/pkgdeck.results 2>&1 || echo PKGDECK_HOST_VM_FAIL > /dev/virtio-ports/pkgdeck.results; poweroff"]
SEED
    genisoimage -quiet -output "$work/seed.iso" -volid cidata -joliet -rock "$work/user-data" "$work/meta-data"
    if [[ -r /dev/kvm && -w /dev/kvm ]]; then
        accel=kvm
        cpu=host
    fi
    echo "VM $phase ($accel); logs: $logs"
    : >"$logs/results.log"
    timeout --kill-after=5s 900s qemu-system-x86_64 -accel "$accel" -cpu "$cpu" -m 2048 -smp 2 -display none -monitor none -serial stdio -no-reboot \
        -drive "file=$disk,format=qcow2,if=virtio" -drive "file=$work/seed.iso,media=cdrom,readonly=on" \
        -virtfs "local,path=$PWD,mount_tag=pkgdeck,security_model=none,readonly=on" \
        "${binary_mount[@]}" \
        -device virtio-serial-pci -chardev "file,id=results,path=$logs/results.log" -device virtserialport,chardev=results,name=pkgdeck.results \
        -netdev user,id=net0 -device virtio-net-pci,netdev=net0 >"$logs/serial.log" 2>&1 &
    child=$!
    tail --pid="$child" -n +1 -f "$logs/results.log" &
    tail_pid=$!
    wait "$child" || return $?
    child=
    wait "$tail_pid"
    tail_pid=
    grep -q "$token" "$logs/results.log" || return 1
    ! grep -q PKGDECK_HOST_VM_FAIL "$logs/results.log"
}
key=$({
    printf '%s' "$image_sha"
    cat scripts/vm/prepare.sh
} | sha256sum | cut -c1-16)
prepared="$cache/prepared-$key.qcow2"
checksum="$cache/prepared-$key.sha256"
source scripts/vm/cache.sh
staging="$prepared.part"
prepare_cached "$cache/base.img" "$prepared" "$checksum"
staging=
if [[ $prepare_only == true ]]; then
    echo 'Prepared VM cache ready'
    exit 0
fi
qemu-img create -f qcow2 -F qcow2 -b "$prepared" "$work/test.qcow2"
boot "$work/test.qcow2" lifecycle guest.sh PKGDECK_HOST_VM_PASS
echo 'Disposable VM passed; test overlay removed'
