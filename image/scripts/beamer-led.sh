#!/bin/bash
# Drive the Pi's single ACT LED as a live readout of station health.
#
#   beamer-led.sh boot     slow blink  - powered, health not determined yet
#   beamer-led.sh ok       solid       - gadget bound AND network confirmed
#   beamer-led.sh error    fast blink  - something is wrong this session, or
#                                        nothing has yet shown the network works
#   beamer-led.sh off
set -uo pipefail

BOOT_MS=500
ERROR_MS=100

# Not under /run/beamer: this script does not source beamer-common.sh.
STATE=/run/led-state

usage() { echo "usage: ${0##*/} boot|ok|error|off" >&2; exit 2; }

find_led() {
    local d
    if [[ -n "${BEAMER_LED_DIR:-}" ]]; then
        printf '%s' "$BEAMER_LED_DIR"
        return 0
    fi
    for d in /sys/class/leds/ACT /sys/class/leds/led0; do
        if [[ -d "$d" ]]; then
            printf '%s' "$d"
            return 0
        fi
    done
    return 1
}

set_trigger() { printf '%s' "$1" > "$LED/trigger" 2>/dev/null || true; }

blink() {
    set_trigger timer
    printf '%s' "$1" > "$LED/delay_on"  2>/dev/null || true
    printf '%s' "$1" > "$LED/delay_off" 2>/dev/null || true
}

solid() {
    local max=1
    set_trigger none
    if [[ -r "$LED/max_brightness" ]]; then
        max=$(cat "$LED/max_brightness" 2>/dev/null || echo 1)
    fi
    printf '%s' "$max" > "$LED/brightness" 2>/dev/null || true
}

dark() {
    set_trigger none
    printf '0' > "$LED/brightness" 2>/dev/null || true
}

[[ $# -eq 1 ]] || usage
case "$1" in
    boot|ok|error|off) ;;
    *) usage ;;
esac

WANT=$1
CURRENT=
read -r CURRENT < "$STATE" 2>/dev/null || true
if [[ "$CURRENT" == "$WANT" ]]; then
    exit 0
fi

LED=$(find_led) || exit 0

case "$WANT" in
    boot)  blink "$BOOT_MS" ;;
    error) blink "$ERROR_MS" ;;
    ok)    solid ;;
    off)   dark ;;
esac

printf '%s' "$WANT" > "$STATE" 2>/dev/null || true

exit 0
