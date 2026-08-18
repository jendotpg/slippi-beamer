#!/bin/bash
# Grow the root partition and filesystem to fill the card, once, on first boot.
#
# build-image.sh shrinks the rootfs to its minimum plus ~64 MB so the shipped
# image compresses small. Stock Raspberry Pi OS grows it back from the initramfs
# via the 'resize' cmdline token - but that initramfs cost 8.1 seconds of
# pre-kernel time on a Zero W, so we do not ship one. This is the replacement.
#
# Deliberately ordered late (After=gadget.service): a station that has not
# grown its filesystem yet still works, and nothing here has any business
# sitting between power-on and the Wii seeing its drive.
set -uo pipefail

source /usr/local/lib/beamer/beamer-common.sh

STAMP="$BEAMER_STATE/grown"
MIN_GROW_MB=${MIN_GROW_MB:-64}

# Deliberately NOT "fill the card". A station holds a 1 GB gadget image, at most
# 16 replay files and a capped journal; everything past this is dead space that
# systemd-fsck-root has to walk on every boot. Filling a 239 GB card measured
# 4.2 seconds of fsck in front of the gadget binding, which is more than the
# entire rest of userspace we spent this long optimising.
MAX_ROOT_MB=${MAX_ROOT_MB:-8192}

[[ $EUID -eq 0 ]] || { echo "run as root" >&2; exit 1; }

ROOT_DEV=$(findmnt -no SOURCE / 2>/dev/null)
[[ -b "$ROOT_DEV" ]] || {
    beamer_error growfs "Could not find the root device (findmnt said '${ROOT_DEV:-nothing}')." \
                        "The filesystem was left at its shipped size."
    exit 1
}

case "$ROOT_DEV" in
    *[0-9]p[0-9]*) DISK=${ROOT_DEV%p*};        PART=${ROOT_DEV##*p} ;;
    *)             DISK=${ROOT_DEV%%[0-9]*};   PART=${ROOT_DEV##*[!0-9]} ;;
esac
[[ -b "$DISK" && -n "$PART" ]] || {
    beamer_error growfs "Could not split '$ROOT_DEV' into a disk and a partition number." \
                        "The filesystem was left at its shipped size."
    exit 1
}

DISK_SECTORS=$(blockdev --getsz "$DISK" 2>/dev/null || echo 0)
PART_START=$(cat "/sys/class/block/${ROOT_DEV##*/}/start" 2>/dev/null || echo 0)
PART_SECTORS=$(blockdev --getsz "$ROOT_DEV" 2>/dev/null || echo 0)
FREE_MB=$(( (DISK_SECTORS - PART_START - PART_SECTORS) / 2048 ))
CUR_MB=$(( PART_SECTORS / 2048 ))

TARGET_MB=$(( CUR_MB + FREE_MB ))
(( TARGET_MB > MAX_ROOT_MB )) && TARGET_MB=$MAX_ROOT_MB

if (( TARGET_MB - CUR_MB < MIN_GROW_MB )); then
    beamer_log growfs "root is ${CUR_MB}MB, cap is ${MAX_ROOT_MB}MB, ${FREE_MB}MB unallocated - nothing worth growing"
    touch "$STAMP"
    exit 0
fi

beamer_log growfs "growing $ROOT_DEV from ${CUR_MB}MB to ${TARGET_MB}MB (${FREE_MB}MB available, capped at ${MAX_ROOT_MB}MB)"

if ! sfdisk --no-reread --force -N "$PART" "$DISK" <<<", $(( TARGET_MB * 2048 ))" >/dev/null 2>&1; then
    beamer_error growfs "sfdisk could not extend partition $PART of $DISK." \
                        "The filesystem was left at its shipped size."
    exit 1
fi

partx -u "$DISK" >/dev/null 2>&1 || partprobe "$DISK" >/dev/null 2>&1 || true

if ! resize2fs "$ROOT_DEV" >/dev/null 2>&1; then
    beamer_error growfs "resize2fs failed on $ROOT_DEV after extending the partition." \
                        "The partition is bigger but the filesystem is not;" \
                        "run 'sudo resize2fs $ROOT_DEV' to finish the job."
    exit 1
fi

touch "$STAMP"
beamer_log growfs "root filesystem is now $(findmnt -no SIZE / 2>/dev/null || echo '?')"
exit 0
