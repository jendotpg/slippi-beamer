#!/bin/bash
# Build station images from macOS.
#
#   ./mac-build.sh              all targets
#   ./mac-build.sh armhf        just the 32-bit image
#
# BEAMER_USER_PASS=...              MANDATORY - password for the account
# BEAMER_USER=beamer                account name, default beamer
# XZ_LEVEL=1 ./mac-build.sh armhf   iterate without waiting on compression,
#                                   which is most of the wall time of a build.
# REBUILD_GOLDEN=1 ./mac-build.sh   force the golden VM to be rebuilt
#
# Image builds need loop devices and a Linux chroot, so the actual work happens
# inside a lima VM. That VM is defined entirely by build/piforge.yaml: this
# script builds a golden instance from it once, then clones a throwaway instance
# for every build and throws it away at the start of the next one.
set -euo pipefail

REPO=$(cd "$(dirname "$0")" && pwd)
SPEC="$REPO/build/piforge.yaml"
VM=${VM:-piforge}
GOLDEN=${GOLDEN:-piforge-golden}
REMOTE=/tmp/beamer-image
XZ_LEVEL=${XZ_LEVEL:-6}
BEAMER_USER=${BEAMER_USER:-beamer}
BEAMER_USER_PASS=${BEAMER_USER_PASS:-}

# The repo is mounted into the VM at the same absolute path, so one value works
# on both sides and build-image.sh needs no arguments it did not already have.
# These are also its own defaults (see build-image.sh); only this script used to
# override them to /var/tmp inside the VM, which is what left 1.1GB of pinned
# Raspberry Pi OS base images living nowhere but in a disposable VM.
CACHE="$REPO/.cache"
DIST="$REPO/dist"

[[ -n "$BEAMER_USER_PASS" ]] || {
    echo "ERROR: set BEAMER_USER_PASS - see the header." >&2
    echo "A station has no console, so an image with no login has to be" >&2
    echo "reflashed to debug." >&2
    exit 1
}

TARGETS=("$@")
[[ ${#TARGETS[@]} -gt 0 ]] || TARGETS=(armhf)
[[ "${TARGETS[0]}" == "all" ]] && TARGETS=(armhf)

command -v limactl >/dev/null || { echo "ERROR: lima not installed (brew install lima)" >&2; exit 1; }
[[ -f "$SPEC" ]] || { echo "ERROR: no VM spec at $SPEC" >&2; exit 1; }

say() { echo; echo "==> $*"; }

instance_exists() { limactl list --quiet 2>/dev/null | grep -qx "$1"; }

# --- golden VM ------------------------------------------------------------
# Built from the spec, then left stopped forever. It is only ever cloned; no
# build runs in it, so nothing accumulates in it either.
mkdir -p "$CACHE" "$DIST"

SPEC_HASH=$(shasum -a 256 "$SPEC" | cut -d' ' -f1)

# Recorded on the host rather than inside the instance. The golden is deliberately
# kept stopped, and `limactl shell` cannot read from a stopped instance - so
# reading the stamp from the guest came back empty every time and rebuilt the
# golden on every build, silently, while still appearing to work.
#
# A stale stamp cannot cause a stale VM to be reused: instance_exists is checked
# too, so deleting the golden by hand still forces a rebuild. The only cost of
# losing this file is one unnecessary rebuild.
GOLDEN_STAMP="$CACHE/$GOLDEN.spec"
GOLDEN_HASH=
[[ -f "$GOLDEN_STAMP" ]] && GOLDEN_HASH=$(cat "$GOLDEN_STAMP")

if [[ "${REBUILD_GOLDEN:-0}" == "1" ]] || ! instance_exists "$GOLDEN" || [[ "$GOLDEN_HASH" != "$SPEC_HASH" ]]; then
    if instance_exists "$GOLDEN"; then
        if [[ "${REBUILD_GOLDEN:-0}" == "1" ]]; then
            say "rebuilding golden VM '$GOLDEN' (REBUILD_GOLDEN=1)"
        else
            say "golden VM '$GOLDEN' was built from a different piforge.yaml - rebuilding"
        fi
        limactl stop -f "$GOLDEN" 2>/dev/null || true
        limactl delete -f "$GOLDEN"
    else
        say "creating golden VM '$GOLDEN' from build/piforge.yaml"
    fi

    limactl create --yes --name="$GOLDEN" --mount-only "$REPO:w" "$SPEC"
    limactl start --yes "$GOLDEN"
    limactl stop "$GOLDEN"

    # Written only after the instance is fully built, so an interrupted create
    # leaves no stamp and the next build retries instead of trusting a half-VM.
    printf '%s\n' "$SPEC_HASH" > "$GOLDEN_STAMP"
else
    say "golden VM '$GOLDEN' is current"
    [[ "$(limactl list --format '{{.Status}}' "$GOLDEN" 2>/dev/null)" == "Running" ]] \
        && limactl stop "$GOLDEN"
fi

# --- a fresh build VM -----------------------------------------------------
# Deleted here rather than after the build, so the previous build's VM is still
# around to poke at when something failed.
if instance_exists "$VM"; then
    say "discarding the previous build VM '$VM'"
    limactl stop -f "$VM" 2>/dev/null || true
    limactl delete -f "$VM"
fi

say "cloning '$GOLDEN' -> '$VM'"
limactl clone --yes "$GOLDEN" "$VM"
limactl start --yes "$VM"

# Tripwires. With a fresh clone these cannot fail; if they ever do, the clone
# silently did not happen and the build is about to run on a reused VM.
ACTUAL_HOST=$(limactl shell "$VM" -- hostname)
[[ "$ACTUAL_HOST" == "lima-$VM" ]] || {
    echo "ERROR: build VM reports hostname '$ACTUAL_HOST', expected 'lima-$VM'." >&2
    echo "The clone did not take, so this is not a clean build host." >&2
    exit 1
}
STRAY=$(limactl shell "$VM" -- sh -c 'ls -A /usr/local/sbin 2>/dev/null' || true)
[[ -z "$STRAY" ]] || {
    echo "ERROR: /usr/local/sbin is not empty on a freshly cloned VM:" >&2
    echo "$STRAY" >&2
    echo "Something is baked into the golden instance that should not be." >&2
    exit 1
}

# --- sync the repo in -----------------------------------------------------
# The repo is mounted read-write, but the build untars its own copy: build-image.sh
# chmods and chroots, and doing that on a mounted host filesystem is both slow and
# a good way to leave root-owned files in your working tree.
say "syncing scripts into the VM"
limactl shell "$VM" -- sudo rm -rf "$REMOTE"
limactl shell "$VM" -- mkdir -p "$REMOTE"
tar -C "$REPO" -cf - build scripts | limactl shell "$VM" -- tar -C "$REMOTE" -xf -
limactl shell "$VM" -- bash -c "chmod +x '$REMOTE'/build/*.sh '$REMOTE'/scripts/*.sh '$REMOTE'/scripts/beamer-led.shutdown"

# --- build ----------------------------------------------------------------
mkdir -p "$CACHE" "$DIST"
for t in "${TARGETS[@]}"; do
    say "building $t"
    [[ "$t" == armhf ]] && echo "(armhf runs under qemu on Apple Silicon - expect this to take a while)"
    limactl shell "$VM" -- sudo env CACHE="$CACHE" DIST="$DIST" \
        XZ_LEVEL="$XZ_LEVEL" \
        BEAMER_USER="$BEAMER_USER" \
        BEAMER_USER_PASS="$BEAMER_USER_PASS" \
        "$REMOTE/build/build-image.sh" "$t"
done

# Artifacts land straight in ./dist through the mount, so there is nothing to
# copy back out.

say "generating Imager manifest"
"$REPO/build/make-manifest.sh"

say "done"
ls -lh "$DIST"
