#!/bin/bash
# Generate the Raspberry Pi Imager OS manifest that lists the beamer artifacts.
#
#   ./make-manifest.sh              -> ../dist/beamer.rpi-imager-manifest
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/.." && pwd)
DIST=${DIST:-$REPO/dist}
OUT="$DIST/beamer.rpi-imager-manifest"

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

ENTRIES=()
for target in armhf; do
    case "$target" in
        armhf) devices='"pi3-32bit", "pi1-32bit"'; boards='Pi Zero W and Pi Zero 2 W' ;;
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

    say "$NAME -> $DATE, $boards"
    ENTRIES[${#ENTRIES[@]}]="$(cat <<EOF
    {
      "name": "Beamer station $DATE ($target)",
      "description": "Wii Slippi beamer station. $boards.",
      "icon": "https://downloads.raspberrypi.com/raspios_armhf/Raspberry_Pi_OS_(32-bit).png",
      "url": "file://$IMG",
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
    echo '  "os_list": ['
    sep=""
    for entry in "${ENTRIES[@]}"; do
        printf '%s%s' "$sep" "$entry"
        sep=$',\n'
    done
    printf '\n  ]\n}\n'
} > "$OUT"

python3 -m json.tool "$OUT" >/dev/null || { echo "ERROR: emitted invalid JSON" >&2; exit 1; }

cat <<EOF

wrote $OUT

Load it into Imager with 
App Options -> Content Repository -> EDIT -> Use custom file -> APPLY & RESTART.
Imager asks again on every restart, by design.
EOF
