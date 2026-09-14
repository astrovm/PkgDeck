#!/usr/bin/env bash
# The caller holds cache.lock. Only a successful prepare publishes an image.
verified() { [[ -f $2 ]] && [[ $(sha256sum "$2" | cut -d ' ' -f1) == "$1" ]]; }
prepare_cached() {
    local image=$1 prepared=$2 checksum=$3
    if [[ -f $checksum ]] && verified "$(cat "$checksum")" "$prepared"; then
        echo "Using verified prepared VM cache: $prepared"
        return
    fi
    local partial="$prepared.part"
    rm -f -- "$partial"
    if qemu-img create -f qcow2 -F qcow2 -b "$image" "$partial" 12G && boot "$partial" prepare prepare.sh PKGDECK_PREPARE_PASS; then
        mv "$partial" "$prepared"
        sha256sum "$prepared" | cut -d ' ' -f1 >"$checksum"
    else
        local status=$?
        rm -f -- "$partial"
        return "$status"
    fi
}
