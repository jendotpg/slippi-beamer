# Shared helpers for the beamer scripts. Always sourced, never executed.
#
# Some of this runs in the pre-bind window, where every fork+exec is time the
# user spends waiting for the drive to appear - measured at roughly 100ms per
# external command on a Zero W with a cold page cache. So the hot paths here use
# bash builtins (read, printf -v, EPOCHSECONDS) rather than cat/hostname/date.
#
# Every timestamp this project writes is UTC, so set it once instead of passing
# -u to a date(1) we would rather not fork at all.
export TZ=UTC

BEAMER_STATE=/var/lib/beamer # durable
BEAMER_RUN=/run/beamer       # ephemeral

# --- durable: has to survive a power cut ----------------------------------
BEAMER_STATION_ID="$BEAMER_STATE/station-id"
BEAMER_PROVISIONED="$BEAMER_STATE/provisioned"
BEAMER_GROWN="$BEAMER_STATE/grown"
BEAMER_WIFI_HASH="$BEAMER_STATE/wifi.hash"
BEAMER_ERR_LATE="$BEAMER_STATE/error.late.log"
BEAMER_ERR_PREV="$BEAMER_STATE/error.prev.log"
BEAMER_JOURNAL_DIR="$BEAMER_STATE/journald_dumps"
BEAMER_STATUS_LATE="$BEAMER_STATE/status.late"
BEAMER_STATUS_PREV="$BEAMER_STATE/status.prev"

# --- ephemeral: rebuilt every boot ----------------------------------------
BEAMER_ERR_LOG="$BEAMER_RUN/error.log"
BEAMER_REPORT_JSON="$BEAMER_RUN/report.json"
BEAMER_LIVE_GAME="$BEAMER_RUN/live_game"
BEAMER_GAME_JSON="$BEAMER_RUN/game.json"
BEAMER_WITHHELD="$BEAMER_RUN/withheld"
BEAMER_STATUS_FRAG="$BEAMER_RUN/status.frag"
BEAMER_HEALTH_FRAG="$BEAMER_RUN/health.frag"
BEAMER_NET_STATUS="$BEAMER_RUN/net-status"
BEAMER_NET_RESULT="$BEAMER_RUN/net-result"
BEAMER_IMG_STAMP="$BEAMER_RUN/img-stamp"
BEAMER_DRIVE_CACHE="$BEAMER_RUN/drive-cache"
BEAMER_STATION_NAME_FILE="$BEAMER_RUN/station-name"
BEAMER_KEEP_FILE="$BEAMER_RUN/num-replays"
BEAMER_EXPECT_SSID="$BEAMER_RUN/expected-ssid"
BEAMER_WIFI_OUTCOME="$BEAMER_RUN/wifi-outcome"
BEAMER_WIFI_COUNTRY="$BEAMER_RUN/wifi-country"
BEAMER_WIFI_CHANGED="$BEAMER_RUN/wifi-changed"
BEAMER_RESET_FLAG="$BEAMER_RUN/reset-in-progress"
BEAMER_JOURNAL_SLOT="$BEAMER_RUN/journal-slot"

BEAMER_BIND_UPTIME=/run/gadget-bind-uptime # also written by gadget-up.sh
BEAMER_IFACE=${BEAMER_IFACE:-wlan0}
BEAMER_GADGET_IMG=/srv/gadget.img
BEAMER_GADGET_OFFSET=1048576
BEAMER_GADGET_FAT="${BEAMER_GADGET_IMG}@@${BEAMER_GADGET_OFFSET}"
BEAMER_LED_CMD=/usr/local/sbin/beamer-led.sh
BEAMER_WEB_SLIPPI=/var/www/html/SLIPPI
BEAMER_WEB_INDEX="$BEAMER_WEB_SLIPPI/index.json"
BEAMER_SLP_PEEK=/usr/local/lib/beamer/slp-peek
BEAMER_ARCH_FILE=/etc/beamer/arch
BEAMER_KEEP_DEFAULT=10
BEAMER_KEEP_MAX=16
# determined experimentally - past 2000, stat starts to actually cost something
BEAMER_COUNT_CAP=${BEAMER_COUNT_CAP:-2000}
# journal dumps kept in BEAMER_JOURNAL_DIR - unrelated to the replay keep above
BEAMER_JOURNAL_KEEP=${BEAMER_JOURNAL_KEEP:-16}

beamer_dirs() {
    [[ -d $BEAMER_STATE && -d $BEAMER_RUN ]] && return 0
    mkdir -p "$BEAMER_STATE" "$BEAMER_RUN"
}

beamer_station_id() {
    local id=
    if [[ -r "$BEAMER_STATION_ID" ]]; then
        read -r id < "$BEAMER_STATION_ID" || true
    fi
    printf '%s' "${id:-unknown}"
}

beamer_arch() {
    local arch=
    if [[ -r "$BEAMER_ARCH_FILE" ]]; then
        read -r arch < "$BEAMER_ARCH_FILE" || true
    fi
    printf '%s' "${arch:-unknown}"
}

beamer_station_name() {
    local name=
    if [[ -r "$BEAMER_STATION_NAME_FILE" ]]; then
        read -r name < "$BEAMER_STATION_NAME_FILE" || true
    fi
    if [[ -n "$name" ]]; then
        printf '%s' "$name"
    else
        beamer_station_id
    fi
}

# beamer_led boot|ok|error|off
beamer_led() {
    if [[ -x "$BEAMER_LED_CMD" ]]; then
        "$BEAMER_LED_CMD" "$1" >/dev/null 2>&1 || true
    fi
}

beamer_session_has_errors() {
    [[ -s "$BEAMER_ERR_LOG" || -s "$BEAMER_ERR_LATE" ]]
}

beamer_net_result() {
    local net=
    if [[ -r "$BEAMER_NET_RESULT" ]]; then
        read -r net < "$BEAMER_NET_RESULT" 2>/dev/null || true
    fi
    case "$net" in
        ok) printf 'ok' ;;
        '') printf 'pending' ;;
        *)  printf 'fail' ;;
    esac
}

beamer_log() {
    local component=$1; shift
    printf 'beamer[%s]: %s\n' "$component" "$*" >&2
}

beamer_journal_slot() {
    BEAMER_JOURNAL_PATH=
    if [[ -r "$BEAMER_JOURNAL_SLOT" ]]; then
        read -r BEAMER_JOURNAL_PATH < "$BEAMER_JOURNAL_SLOT" || true
    fi
    if [[ -n "$BEAMER_JOURNAL_PATH" ]]; then
        return 0
    fi

    mkdir -p "$BEAMER_JOURNAL_DIR" 2>/dev/null || return 1

    local f num n=0
    for f in "$BEAMER_JOURNAL_DIR"/*.log.new; do
        if [[ -e $f ]]; then
            rm -f "$f" || true # orphaned by a power cut mid-dump
        fi
    done
    for f in "$BEAMER_JOURNAL_DIR"/[0-9]*.log; do
        if [[ -e $f ]]; then
            num=${f##*/}
            num=${num%%-*}
            if (( 10#${num:-0} > n )); then
                n=$(( 10#$num ))
            fi
        fi
    done

    local up
    read -r up _ < /proc/uptime 2>/dev/null || up=0
    printf -v BEAMER_JOURNAL_PATH '%s/%06d-%(%Y%m%dT%H%M%SZ)T.log' \
        "$BEAMER_JOURNAL_DIR" "$(( n + 1 ))" "$(( EPOCHSECONDS - ${up%.*} ))"

    printf '%s\n' "$BEAMER_JOURNAL_PATH" > "$BEAMER_JOURNAL_SLOT" || true
    return 0
}

beamer_journal_prune() {
    local dumps=() f i n
    for f in "$BEAMER_JOURNAL_DIR"/[0-9]*.log; do
        if [[ -e $f ]]; then
            dumps+=( "$f" )
        fi
    done

    n=${#dumps[@]}
    if (( n <= BEAMER_JOURNAL_KEEP )); then
        return 0
    fi
    for (( i = 0; i < n - BEAMER_JOURNAL_KEEP; i++ )); do
        rm -f "${dumps[i]}" || true
    done
    return 0
}

beamer_persist_journal() {
    if ! beamer_journal_slot || [[ -z "$BEAMER_JOURNAL_PATH" ]]; then
        return 0
    fi

    if journalctl --no-pager --no-hostname -o short-precise \
            > "$BEAMER_JOURNAL_PATH.new" 2>/dev/null; then
        mv -f "$BEAMER_JOURNAL_PATH.new" "$BEAMER_JOURNAL_PATH" || true
        sync || true
        beamer_journal_prune
    else
        rm -f "$BEAMER_JOURNAL_PATH.new" || true
    fi
    return 0
}

beamer_error() {
    local component=$1; shift
    local line first=1 target=$BEAMER_ERR_LOG
    if (( ${BEAMER_ERROR_LATE:-0} )); then
        target=$BEAMER_ERR_LATE
    fi
    beamer_dirs

    local head_line
    printf -v head_line '[%s] %s' "$component" "$1"
    if [[ -s "$target" ]] && grep -qxF -- "$head_line" "$target"; then
        printf 'beamer[%s]: ERROR (already recorded): %s\n' "$component" "$*" >&2
        beamer_led error
        return 0
    fi

    for line in "$@"; do
        if (( first )); then
            printf '[%s] %s\n' "$component" "$line" >> "$target"
            first=0
        else
            printf '%*s%s\n' $(( ${#component} + 3 )) '' "$line" >> "$target"
        fi
    done
    printf 'beamer[%s]: ERROR: %s\n' "$component" "$*" >&2

    beamer_led error
    beamer_persist_journal
}

beamer_rotate_errors() {
    beamer_dirs
    if [[ -s "$BEAMER_ERR_LATE" ]]; then
        mv -f "$BEAMER_ERR_LATE" "$BEAMER_ERR_PREV"
    else
        rm -f "$BEAMER_ERR_PREV"
    fi
    : > "$BEAMER_ERR_LOG"
    : > "$BEAMER_ERR_LATE"
}

# in previous session or during boot; live errors (session errors) dont trip this
beamer_have_errors() {
    [[ -s "$BEAMER_ERR_PREV" || -s "$BEAMER_ERR_LOG" ]]
}

beamer_mirror_errors() {
    [[ -e "$BEAMER_GADGET_IMG" ]] || return 0

    if ! beamer_have_errors; then
        mdel -i "$BEAMER_GADGET_FAT" ::/CONFIG/error.txt >/dev/null 2>&1 || true
        return 0
    fi

    beamer_dirs
    local tmp=$BEAMER_RUN/error.$$
    {
        printf 'Beamer errors, boot of %s\n' "$(beamer_boot_time)"
        printf 'station %s\n' "$(beamer_station_id)"
        printf '\n'
        if [[ -s "$BEAMER_ERR_PREV" ]]; then
            sed 's/^/[previous boot] /' "$BEAMER_ERR_PREV"
        fi
        if [[ -s "$BEAMER_ERR_LOG" ]]; then
            cat "$BEAMER_ERR_LOG"
        fi
        printf '\n'
        printf 'Fix CONFIG/config.txt on this drive, eject it, and move the cable back.\n'
        printf 'The card never needs reflashing.\n'
        printf '\n'
        printf 'If the LED on the Pi is SOLID, this station is working right now and\n'
        printf 'the above describes an earlier boot. This file is always one boot behind;\n'
        printf 'the LED is not. Trust the LED.\n'
    } | sed 's/$/\r/' > "$tmp"

    beamer_mcopy_config "$tmp" ::/CONFIG/error.txt
    rm -f "$tmp"
}

beamer_mcopy_config() {
    local src=$1 dest=$2
    if mcopy -o -i "$BEAMER_GADGET_FAT" "$src" "$dest" >/dev/null 2>&1; then
        return 0
    fi
    mmd -i "$BEAMER_GADGET_FAT" ::/CONFIG >/dev/null 2>&1 || true
    mcopy -o -i "$BEAMER_GADGET_FAT" "$src" "$dest" >/dev/null 2>&1 || true
}

beamer_hostname() {
    local h=
    [[ -r /etc/hostname ]] && read -r h < /etc/hostname
    printf '%s' "${h:-unknown}"
}

beamer_hostname_slug() {
    local s=${1,,}
    s=${s//[^a-z0-9]/-}
    while [[ $s == *--* ]]; do s=${s//--/-}; done
    s=${s#-}; s=${s%-}
    s=${s:0:56}
    s=${s%-}
    printf '%s' "$s"
}

beamer_apply_hostname() {
    local slug host cur=
    slug=$(beamer_hostname_slug "$(beamer_station_name)")
    if [[ -z "$slug" ]]; then
        slug=$(beamer_hostname_slug "$(beamer_station_id)")
        beamer_log hostname "station name has no characters a hostname can use; using the station ID"
    fi
    host="beamer-$slug"

    [[ -r /etc/hostname ]] && read -r cur < /etc/hostname
    if [[ "$cur" != "$host" ]]; then
        printf '%s\n' "$host" > /etc/hostname
        beamer_log hostname "hostname is now $host"
    fi

    [[ "$(hostname)" == "$host" ]] || hostname "$host"
    if systemctl is-active --quiet dbus.socket 2>/dev/null; then
        hostnamectl set-hostname "$host" >/dev/null 2>&1 || true
    fi

    if grep -q "^127.0.1.1[[:space:]]*$host\$" /etc/hosts 2>/dev/null; then
        return 0
    fi
    if grep -q '^127.0.1.1' /etc/hosts 2>/dev/null; then
        sed -i "s/^127.0.1.1.*/127.0.1.1\t$host/" /etc/hosts
    else
        printf '127.0.1.1\t%s\n' "$host" >> /etc/hosts
    fi
}

beamer_boot_time() {
    local up
    read -r up _ < /proc/uptime 2>/dev/null || up=0
    printf '%(%Y-%m-%d %H:%M:%S UTC)T' "$(( EPOCHSECONDS - ${up%.*} ))"
}

# --- JSON -----------------------------------------------------------------
# Enough JSON to write the two report files, and no more. Deliberately not a
# python one-liner: python startup on a Zero W costs about a second, and these
# run from the status check every 10 seconds and from the flush.

beamer_json_str() {
    local s=${1-}
    s=${s//\\/\\\\}
    s=${s//\"/\\\"}
    s=${s//$'\n'/\\n}
    s=${s//$'\r'/\\r}
    s=${s//$'\t'/\\t}
    printf '"%s"' "$s"
}

beamer_json_strn() {
    if [[ -z ${1-} ]]; then printf 'null'; else beamer_json_str "$1"; fi
}

beamer_json_num() {
    local v=${1-}
    if [[ "$v" =~ ^-?[0-9]+(\.[0-9]+)?$ ]]; then printf '%s' "$v"; else printf 'null'; fi
}

beamer_json_bool() {
    if (( ${1:-0} )); then printf 'true'; else printf 'false'; fi
}

beamer_iso_time() {
    printf '%(%Y-%m-%dT%H:%M:%SZ)T' "${1:--1}"
}

beamer_url_escape() {
    local s=${1-}
    s=${s//%/%25}
    s=${s//#/%23}
    s=${s//\?/%3F}
    s=${s// /%20}
    printf '%s' "$s"
}

# --- enumerating the replays ----------------------------------------------
beamer_slippi_count() {
    local cap=${BEAMER_COUNT_CAP:-255} listing line name n=0 newest= rc=0
    BEAMER_SLIPPI_FILES=
    BEAMER_SLIPPI_CAPPED=0
    BEAMER_SLIPPI_NEWEST=

    listing=$(mdir -b -i "$BEAMER_GADGET_FAT" ::/SLIPPI 2>/dev/null) || rc=$?
    if (( rc != 0 )); then
        return 0
    fi

    while IFS= read -r line; do
        name=${line##*/}
        if [[ -z $name || $name == .* || ${name,,} != *.slp ]]; then
            continue
        fi
        n=$(( n + 1 ))
        if [[ $line > $newest ]]; then
            newest=$line
        fi
    done <<< "$listing"

    BEAMER_SLIPPI_NEWEST=$newest
    if (( n > cap )); then
        BEAMER_SLIPPI_CAPPED=1
        n=$cap
    fi
    BEAMER_SLIPPI_FILES=$n
}

# --- reading a replay -----------------------------------------------------
beamer_peek_slp() {
    local path=$1 out rc=0
    BEAMER_GAME_STATE=
    BEAMER_GAME_INFO=

    [[ -x "$BEAMER_SLP_PEEK" ]] || return 1

    out=$(
        set +o pipefail
        mtype -i "$BEAMER_GADGET_FAT" "$path" 2>/dev/null | "$BEAMER_SLP_PEEK" -
        exit "${PIPESTATUS[1]}"
    ) || rc=$?

    (( rc == 0 )) || return 1
    [[ -n "$out" ]] || return 1

    BEAMER_GAME_INFO=$out
    if [[ "$out" == '{"live":true,'* ]]; then
        BEAMER_GAME_STATE=live
    else
        BEAMER_GAME_STATE=done
    fi
}

# --- the /status report ---------------------------------------------------
beamer_write_report() {
    local tmp=$BEAMER_REPORT_JSON.$$ game net result

    game=null
    if [[ -s "$BEAMER_GAME_JSON" ]]; then
        read -r game < "$BEAMER_GAME_JSON" 2>/dev/null || game=null
        [[ -n "$game" ]] || game=null
    fi

    net=$(beamer_net_result)
    if beamer_session_has_errors; then
        result=fail
    elif [[ "$net" == ok ]]; then
        result=pass
    elif [[ "$net" == pending ]]; then
        result=pending
    else
        result=fail
    fi

    {
        printf '{\n'
        printf '  "schema": 1,\n'
        printf '  "beamer_arch": %s,\n' "$(beamer_json_str "$(beamer_arch)")"
        printf '  "generated": %s,\n' "$(beamer_json_str "$(beamer_iso_time)")"
        if [[ -s "$BEAMER_STATUS_FRAG" ]]; then cat "$BEAMER_STATUS_FRAG"; fi
        if [[ -s "$BEAMER_HEALTH_FRAG" ]]; then cat "$BEAMER_HEALTH_FRAG"; fi
        printf '  "game": %s,\n'   "$game"
        printf '  "result": %s,\n' "$(beamer_json_str "$result")"
        printf '  "errors": %s\n'  "$(beamer_json_errors)"
        printf '}\n'
    } > "$tmp" || {
        rm -f "$tmp"
        beamer_log report "could not write $BEAMER_REPORT_JSON"
        return 0
    }

    chmod 0644 "$tmp"
    mv -f "$tmp" "$BEAMER_REPORT_JSON"

    case "$result" in
        pass)    beamer_led ok ;;
        pending) beamer_led boot ;;
        *)       beamer_led error ;;
    esac
}

beamer_json_errors() {
    local f line first=1
    printf '['
    for f in "$BEAMER_ERR_LOG" "$BEAMER_ERR_LATE"; do
        [[ -s "$f" ]] || continue
        while IFS= read -r line || [[ -n "$line" ]]; do
            (( first )) || printf ', '
            first=0
            beamer_json_str "$line"
        done < "$f"
    done
    printf ']'
}

beamer_snapshot_status() {
    local net=${1:-} addr
    [[ -n "$net" ]] || net=$(cat "$BEAMER_NET_STATUS" 2>/dev/null || true)
    addr=$(ip -4 -o addr show dev "$BEAMER_IFACE" scope global 2>/dev/null \
           | awk '{print $4; exit}' || true)
    beamer_dirs
    {
        printf 'net=%s\n' "$net"
        printf 'ip=%s\n' "${addr%%/*}"
    } > "$BEAMER_STATUS_LATE"
}

beamer_rotate_status() {
    beamer_dirs
    if [[ -s "$BEAMER_STATUS_LATE" ]]; then
        mv -f "$BEAMER_STATUS_LATE" "$BEAMER_STATUS_PREV"
    else
        rm -f "$BEAMER_STATUS_PREV"
    fi
}

beamer_read_status() {
    local f=$1 line
    BEAMER_LAST_NET=
    BEAMER_LAST_IP=
    [[ -r "$f" ]] || return 0
    while IFS= read -r line || [[ -n "$line" ]]; do
        case $line in
            net=*) BEAMER_LAST_NET=${line#net=} ;;
            ip=*)  BEAMER_LAST_IP=${line#ip=} ;;
        esac
    done < "$f"
}

beamer_write_status() {
    local outcome=${1:-} net=${2:-} ip=${3:-} suffix=${4:-} tmp

    [[ -e "$BEAMER_GADGET_IMG" ]] || return 0

    local station=unknown host=unknown name= up
    [[ -r "$BEAMER_STATION_ID" ]] && read -r station < "$BEAMER_STATION_ID"
    [[ -r "$BEAMER_STATION_NAME_FILE" ]] && read -r name < "$BEAMER_STATION_NAME_FILE"
    [[ -r /etc/hostname ]] && read -r host < /etc/hostname
    read -r up _ < /proc/uptime 2>/dev/null || up=0

    beamer_dirs
    tmp=$BEAMER_RUN/status.$$
    printf 'Beamer station status\r\nname    %s\r\nstation %s\r\nhost    %s\r\nboot    %(%Y-%m-%d %H:%M:%S UTC)T\r\n\r\nwifi:    %s\r\nnetwork: %s%s\r\nip:      %s%s\r\n' \
        "${name:-$station}" "$station" "$host" "$(( EPOCHSECONDS - ${up%.*} ))" \
        "${outcome:-unknown}" \
        "${net:-unknown}" "${net:+$suffix}" \
        "${ip:-unknown}" "${ip:+$suffix}" > "$tmp"

    beamer_mcopy_config "$tmp" ::/CONFIG/status.txt
    rm -f "$tmp"
}

beamer_purge_foreign_wifi() {
    local dir=/etc/NetworkManager/system-connections
    local f base id
    [[ -d "$dir" ]] || return 0

    for f in "$dir"/*.nmconnection; do
        [[ -e "$f" ]] || continue
        base=${f##*/}
        if [[ "$base" == "beamer-wifi.nmconnection" ]]; then
            continue
        fi
        grep -qE '^[[:space:]]*type=(wifi|802-11-wireless)[[:space:]]*$' "$f" || continue

        if systemctl is-active --quiet NetworkManager 2>/dev/null; then
            id=$(sed -n 's/^[[:space:]]*id=//p' "$f" | head -n1)
            if [[ -n "$id" ]]; then
                nmcli connection delete id "$id" >/dev/null 2>&1 || true
            fi
        fi
        rm -f "$f"
        beamer_log purge "removed foreign WiFi profile $base"
    done
}

beamer_clear_wifi() {
    local profile=/etc/NetworkManager/system-connections/beamer-wifi.nmconnection

    if [[ -e "$profile" ]]; then
        if systemctl is-active --quiet NetworkManager 2>/dev/null; then
            nmcli connection delete id beamer-wifi >/dev/null 2>&1 || true
        fi
        rm -f "$profile"
        beamer_log clear-wifi "removed the beamer-wifi profile"
    fi

    rm -f "$BEAMER_EXPECT_SSID"
    rm -f "$BEAMER_WIFI_HASH"
}
