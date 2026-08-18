#!/bin/bash
# Refresh CONFIG/status.txt the moment the host ejects the medium, then power
# the station off.
#
# Deliberately NOT set -e, for the same reason load-conf.sh is not: a status
# write failing must can't keep a station from shutting down.
set -uo pipefail

source /usr/local/lib/beamer/beamer-common.sh

G=/sys/kernel/config/usb_gadget/beamer
LUN=$G/functions/mass_storage.0/lun.0
POLL=${POLL:-1}

[[ $EUID -eq 0 ]] || { echo "run as root" >&2; exit 1; }

while [[ -d "$LUN" ]]; do
    BACKING=
    read -r BACKING < "$LUN/file" 2>/dev/null || true
    if [[ -z "$BACKING" && ! -e "$BEAMER_RESET_FLAG" ]]; then
        break
    fi
    sleep "$POLL"
done

if [[ ! -d "$LUN" ]]; then
    exit 0
fi

UDC_NOW=
read -r UDC_NOW < "$G/UDC" 2>/dev/null || true
[[ -n "$UDC_NOW" ]] || exit 0

beamer_log eject-watch "host ejected the medium; refreshing CONFIG/status.txt"

systemctl stop status-check.timer status-check.service flush-gadget-data.service

beamer_snapshot_status
beamer_read_status "$BEAMER_STATUS_LATE"
beamer_write_status "$(cat "$BEAMER_WIFI_OUTCOME" 2>/dev/null || true)" \
                    "$BEAMER_LAST_NET" "$BEAMER_LAST_IP" ""
beamer_mirror_errors

sync

beamer_led off

beamer_log eject-watch "powering off"
systemctl --no-block poweroff
exit 0
