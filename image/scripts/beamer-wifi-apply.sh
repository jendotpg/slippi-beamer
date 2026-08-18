#!/bin/bash
# Deliberately NOT set -e, unlike most of the scripts here: every step is
# "try this, and report it if it did not work". Every path exits 0.
set -uo pipefail

source /usr/local/lib/beamer/beamer-common.sh

COUNTRY_FILE="$BEAMER_STATE/wifi-country"
CHANGED_FLAG="$BEAMER_STATE/wifi-changed"

[[ $EUID -eq 0 ]] || { echo "run as root" >&2; exit 1; }

beamer_purge_foreign_wifi

COUNTRY=$(head -n1 "$COUNTRY_FILE" 2>/dev/null || true)

if [[ -n "$COUNTRY" ]]; then
    if [[ -e "$CHANGED_FLAG" ]]; then
        if command -v raspi-config >/dev/null 2>&1; then
            raspi-config nonint do_wifi_country "$COUNTRY" >/dev/null 2>&1 \
                || iw reg set "$COUNTRY" >/dev/null 2>&1
        else
            iw reg set "$COUNTRY" >/dev/null 2>&1
        fi
        rm -f "$CHANGED_FLAG"
        beamer_log wifi-apply "set regulatory domain to $COUNTRY"
    else
        iw reg set "$COUNTRY" >/dev/null 2>&1
    fi
fi

rfkill unblock wifi >/dev/null 2>&1

if systemctl is-active --quiet NetworkManager 2>/dev/null; then
    nmcli connection reload >/dev/null 2>&1
fi

exit 0
