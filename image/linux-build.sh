#!/bin/bash
# Build station images on a Linux host.
#
#   ./linux-build.sh              all targets
#   ./linux-build.sh armhf        just the 32-bit image
#
# BEAMER_USER_PASS=...              MANDATORY - password for the account
# BEAMER_USER=beamer                account name, default beamer
# XZ_LEVEL=1 ./linux-build.sh       iterate without waiting on compression,
#                                   which is most of the wall time of a build.
# ASSET_BASE_URL=https://...        where the manifest should say the artifacts
#                                   live; see build/make-manifest.sh.
# SKIP_DEPS=1                       do not apt-get anything; the host is set up.
#
set -euo pipefail

REPO=$(cd "$(dirname "$0")" && pwd)
XZ_LEVEL=${XZ_LEVEL:-6}
BEAMER_USER=${BEAMER_USER:-beamer}
BEAMER_USER_PASS=${BEAMER_USER_PASS:-}

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

say() { echo; echo "==> $*"; }

# --- host checks ----------------------------------------------------------
[[ "$(uname -s)" == "Linux" ]] || {
    echo "ERROR: this builds on Linux. On macOS use ./mac-build.sh, which" >&2
    echo "wraps this same build in a lima VM." >&2
    exit 1
}

sudo -n true 2>/dev/null || {
    echo "ERROR: passwordless sudo is required - the build chroots and needs" >&2
    echo "loop devices. Run 'sudo -v' first if your sudo takes a password." >&2
    exit 1
}

# --- dependencies ---------------------------------------------------------
apt_get() {
    local limit=$1; shift
    local what=$1 rc=0
    timeout -k 30 "$limit" sudo env \
        DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=a \
        apt-get -y \
            -o Acquire::Retries=3 \
            -o Acquire::http::Timeout=30 \
            -o Acquire::https::Timeout=30 \
            "$@" </dev/null || rc=$?
    (( rc == 0 )) && return 0
    if (( rc == 124 )); then
        echo "ERROR: 'apt-get $what' made no progress within ${limit}s and was killed." >&2
        echo "This is almost always a failing Ubuntu mirror rather than anything" >&2
        echo "wrong with the build - check the Ign:/Err: lines above. Re-running is" >&2
        echo "usually enough. SKIP_DEPS=1 skips this block on a host already set up." >&2
    else
        echo "ERROR: 'apt-get $what' failed (exit $rc)." >&2
    fi
    exit 1
}

# Keep in step with the first provision block of build/piforge.yaml: that list
# and this one describe the same build host, one for lima and one for a real
# Linux box, and a build that needs a new tool needs it in both.
if [[ "${SKIP_DEPS:-0}" == "1" ]]; then
    say "skipping dependency install (SKIP_DEPS=1)"
else
    say "refreshing package lists"
    apt_get 600 update

    say "installing build dependencies"
    apt_get 900 install --no-install-recommends \
        e2fsprogs dosfstools fdisk mtools xz-utils curl ca-certificates \
        qemu-user-static binfmt-support binutils

    say "dependencies installed"
fi

# --- qemu-arm in binfmt_misc ----------------------------------------------
if [[ ! -e /proc/sys/fs/binfmt_misc/qemu-arm ]]; then
    say "registering qemu-arm in binfmt_misc"
    sudo update-binfmts --enable qemu-arm || true
fi
[[ -e /proc/sys/fs/binfmt_misc/qemu-arm ]] || {
    echo "ERROR: qemu-arm is not registered in binfmt_misc on this host." >&2
    echo "The armhf chroot cannot execute anything without it. Check:" >&2
    echo "  update-binfmts --display | grep -A3 qemu-arm" >&2
    echo "  ls -l /proc/sys/fs/binfmt_misc/" >&2
    exit 1
}
[[ -x /usr/bin/qemu-arm-static ]] || {
    echo "ERROR: /usr/bin/qemu-arm-static is missing." >&2
    echo "build/targets/armhf.conf points QEMU_STATIC at it and build-image.sh" >&2
    echo "copies it into the rootfs. Install qemu-user-static." >&2
    exit 1
}

# --- build ----------------------------------------------------------------
chmod +x "$REPO"/build/*.sh "$REPO/scripts/make-fs.sh"

mkdir -p "$CACHE" "$DIST"

restore_ownership() { sudo chown -R "$(id -u):$(id -g)" "$CACHE" "$DIST" 2>/dev/null || true; }
trap restore_ownership EXIT

for t in "${TARGETS[@]}"; do
    say "building $t"
    [[ "$t" == armhf ]] && echo "(armhf runs under qemu - expect this to take a while)"
    sudo env XZ_LEVEL="$XZ_LEVEL" \
        BEAMER_USER="$BEAMER_USER" \
        BEAMER_USER_PASS="$BEAMER_USER_PASS" \
        "$REPO/build/build-image.sh" "$t"
done

restore_ownership

say "generating Imager manifest"
"$REPO/build/make-manifest.sh"

say "done"
ls -lh "$DIST"
