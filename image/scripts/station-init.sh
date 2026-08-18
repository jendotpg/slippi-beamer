#!/bin/bash
# Per-station first boot. Everything here depends on the specific board it runs
# on, which is why it cannot be baked into the image with the rest of
# provisioning.
set -euo pipefail

source /usr/local/lib/beamer/beamer-common.sh

TEMPLATE=/srv/gadget-template.img
IMAGE=/srv/gadget.img

# Fixed project namespace for the UUIDv5 derivation. Changing this value
# re-identifies the entire fleet; it is a constant, not a tunable.
NS=1eba09f7-1bc0-42a3-b69f-697f76c34358

[[ $EUID -eq 0 ]] || { echo "run as root" >&2; exit 1; }

beamer_dirs

# --- station identity -----------------------------------------------------
if [[ ! -s "$BEAMER_STATION_ID" ]]; then
    SERIAL=$(sed -n 's/^Serial[[:space:]]*:[[:space:]]*//p' /proc/cpuinfo | tail -n1)
    if [[ -z "$SERIAL" || "$SERIAL" =~ ^0+$ ]]; then
        beamer_error station-init "No usable CPU serial in /proc/cpuinfo (got '${SERIAL:-}')." \
                                  "Refusing to fall back to a random ID - that would hand out a" \
                                  "new station identity on every reflash without anyone noticing."
        exit 1
    fi
    python3 -c 'import sys, uuid; print(uuid.uuid5(uuid.UUID(sys.argv[1]), sys.argv[2]))' \
        "$NS" "$SERIAL" > "$BEAMER_STATION_ID"
fi

STATION=$(cat "$BEAMER_STATION_ID")
[[ -n "$STATION" ]] || { echo "ERROR: $BEAMER_STATION_ID is empty" >&2; exit 1; }

beamer_apply_hostname
echo "station: $STATION"

# --- live image -----------------------------------------------------------
[[ -e "$TEMPLATE" ]] || { echo "ERROR: $TEMPLATE missing from the image" >&2; exit 1; }
if [[ -e "$IMAGE" ]]; then
    echo "$IMAGE exists, leaving it (use reset-gadget-data.sh to wipe)"
else
    cp --sparse=always "$TEMPLATE" "$IMAGE"
    echo "created $IMAGE"
fi

# --- gadget-side markers --------------------------------------------------
MARKER=$(mktemp)
{
    printf 'Beamer initialization finished.\n'
    printf '\n'
    printf 'station %s\n' "$STATION"
    printf 'time    %s\n' "$(date -u '+%Y-%m-%d %H:%M:%S UTC')"
    printf '\n'
    printf 'This drive is safe to eject and unplug.\n'
    printf 'To put the station on a network, fill in CONFIG/config.txt and reboot the Pi.\n'
} | sed 's/$/\r/' > "$MARKER"
mmd -i "$BEAMER_GADGET_FAT" ::/CONFIG >/dev/null 2>&1 || true
mcopy -o -i "$BEAMER_GADGET_FAT" "$MARKER" ::/CONFIG/init-finished.txt >/dev/null 2>&1 || true
rm -f "$MARKER"

beamer_mirror_errors

touch "$BEAMER_PROVISIONED"
echo "station-init complete"
