#!/bin/bash
# Everything that has to happen in the pre-bind window - while /srv/gadget.img is
# still ours to write - and nothing that does not. Run by
# beamer-preflight.service, ordered Before=gadget.service.
#
# Deliberately NOT set -e, unlike most of the scripts here: every step is
# "try this, and report it if it did not work". Every path exits 0.
set -uo pipefail

source /usr/local/lib/beamer/beamer-common.sh

TEMPLATE=/srv/gadget-template.img
NM_DIR=/etc/NetworkManager/system-connections
PROFILE="$NM_DIR/beamer-wifi.nmconnection"

[[ $EUID -eq 0 ]] || { echo "run as root" >&2; exit 1; }

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
beamer_dirs

BEAMER_STEP_US=${EPOCHREALTIME/./}
BEAMER_START_US=$BEAMER_STEP_US
step() {
    local now=${EPOCHREALTIME/./}
    beamer_log preflight "$1 +$(( (now - BEAMER_STEP_US) / 1000 ))ms"
    BEAMER_STEP_US=$now
}
total() {
    beamer_log preflight "total $(( (${EPOCHREALTIME/./} - BEAMER_START_US) / 1000 ))ms"
}

trim() {
    local s=$1
    s=${s#"${s%%[![:space:]]*}"}
    s=${s%"${s##*[![:space:]]}"}
    REPLY=$s
}

unquote() {
    local s=$1
    if (( ${#s} >= 2 )); then
        if [[ ${s:0:1} == '"' && ${s: -1} == '"' ]]; then
            s=${s:1:${#s}-2}
        elif [[ ${s:0:1} == "'" && ${s: -1} == "'" ]]; then
            s=${s:1:${#s}-2}
        fi
    fi
    REPLY=$s
}

ssid_bytes() {
    printf '%s' "$1" | od -An -tu1 -v | tr -s ' ' '\n' | sed '/^$/d;s/$/;/' | tr -d '\n'
}

finish() {
    beamer_dirs
    printf '%s\n' "$1" > "$BEAMER_WIFI_OUTCOME"

    beamer_apply_hostname
    step "applied the hostname"

    beamer_write_status "$1" "$BEAMER_LAST_NET" "$BEAMER_LAST_IP" " (as of the previous boot)"
    step "wrote CONFIG/status.txt"
    beamer_mirror_errors
    step "mirrored CONFIG/error.txt"
    total
    exit 0
}

beamer_rotate_status
beamer_read_status "$BEAMER_STATUS_PREV"

beamer_rotate_errors

rm -f "$BEAMER_NET_RESULT"
step "rotated state"

# --- locate the config ----------------------------------------------------
if [[ -e "$BEAMER_GADGET_IMG" ]]; then
    SRC=$BEAMER_GADGET_IMG
elif [[ -e "$TEMPLATE" ]]; then
    SRC=$TEMPLATE
else
    beamer_error load-conf "No gadget image found; cannot read CONFIG/config.txt." \
                           "Left the current network settings in place."
    finish "no gadget image, network left as-is"
fi

CONF="$WORK/config.txt"
if ! mcopy -n -i "${SRC}@@${BEAMER_GADGET_OFFSET}" ::/CONFIG/config.txt "$CONF" 2>"$WORK/mcopy.err"; then
    beamer_error load-conf "Could not read CONFIG/config.txt from the gadget filesystem." \
                           "$(head -n1 "$WORK/mcopy.err" 2>/dev/null)" \
                           "Left the current network settings in place."
    finish "unreadable, network left as-is"
fi
step "read CONFIG/config.txt off the image"

# --- parse ----------------------------------------------------------------
SSID=
PASSWORD=
COUNTRY=
HIDDEN=false
STATION_NAME=
NUM_REPLAYS=

while IFS= read -r line || [[ -n $line ]]; do
    line=${line%$'\r'}
    trim "$line"; line=$REPLY
    [[ -z $line || ${line:0:1} == '#' ]] && continue
    [[ $line == *=* ]] || continue
    trim "${line%%=*}"; key=$REPLY
    trim "${line#*=}"; unquote "$REPLY"; val=$REPLY
    case ${key^^} in
        SSID)     SSID=$val ;;
        PASSWORD) PASSWORD=$val ;;
        COUNTRY)  COUNTRY=$val ;;
        HIDDEN)   HIDDEN=$val ;;
        STATION-NAME|STATION_NAME)             STATION_NAME=$val ;;
        NUM-REPLAYS-SERVED|NUM_REPLAYS_SERVED) NUM_REPLAYS=$val ;;
    esac
done < "$CONF"

# --- validate -------------------------------------------------------------
# The whole file is accepted or the whole file is rejected. Nothing here falls
# back to a default and carries on: a station's behaviour has to be a function
# of its config file and nothing else, or two cards holding the same file stop
# being interchangeable. See "Fleet determinism" in the README.
ok=1

if (( ${#STATION_NAME} > 63 )); then
    beamer_error load-conf "STATION-NAME is ${#STATION_NAME} characters; the maximum is 63." \
                           "Shorten it in CONFIG/config.txt."
    ok=0
elif [[ $STATION_NAME == *[[:cntrl:]]* ]]; then
    beamer_error load-conf "STATION-NAME contains a control character, which cannot be stored." \
                           "Remove it in CONFIG/config.txt."
    ok=0
fi

if [[ -n $NUM_REPLAYS ]] \
   && { [[ ! $NUM_REPLAYS =~ ^[0-9]+$ ]] \
        || (( NUM_REPLAYS < 1 || NUM_REPLAYS > BEAMER_KEEP_MAX )); }; then
    beamer_error load-conf "NUM-REPLAYS-SERVED must be a whole number from 1 to $BEAMER_KEEP_MAX (got \"$NUM_REPLAYS\")." \
                           "Fix it in CONFIG/config.txt."
    ok=0
fi

# A blank SSID is a valid configuration - it is how a station is deliberately
# kept off the network - so the wifi keys are only checked when there is one.
if [[ -n $SSID ]]; then
    ssid_len=$(printf '%s' "$SSID" | wc -c)
    if (( ssid_len > 32 )); then
        beamer_error load-conf "SSID is $ssid_len bytes; the maximum is 32." \
                               "Shorten it in CONFIG/config.txt."
        ok=0
    fi

    if [[ -n $PASSWORD ]] && (( ${#PASSWORD} < 8 || ${#PASSWORD} > 63 )); then
        beamer_error load-conf "PASSWORD must be 8-63 characters (got ${#PASSWORD})." \
                               "Fix it in CONFIG/config.txt, or leave PASSWORD blank for an open network."
        ok=0
    fi

    if [[ ! $COUNTRY =~ ^[A-Za-z]{2}$ ]]; then
        beamer_error load-conf "COUNTRY must be two letters (got \"$COUNTRY\")." \
                               "Use a code like US, CA, JP or GB in CONFIG/config.txt."
        ok=0
    else
        COUNTRY=${COUNTRY^^}
    fi
fi

case ${HIDDEN,,} in
    true|yes|1) HIDDEN=true ;;
    *)          HIDDEN=false ;;
esac

if (( ! ok )); then
    beamer_clear_wifi
    rm -f "$BEAMER_STATION_NAME_FILE" "$BEAMER_KEEP_FILE" "$BEAMER_WIFI_COUNTRY"
    finish "rejected, see error.txt - no network"
fi

# --- apply ----------------------------------------------------------------
printf '%s\n' "${STATION_NAME:-$(beamer_station_id)}" > "$BEAMER_STATION_NAME_FILE"
printf '%s\n' "${NUM_REPLAYS:-$BEAMER_KEEP_DEFAULT}" > "$BEAMER_KEEP_FILE"
step "applied the station name and replay count"

if [[ -z $SSID ]]; then
    beamer_log load-conf "CONFIG/config.txt has no SSID; taking the station off the network"
    beamer_clear_wifi
    finish "not configured (SSID is blank) - no network"
fi

# --- idempotency ----------------------------------------------------------
step "parsed and validated"

HASH=$(printf '%s\0' "$SSID" "$PASSWORD" "$COUNTRY" "$HIDDEN" | sha256sum | cut -d' ' -f1)
printf '%s\n' "$COUNTRY" > "$BEAMER_WIFI_COUNTRY"
step "hashed"
if [[ -f $PROFILE && -f $BEAMER_WIFI_HASH && "$(cat "$BEAMER_WIFI_HASH")" == "$HASH" ]]; then
    printf '%s\n' "$SSID" > "$BEAMER_EXPECT_SSID"
    finish "unchanged SSID=\"$SSID\""
fi

rm -f "$BEAMER_ERR_PREV"

: > "$BEAMER_WIFI_CHANGED"

# --- profile --------------------------------------------------------------
TMP="$WORK/beamer-wifi.nmconnection"
{
    printf '[connection]\n'
    printf 'id=beamer-wifi\n'
    printf 'type=wifi\n'
    printf 'interface-name=wlan0\n'
    printf 'autoconnect=true\n'
    printf '\n[wifi]\n'
    printf 'mode=infrastructure\n'
    printf 'ssid=%s\n' "$(ssid_bytes "$SSID")"
    [[ $HIDDEN == true ]] && printf 'hidden=true\n'
    if [[ -n $PASSWORD ]]; then
        printf '\n[wifi-security]\n'
        printf 'key-mgmt=wpa-psk\n'
        printf 'psk=%s\n' "$PASSWORD"
    fi
    printf '\n[ipv4]\n'
    printf 'method=auto\n'
    printf '\n[ipv6]\n'
    printf 'method=auto\n'
} > "$TMP"

mkdir -p "$NM_DIR"
if ! install -m 0600 -o root -g root "$TMP" "$PROFILE.new" || ! mv -f "$PROFILE.new" "$PROFILE"; then
    rm -f "$PROFILE.new"
    beamer_error load-conf "Could not write the NetworkManager profile to $PROFILE."
    beamer_clear_wifi
    finish "failed to write profile - no network"
fi

printf '%s\n' "$SSID" > "$BEAMER_EXPECT_SSID"
printf '%s\n' "$HASH" > "$BEAMER_WIFI_HASH"

beamer_log load-conf "staged SSID=\"$SSID\" country=$COUNTRY hidden=$HIDDEN"
finish "applied SSID=\"$SSID\" country=$COUNTRY"
