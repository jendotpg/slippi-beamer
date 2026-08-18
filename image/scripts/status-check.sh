#!/bin/bash
#
# Runs every 10s from status-check.timer, and on demand from POST /status.
# Nothing here sleeps or probes the network - that is health-check.sh, on its own
# much slower timer, so this can stay cheap enough to run six times a minute
# forever. Writes only to memory, never disk.
set -euo pipefail

BEAMER_ERROR_LATE=1

source /usr/local/lib/beamer/beamer-common.sh

# Ticks between forced full listings. At 10s a tick this is once a minute.
FORCE_EVERY=${FORCE_EVERY:-6}

F_UDC=       F_BIND=      F_HOST_STATE=
F_MTOOLS=0   F_SLIPPI=    F_CAPPED=0

beamer_dirs

write_fragment() {
    local tmp=$BEAMER_STATUS_FRAG.$$
    {
        printf '  "udc": %s,\n'                 "$(beamer_json_strn "$F_UDC")"
        printf '  "bind_time_s": %s,\n'         "$(beamer_json_num  "$F_BIND")"
        printf '  "host_state": %s,\n'          "$(beamer_json_strn "$F_HOST_STATE")"
        printf '  "mtools": %s,\n'              "$(beamer_json_bool "$F_MTOOLS")"
        printf '  "slippi_files": %s,\n'        "$(beamer_json_num  "$F_SLIPPI")"
        printf '  "slippi_files_capped": %s,\n' "$(beamer_json_bool "$F_CAPPED")"
    } > "$tmp"
    chmod 0644 "$tmp"
    mv -f "$tmp" "$BEAMER_STATUS_FRAG"
}

publish() {
    write_fragment
    beamer_write_report
}

# --- the gadget -----------------------------------------------------------
read -r F_UDC < /sys/kernel/config/usb_gadget/beamer/UDC 2>/dev/null || true
if [[ -z "$F_UDC" ]]; then
    beamer_error status-check "The gadget did not bind to a UDC." \
                              "Almost always dwc2 not loaded. Check 'lsmod | grep dwc2' and that" \
                              "the config.txt/cmdline.txt edits survived the boot."
    publish
    exit 1
fi

read -r F_BIND < "$BEAMER_BIND_UPTIME" 2>/dev/null || true

for f in /sys/class/udc/*/state; do
    if [[ -e $f ]]; then
        read -r F_HOST_STATE < "$f" 2>/dev/null || true
    fi
done

# --- is there a new file to flush? ----------------------------------------
IMG_TOUCHED=0
if [[ "$BEAMER_GADGET_IMG" -nt "$BEAMER_IMG_STAMP" ]]; then
    IMG_TOUCHED=1
fi

TICK=0
if [[ -r "$BEAMER_IMG_STAMP" ]]; then
    read -r TICK < "$BEAMER_IMG_STAMP" 2>/dev/null || true
    [[ "$TICK" =~ ^[0-9]+$ ]] || TICK=0
fi
TICK=$(( (TICK + 1) % FORCE_EVERY ))
printf '%s\n' "$TICK" > "$BEAMER_IMG_STAMP"

LIVE_PREV=
if [[ -r "$BEAMER_LIVE_GAME" ]]; then
    read -r LIVE_PREV < "$BEAMER_LIVE_GAME" 2>/dev/null || true
fi

NEED_LIST=0
LIVE_NOW=
GAME_INFO=

if (( TICK == 0 )); then
    NEED_LIST=1
elif [[ -n "$LIVE_PREV" ]]; then
    if beamer_peek_slp "::/SLIPPI/$LIVE_PREV"; then
        GAME_INFO=$BEAMER_GAME_INFO
        if [[ "$BEAMER_GAME_STATE" == live ]]; then
            LIVE_NOW=$LIVE_PREV
        else
            NEED_LIST=1
        fi
    else
        NEED_LIST=1
    fi
elif (( IMG_TOUCHED )); then
    NEED_LIST=1
fi

# --- the expensive branch -------------------------------------------------
if (( NEED_LIST )); then
    if minfo g: > /dev/null 2>&1; then
        F_MTOOLS=1
    else
        beamer_error status-check "Could not read the gadget filesystem with mtools." \
                                  "The image may be missing or corrupt; try reset-gadget-data.sh."
        publish
        exit 1
    fi

    beamer_slippi_count
    F_SLIPPI=$BEAMER_SLIPPI_FILES
    F_CAPPED=$BEAMER_SLIPPI_CAPPED
    printf '%s %s %s\n' "$F_MTOOLS" "$F_SLIPPI" "$F_CAPPED" > "$BEAMER_DRIVE_CACHE"

    if [[ -n "$BEAMER_SLIPPI_NEWEST" ]]; then
        if beamer_peek_slp "$BEAMER_SLIPPI_NEWEST"; then
            GAME_INFO=$BEAMER_GAME_INFO
            if [[ "$BEAMER_GAME_STATE" == live ]]; then
                LIVE_NOW=${BEAMER_SLIPPI_NEWEST##*/}
            fi
        else
            LIVE_NOW=${BEAMER_SLIPPI_NEWEST##*/}
        fi
    fi

    /usr/local/sbin/flush-gadget-data.sh || \
        beamer_log status-check "flush-gadget-data.sh failed"
else
    if [[ -r "$BEAMER_DRIVE_CACHE" ]]; then
        read -r F_MTOOLS F_SLIPPI F_CAPPED < "$BEAMER_DRIVE_CACHE" 2>/dev/null || true
    fi
fi

# --- record what is being played ------------------------------------------
if [[ "$LIVE_NOW" != "$LIVE_PREV" ]]; then
    if [[ -n "$LIVE_NOW" ]]; then
        beamer_log status-check "game in progress: $LIVE_NOW"
    else
        beamer_log status-check "finished game: $LIVE_PREV"
    fi
fi
printf '%s\n' "$LIVE_NOW" > "$BEAMER_LIVE_GAME"

if [[ -n "$GAME_INFO" ]]; then
    printf '%s\n' "$GAME_INFO" > "$BEAMER_GAME_JSON"
else
    : > "$BEAMER_GAME_JSON"
fi

publish
