#!/bin/bash
# Deliberately deprioritised in health-check.service (SCHED_IDLE, idle I/O).
# Nothing here is urgent, and it must never take cycles from status-check or
# from the gadget path. Writes only to memory, never disk.
set -euo pipefail

BEAMER_ERROR_LATE=1

source /usr/local/lib/beamer/beamer-common.sh

PROBE_BUDGET=${PROBE_BUDGET:-15}

F_STATION=  F_STATION_NAME=  F_HOST=     F_BOOT=    F_UPTIME=
F_WIFI=     F_NETWORK=  F_IP=
F_HTTP=0    F_SSH=0     F_MDNS=0

beamer_dirs

fmt_uptime() {
    local s=${1:-0} d h m
    d=$(( s / 86400 )); h=$(( s % 86400 / 3600 )); m=$(( s % 3600 / 60 ))
    if   (( d )); then printf '%dd %dh %dm' "$d" "$h" "$m"
    elif (( h )); then printf '%dh %dm' "$h" "$m"
    else               printf '%dm' "$m"
    fi
}

write_fragment() {
    local tmp=$BEAMER_HEALTH_FRAG.$$
    {
        printf '  "station": %s,\n'  "$(beamer_json_strn "$F_STATION")"
        printf '  "station_name": %s,\n' "$(beamer_json_strn "$F_STATION_NAME")"
        printf '  "host": %s,\n'     "$(beamer_json_strn "$F_HOST")"
        printf '  "boot": %s,\n'     "$(beamer_json_strn "$F_BOOT")"
        printf '  "uptime_s": %s,\n' "$(beamer_json_num  "$F_UPTIME")"
        printf '  "wifi": %s,\n'     "$(beamer_json_strn "$F_WIFI")"
        printf '  "network": %s,\n'  "$(beamer_json_strn "$F_NETWORK")"
        printf '  "ip": %s,\n'       "$(beamer_json_strn "$F_IP")"
        printf '  "httpd": %s,\n'    "$(beamer_json_bool "$F_HTTP")"
        printf '  "sshd": %s,\n'     "$(beamer_json_bool "$F_SSH")"
        printf '  "mdns": %s,\n'     "$(beamer_json_bool "$F_MDNS")"
    } > "$tmp"
    chmod 0644 "$tmp"
    mv -f "$tmp" "$BEAMER_HEALTH_FRAG"
}

emit_text() {
    if [[ -n "$F_STATION_NAME" && "$F_STATION_NAME" != "$F_STATION" ]]; then
        echo "name:    $F_STATION_NAME"
    fi
    if [[ -n "$F_STATION" ]]; then
        echo "station: $F_STATION"
    fi
    if [[ -n "$F_WIFI" ]]; then
        echo "wifi:    \"$F_WIFI\" (from CONFIG/config.txt)"
        echo "network: ${F_NETWORK:-not checked yet}"
        echo "ip:      ${F_IP:-none}"
    else
        echo "wifi:    no config.txt applied"
    fi
    echo "uptime:  $(fmt_uptime "$F_UPTIME")"
    if (( F_HTTP )); then
        echo "lighttpd OK"
    fi
    if (( F_SSH )); then
        echo "sshd OK"
    fi
    if (( F_MDNS )); then
        echo "mdns OK"
    fi
    if [[ -r "$BEAMER_LIVE_GAME" ]]; then
        local live=
        read -r live < "$BEAMER_LIVE_GAME" 2>/dev/null || true
        if [[ -n "$live" ]]; then
            echo "game:    in progress ($live)"
        else
            echo "game:    none in progress"
        fi
    fi
}

publish() {
    write_fragment
    beamer_write_report
}

# --- identity and network -------------------------------------------------
if [[ -s "$BEAMER_STATION_ID" ]]; then
    read -r F_STATION < "$BEAMER_STATION_ID"
fi
F_STATION_NAME=$(beamer_station_name)
F_HOST=$(beamer_hostname)
read -r F_UPTIME _ < /proc/uptime
F_UPTIME=${F_UPTIME%.*}
F_BOOT=$(beamer_iso_time "$(( EPOCHSECONDS - F_UPTIME ))")

if [[ -s "$BEAMER_EXPECT_SSID" ]]; then
    read -r F_WIFI < "$BEAMER_EXPECT_SSID" || true
    read -r F_NETWORK < "$BEAMER_NET_STATUS" 2>/dev/null || true
    beamer_read_status "$BEAMER_STATUS_LATE"
    F_IP=${BEAMER_LAST_IP:-}
fi

# --- lighttpd, sshd and avahi ---------------------------------------------
probe_http() {
    local line
    line=$(timeout 3 bash -c 'exec 3<>/dev/tcp/127.0.0.1/80 || exit 1
                              printf "HEAD / HTTP/1.0\r\n\r\n" >&3
                              head -n1 <&3' 2>/dev/null) || return 1
    [[ "$line" == HTTP/1.* ]]
}

probe_ssh() {
    local line
    line=$(timeout 3 bash -c 'exec 3<>/dev/tcp/127.0.0.1/22 || exit 1
                              head -n1 <&3' 2>/dev/null) || return 1
    [[ "$line" == SSH-* ]]
}

probe_mdns() {
    [[ -S /run/avahi-daemon/socket ]] || return 1
    systemctl is-active --quiet avahi-daemon.service
}

DEADLINE=$(( SECONDS + PROBE_BUDGET ))

while :; do
    if (( ! F_HTTP )) && probe_http; then F_HTTP=1; fi
    if (( ! F_SSH ))  && probe_ssh;  then F_SSH=1;  fi
    if (( ! F_MDNS )) && probe_mdns; then F_MDNS=1; fi
    if (( F_HTTP && F_SSH && F_MDNS )); then break; fi
    if (( SECONDS >= DEADLINE )); then break; fi
    sleep 1
done

if (( ! F_HTTP )); then
    LIGHTTPD_STATE=$(systemctl is-active lighttpd.service 2>/dev/null || true)
    beamer_error health-check \
        "Nothing answered HTTP on 127.0.0.1:80 within ${PROBE_BUDGET}s." \
        "lighttpd.service is ${LIGHTTPD_STATE:-unknown}. Replays are pulled over" \
        "HTTP, so this station is collecting files it cannot hand out." \
        "Check 'journalctl -u lighttpd.service'."
fi

if (( ! F_SSH )); then
    SSHD_STATE=$(systemctl is-active ssh.service 2>/dev/null || true)
    beamer_error health-check \
        "sshd did not answer on 127.0.0.1:22 within ${PROBE_BUDGET}s." \
        "ssh.service is ${SSHD_STATE:-unknown}. The station may still be serving" \
        "replays, but there is no way to get into it to find out." \
        "Check 'journalctl -u ssh.service -u regenerate_ssh_host_keys.service'."
fi

if (( ! F_MDNS )); then
    MDNS_STATE=$(systemctl is-active avahi-daemon.service 2>/dev/null || true)
    beamer_error health-check \
        "avahi-daemon.service is ${MDNS_STATE:-unknown}; this station will not" \
        "appear in a discovery browse. Replays are unaffected." \
        "Check 'journalctl -u avahi-daemon.service'."
fi

# --- the verdict ----------------------------------------------------------
# beamer_write_report owns the pass/fail decision and the LED, because it needs
# both this script's network result and status-check's errors.
NET=$(beamer_net_result)

publish
emit_text

if beamer_session_has_errors; then
    echo "health-check: errors were recorded this boot - see CONFIG/error.txt" >&2
elif [[ "$NET" == ok ]]; then
    echo "health check passed"
elif [[ "$NET" == pending ]]; then
    echo "health-check: still waiting on check-net; no verdict yet" >&2
else
    echo "health-check: the station did not join its network - see CONFIG/error.txt" >&2
fi
