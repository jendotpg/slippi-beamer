#!/bin/bash
set -uo pipefail

BEAMER_ERROR_LATE=1

source /usr/local/lib/beamer/beamer-common.sh

IFACE=${IFACE:-$BEAMER_IFACE}
BEAMER_IFACE=$IFACE 
ASSOC_TIMEOUT=${ASSOC_TIMEOUT:-45}
DHCP_TIMEOUT=${DHCP_TIMEOUT:-30}

record() {
    printf '%s\n' "$2" > "$BEAMER_NET_STATUS"
    printf '%s\n' "$1" > "$BEAMER_NET_RESULT"
    beamer_snapshot_status "$2"
}

rm -f "$BEAMER_NET_RESULT"

[[ -s $BEAMER_EXPECT_SSID ]] || exit 0
EXPECT=$(head -n1 "$BEAMER_EXPECT_SSID")
[[ -n $EXPECT ]] || exit 0

# --- 1. associated, with the right AP? ------------------------------------
ssid=
deadline=$(( SECONDS + ASSOC_TIMEOUT ))
while :; do
    ssid=$(iw dev "$IFACE" link 2>/dev/null | sed -n 's/^[[:space:]]*SSID:[[:space:]]*//p' | head -n1)
    [[ "$ssid" == "$EXPECT" ]] && break
    (( SECONDS >= deadline )) && break
    sleep 3
done

if [[ "$ssid" != "$EXPECT" ]]; then
    if [[ -n $ssid ]]; then
        beamer_error check-net "Joined \"$ssid\", but CONFIG/config.txt asks for \"$EXPECT\"."
    else
        beamer_error check-net "Did not associate with SSID \"$EXPECT\"." \
                               "Usually a wrong password, or the network is out of range." \
                               "Check SSID and PASSWORD in CONFIG/config.txt."
    fi
    record fail "NOT associated with \"$EXPECT\""
    exit 0
fi

# --- 2. got an address? ---------------------------------------------------
addr=
deadline=$(( SECONDS + DHCP_TIMEOUT ))
while :; do
    addr=$(ip -4 -o addr show dev "$IFACE" scope global 2>/dev/null | awk '{print $4; exit}')
    [[ -n $addr ]] && break
    (( SECONDS >= deadline )) && break
    sleep 3
done

if [[ -z $addr ]]; then
    beamer_error check-net "Associated with \"$EXPECT\" but got no IPv4 address." \
                           "The password is fine; the network is not handing out addresses."
    record fail "associated with \"$EXPECT\", no IPv4 address"
    exit 0
fi

# --- 3. gateway ------------------------------------------------------------
gw=$(ip -4 route show default dev "$IFACE" 2>/dev/null | awk '/^default/ {print $3; exit}')
if [[ -n $gw ]]; then
    if ping -c2 -W2 "$gw" >/dev/null 2>&1; then
        beamer_log check-net "gateway $gw responds"
    else
        beamer_log check-net "gateway $gw did not answer ping (not treated as a failure)"
    fi
fi

record ok "associated with \"$EXPECT\""
beamer_log check-net "associated with \"$EXPECT\", ip ${addr%%/*}"
exit 0
