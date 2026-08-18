#!/bin/bash
# The station's control endpoints, run by lighttpd as www-data:
#
#   GET  /status         the last status report, straight off disk
#   POST /status         re-run the status check, then return the fresh report
#   POST /reset-beamer   wipe the replay drive (needs X-Beamer-Confirm: reset)
#
# GET /SLIPPI/ is not here: that is a static index.json written by the flush, so
# no CGI runs for the path a downloader hits most.
#
# Deliberately NOT set -e: a CGI that dies partway through has already sent
# headers, and the client gets a truncated response with no explanation. Every
# path here ends in a complete reply instead.
set -uo pipefail

export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

source /usr/local/lib/beamer/beamer-common.sh

STATUS_UNIT=status-check.service
RESET_UNIT=beamer-reset.service
LOCK=/run/lock/beamer-api.lock
STATUS_TIMEOUT=${STATUS_TIMEOUT:-25}
RESET_TIMEOUT=${RESET_TIMEOUT:-60}

# --- replying -------------------------------------------------------------
respond() {
    local status=$1 body=$2 header
    shift 2
    printf 'Status: %s\r\n' "$status"
    printf 'Content-Type: application/json\r\n'
    printf 'Cache-Control: no-store\r\n'
    for header in "$@"; do
        printf '%s\r\n' "$header"
    done
    printf '\r\n'
    printf '%s\n' "$body"
}

fail() {
    local status=$1 message=$2
    shift 2
    respond "$status" "{\"ok\": false, \"error\": $(beamer_json_str "$message")}" "$@"
    exit 0
}

serve_report() {
    if [[ -r "$BEAMER_REPORT_JSON" ]]; then
        respond 200 "$(cat "$BEAMER_REPORT_JSON")"
    else
        fail 503 "no status check has run yet on this station"
    fi
    exit 0
}

# --- the privileged actions -----------------------------------------------
with_lock() {
    exec 9>"$LOCK" || fail 500 "could not open $LOCK"
    flock -n 9 || fail 409 "another beamer action is already running"
    "$@"
}

start_unit() {
    timeout "$2" systemctl --no-ask-password restart "$1" 2>&1 >/dev/null
}

report_stamp() {
    stat -c %y "$BEAMER_REPORT_JSON" 2>/dev/null || printf 'none'
}

do_status_check() {
    local err rc=0 before after
    before=$(report_stamp)
    err=$(start_unit "$STATUS_UNIT" "$STATUS_TIMEOUT") || rc=$?
    after=$(report_stamp)

    if (( rc == 124 )); then
        fail 504 "status check did not finish within ${STATUS_TIMEOUT}s"
    fi

    if [[ "$after" == "$before" ]]; then
        fail 500 "status check did not run: ${err:-exit $rc}"
    fi
    serve_report
}

do_reset() {
    local err rc=0
    err=$(start_unit "$RESET_UNIT" "$RESET_TIMEOUT") || rc=$?

    if (( rc == 124 )); then
        fail 504 "reset did not finish within ${RESET_TIMEOUT}s"
    elif (( rc )); then
        fail 500 "reset failed; see 'journalctl -u $RESET_UNIT' on the station"
    fi
    respond 200 '{"ok": true, "message": "reset OK"}'
    exit 0
}

require_confirm() {
    if [[ "${HTTP_X_BEAMER_CONFIRM-}" != "reset" ]]; then
        fail 400 "POST /reset-beamer needs the header 'X-Beamer-Confirm: reset'. It erases every replay on this station."
    fi
}

# --- dispatch -------------------------------------------------------------
URL=${SCRIPT_NAME:-}
if [[ -z "$URL" ]]; then
    URL=${REQUEST_URI-}
    URL=${URL%%\?*}
fi
URL=${URL%/}
METHOD=${REQUEST_METHOD:-GET}

case "$URL:$METHOD" in
    /status:GET|/status:HEAD) serve_report ;;
    /status:POST)             with_lock do_status_check ;;
    /reset-beamer:POST)       require_confirm; with_lock do_reset ;;
    /status:*)                fail 405 "use GET or POST on /status" "Allow: GET, HEAD, POST" ;;
    /reset-beamer:*)          fail 405 "use POST on /reset-beamer" "Allow: POST" ;;
    *)                        fail 404 "no such endpoint" ;;
esac
