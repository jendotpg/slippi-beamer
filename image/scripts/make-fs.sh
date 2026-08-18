#!/bin/bash
# Build the blank gadget image template - what the Wii sees as a USB stick.
#
# Needs losetup, so it must NOT run inside a chroot; the FAT32 bytes it 
# produces are architecture-independent, so building on the builder and 
# copying the result into the rootfs is equivalent to building it on the Pi.
#
# Also safe to run directly as root on a live Pi.
#
#   make-fs.sh [PATH]        default /srv/gadget-template.img
#   FORCE=1 make-fs.sh       rebuild an existing template
set -euo pipefail

TEMPLATE=${1:-/srv/gadget-template.img}
SIZE=1G

[[ $EUID -eq 0 ]] || { echo "run as root" >&2; exit 1; }

if [[ -e "$TEMPLATE" && "${FORCE:-0}" != "1" ]]; then
    echo "$TEMPLATE exists, leaving it (FORCE=1 to rebuild)"
    exit 0
fi

# 1 GB, MBR partition table, type 0x0c, first partition starting at sector 2048.
# This mimics a real USB stick; libfat/IOS expect this rather than a superfloppy
# layout.
#
# -s 8 gives 4 KB clusters. Do NOT use larger clusters at this size: FAT32
# requires at least 65525 clusters, and 1 GB / 32 KB is only 32768, which
# produces an invalid or refused filesystem. 4 KB gives 262144 clusters.
mkdir -p "$(dirname "$TEMPLATE")"
rm -f "$TEMPLATE"
truncate -s "$SIZE" "$TEMPLATE"
sfdisk "$TEMPLATE" <<'PART'
label: dos
start=2048, type=c
PART

LOOP=$(losetup -fP --show "$TEMPLATE")
trap 'losetup -d "$LOOP" 2>/dev/null || true' EXIT
[[ -e "${LOOP}p1" ]] || { echo "ERROR: partition scan failed" >&2; exit 1; }
mkfs.vfat -F 32 -s 8 -n BEAMER "${LOOP}p1"
blkid "${LOOP}p1"
losetup -d "$LOOP"
trap - EXIT

IMG="$TEMPLATE@@1048576"
mmd -i "$IMG" ::/CONFIG
mmd -i "$IMG" ::/SLIPPI

CONF=$(mktemp)
trap 'rm -f "$CONF"' EXIT
sed 's/$/\r/' > "$CONF" <<'EOF'
# Beamer station configuration.
#
# Fill this in, save, eject this drive, then move the cable back to the Wii.
# The Pi reads this file at every boot.
#
# Until SSID is filled in, this station has no network. That is expected.
# After a reboot, check CONFIG/status.txt; if CONFIG/error.txt exists, it says
# what went wrong.
#
# SSID / PASSWORD    the network to join. Leave PASSWORD blank for an open
#                    network; otherwise it is 8-63 characters.
# COUNTRY            two-letter regulatory domain: US, CA, JP, GB...
# HIDDEN             true or false - whether the network broadcasts its name.
# STATION-NAME       what to call this station, on the status file and over the
#                    network. Blank means use the station's ID.
# NUM-REPLAYS-SERVED how many of the newest replays this station hands out over
#                    HTTP. 1 to 16.

SSID=
PASSWORD=
COUNTRY=US
HIDDEN=false
STATION-NAME=
NUM-REPLAYS-SERVED=10
EOF
mcopy -o -i "$IMG" "$CONF" ::/CONFIG/config.txt
rm -f "$CONF"
trap - EXIT

echo "built $TEMPLATE"
