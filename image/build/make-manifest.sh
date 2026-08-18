#!/bin/bash
# Generate the Raspberry Pi Imager OS manifest that lists the beamer artifacts.
#
#   ./make-manifest.sh              -> ../dist/beamer.rpi-imager-manifest
#
# ASSET_BASE_URL=https://...      where the artifacts will be served from. Unset,
#                                 entries point at the local file, which is what
#                                 you want for flashing a card off your own disk.
#                                 Set, they point at <base>/<filename> - that is
#                                 how the release manifest gets URLs a stranger
#                                 can resolve. See ../../.github/workflows/publish.yml.
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/.." && pwd)
DIST=${DIST:-$REPO/dist}
OUT="$DIST/beamer.rpi-imager-manifest"
ASSET_BASE_URL=${ASSET_BASE_URL:-}

command -v python3 >/dev/null || { echo "ERROR: python3 not found" >&2; exit 1; }
command -v xz      >/dev/null || { echo "ERROR: xz not found" >&2; exit 1; }

# sha256sum on Linux, shasum -a 256 on macOS.
if command -v sha256sum >/dev/null; then
    sha256() { sha256sum | cut -d' ' -f1; }
elif command -v shasum >/dev/null; then
    sha256() { shasum -a 256 | cut -d' ' -f1; }
else
    echo "ERROR: no sha256sum or shasum" >&2; exit 1
fi

say() { echo "==> $*"; }

IMAGER_DEVICES=$(cat <<'EOF'
  "imager": {
    "devices": [
      {
        "name": "Raspberry Pi Zero W",
        "description": "Zero W and Zero WH - the only supported board. Plain Zero (no W) lacks wifi and will not work.",
        "icon": "https://downloads.raspberrypi.com/imager/icons/RPi_Zero.png",
        "tags": ["pi1-32bit"],
        "matching_type": "exclusive"
      },
      {
        "name": "Raspberry Pi Zero 2 W",
        "description": "Zero 2 W and Zero 2 WH. Untested, but wired like the Zero W.",
        "icon": "https://downloads.raspberrypi.com/imager/icons/RPi_Zero_2_W.png",
        "tags": ["pi3-64bit", "pi3-32bit"],
        "matching_type": "exclusive"
      },
      {
        "name": "Raspberry Pi 3 Model A+",
        "description": "Untested. Needs a USB A-to-A cable and its own power supply. Not the 3B or 3B+ - those cannot do device mode at all.",
        "icon": "https://downloads.raspberrypi.com/imager/icons/RPi_3.png",
        "tags": ["pi3-64bit", "pi3-32bit"],
        "matching_type": "exclusive"
      },
      {
        "name": "Raspberry Pi 4 Model B",
        "description": "Untested. Device mode is on the USB-C port, so the board needs its own 5V on the GPIO header.",
        "icon": "https://downloads.raspberrypi.com/imager/icons/RPi_4.png",
        "tags": ["pi4-64bit", "pi4-32bit"],
        "matching_type": "exclusive"
      },
      {
        "name": "Raspberry Pi 400",
        "description": "Untested. Same USB-C power caveat as the Pi 4, and no activity LED to read station status off.",
        "icon": "https://downloads.raspberrypi.com/imager/icons/RPi_4.png",
        "tags": ["pi4-64bit", "pi4-32bit"],
        "matching_type": "exclusive"
      },
      {
        "name": "Raspberry Pi 5",
        "description": "Untested. Device mode is on the USB-C port, so the board needs its own 5V on the GPIO header.",
        "icon": "https://downloads.raspberrypi.com/imager/icons/RPi_5.png",
        "tags": ["pi5-64bit", "pi5-32bit"],
        "matching_type": "exclusive"
      },
      {
        "name": "Raspberry Pi 500",
        "description": "Untested. 500 and 500+. Same USB-C power caveat as the Pi 5, and no activity LED to read station status off.",
        "icon": "https://downloads.raspberrypi.com/imager/icons/RPi_5.png",
        "tags": ["pi5-64bit", "pi5-32bit"],
        "matching_type": "exclusive"
      }
    ]
  },
EOF
)

ENTRIES=()
for target in armhf; do
    case "$target" in
        armhf) devices='"pi1-32bit", "pi3-32bit", "pi4-32bit", "pi5-32bit"'
               boards='Built and tested on the Pi Zero W; every other board the picker lists is untested.' ;;
    esac

    IMG=$(ls -t "$DIST"/beamer-*-"$target".img.xz 2>/dev/null | head -1 || true)
    [[ -n "$IMG" ]] || { say "no $target artifact in $DIST, skipping"; continue; }

    NAME=$(basename "$IMG")
    DATE=$(echo "$NAME" | sed -E 's/^beamer-([0-9]{4}-[0-9]{2}-[0-9]{2})-.*/\1/')
    [[ "$DATE" != "$NAME" ]] || { echo "ERROR: cannot read a date out of $NAME" >&2; exit 1; }

    DOWNLOAD_SIZE=$(wc -c < "$IMG" | tr -d ' ')

    if [[ -f "$IMG.meta" ]]; then
        read -r EXTRACT_SIZE EXTRACT_SHA256 < <(python3 -c \
            'import json,sys; m=json.load(open(sys.argv[1])); print(m["extract_size"], m["extract_sha256"])' \
            "$IMG.meta")
    else
        say "no $NAME.meta - decompressing to hash it (slow; the next build caches this)"
        EXTRACT_SIZE=$(xz --robot -l "$IMG" | awk '$1=="totals"{print $5}')
        EXTRACT_SHA256=$(xz -dc "$IMG" | sha256)
    fi
    [[ -n "$EXTRACT_SIZE" && -n "$EXTRACT_SHA256" ]] || {
        echo "ERROR: no uncompressed size/hash for $NAME" >&2; exit 1; }

    if [[ -n "$ASSET_BASE_URL" ]]; then
        URL="${ASSET_BASE_URL%/}/$NAME"
    else
        URL="file://$IMG"
    fi

    say "$NAME -> $DATE"
    ENTRIES[${#ENTRIES[@]}]="$(cat <<EOF
    {
      "name": "Beamer station $DATE ($target)",
      "description": "Wii Slippi beamer station. $boards",
      "icon": "https://downloads.raspberrypi.com/raspios_armhf/Raspberry_Pi_OS_(32-bit).png",
      "url": "$URL",
      "release_date": "$DATE",
      "image_download_size": $DOWNLOAD_SIZE,
      "extract_size": $EXTRACT_SIZE,
      "extract_sha256": "$EXTRACT_SHA256",
      "devices": [$devices]
    }
EOF
)"
done

[[ ${#ENTRIES[@]} -gt 0 ]] || { echo "ERROR: no artifacts in $DIST - build one first" >&2; exit 1; }

{
    echo '{'
    echo "$IMAGER_DEVICES"
    echo '  "os_list": ['
    sep=""
    for entry in "${ENTRIES[@]}"; do
        printf '%s%s' "$sep" "$entry"
        sep=$',\n'
    done
    printf '\n  ]\n}\n'
} > "$OUT.tmp"

python3 - "$OUT.tmp" <<'EOF' || { rm -f "$OUT.tmp"; exit 1; }
import json, sys

path = sys.argv[1]
try:
    m = json.load(open(path))
except ValueError as e:
    sys.exit(f"ERROR: emitted invalid JSON: {e}")

# Both directions dead-end the user: an entry no board can reach is invisible,
# and a board with nothing to offer leaves Next enabled and the OS list empty.
selectable = {t for d in m["imager"]["devices"] for t in d["tags"]}
offered = {t for e in m["os_list"] for t in e["devices"]}

for entry in m["os_list"]:
    tags = set(entry["devices"])
    if not tags & selectable:
        sys.exit(f"ERROR: no device in the picker can select {entry['name']!r} "
                 f"(tagged {sorted(tags)}, picker offers {sorted(selectable)})")

for device in m["imager"]["devices"]:
    tags = set(device["tags"])
    if not tags & offered:
        sys.exit(f"ERROR: the picker offers {device['name']!r} but no image is "
                 f"tagged for it (wants one of {sorted(tags)}, images carry "
                 f"{sorted(offered)})")
EOF

mv "$OUT.tmp" "$OUT"

cat <<EOF

wrote $OUT

Load it into Imager with 
App Options -> Content Repository -> EDIT -> Use custom file -> APPLY & RESTART.
Imager asks again on every restart, by design.
EOF
