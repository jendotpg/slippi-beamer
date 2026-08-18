#!/bin/bash
# Never modify the live image any other way while a host has it mounted.
set -euo pipefail

source /usr/local/lib/beamer/beamer-common.sh

LUN=/sys/kernel/config/usb_gadget/beamer/functions/mass_storage.0/lun.0
IMAGE=/srv/gadget.img
TEMPLATE=/srv/gadget-template.img
SAVED=
MARKER=
RESTORE_TIMER=0

[[ $EUID -eq 0 ]] || { echo "run as root" >&2; exit 1; }
[[ -d "$LUN" ]] || { echo "gadget not up" >&2; exit 1; }
[[ -e "$TEMPLATE" ]] || { echo "$TEMPLATE missing" >&2; exit 1; }

cleanup() {
    rm -f "$BEAMER_RESET_FLAG"
    if [[ -n "$SAVED" ]]; then rm -f "$SAVED"; fi
    if [[ -n "$MARKER" ]]; then rm -f "$MARKER"; fi
    if (( RESTORE_TIMER )); then
        systemctl start status-check.timer >/dev/null 2>&1 || true
    fi
    :
}
trap cleanup EXIT
trap 'exit 143' TERM
trap 'exit 130' INT

beamer_dirs
: > "$BEAMER_RESET_FLAG"
if systemctl stop status-check.timer status-check.service flush-gadget-data.service >/dev/null 2>&1; then
    RESTORE_TIMER=1
fi

echo "" > "$LUN/file" 2>/dev/null || {
    echo 1 > "$LUN/forced_eject"
    echo "" > "$LUN/file"
}

SAVED=$(mktemp)
SAVED_CONF=0
if mcopy -n -i "$BEAMER_GADGET_FAT" ::/CONFIG/config.txt "$SAVED" >/dev/null 2>&1; then
    SAVED_CONF=1
fi

cp --sparse=always "$TEMPLATE" "$IMAGE"

if (( SAVED_CONF )); then
    mcopy -o -i "$BEAMER_GADGET_FAT" "$SAVED" ::/CONFIG/config.txt >/dev/null 2>&1 || true
fi
rm -f "$SAVED"
SAVED=

MARKER=$(mktemp)
{
    printf 'Beamer initialization finished.\n'
    printf '\n'
    printf 'station %s\n' "$(beamer_station_id)"
    printf 'time    %s\n' "$(date -u '+%Y-%m-%d %H:%M:%S UTC')"
    printf '\n'
    printf 'This drive is safe to eject and unplug.\n'
    printf 'To put the station on a network, fill in CONFIG/config.txt and reboot the Pi.\n'
} | sed 's/$/\r/' > "$MARKER"
mcopy -o -i "$BEAMER_GADGET_FAT" "$MARKER" ::/CONFIG/init-finished.txt >/dev/null 2>&1 || true
rm -f "$MARKER"
MARKER=

beamer_mirror_errors

echo "$IMAGE" > "$LUN/file"

minfo g: > /dev/null && echo "reset OK"
