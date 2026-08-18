#!/bin/bash
# A replay is only published once it has stopped being written.
# A file already in the web root is never touched again.
set -euo pipefail

source /usr/local/lib/beamer/beamer-common.sh

IMG=$BEAMER_GADGET_FAT
DEST=$BEAMER_WEB_SLIPPI
KEEP=${KEEP:-}
if [[ -z $KEEP && -r $BEAMER_KEEP_FILE ]]; then
    read -r KEEP < "$BEAMER_KEEP_FILE" || true
fi
[[ $KEEP =~ ^[0-9]+$ ]] && (( KEEP >= 1 )) || KEEP=$BEAMER_KEEP_DEFAULT

write_index() {
    local published=("$@")
    local tmp=$DEST/.index.json.$$
    local line size mtime path name first=1
    local -a stats=()

    if (( ${#published[@]} )); then
        mapfile -t stats < <(stat -c '%s %Y %n' -- "${published[@]}" 2>/dev/null || true)
    fi

    {
        printf '{\n'
        printf '  "schema": 1,\n'
        printf '  "station": %s,\n'   "$(beamer_json_str "$(beamer_station_id)")"
        printf '  "generated": %s,\n' "$(beamer_json_str "$(beamer_iso_time)")"
        printf '  "count": %s,\n'     "${#stats[@]}"
        printf '  "files": ['
        for line in "${stats[@]}"; do
            size=${line%% *}
            mtime=${line#* }; mtime=${mtime%% *}
            path=${line#* }; path=${path#* }
            name=${path##*/}
            (( first )) || printf ','
            first=0
            printf '\n    {"name": %s, "size": %s, "mtime": %s, "url": %s}' \
                "$(beamer_json_str "$name")" \
                "$(beamer_json_num "$size")" \
                "$(beamer_json_str "$(beamer_iso_time "$mtime")")" \
                "$(beamer_json_str "/SLIPPI/$(beamer_url_escape "$name")")"
        done
        (( first )) || printf '\n  '
        printf ']\n'
        printf '}\n'
    } > "$tmp"

    chmod 0644 "$tmp"
    mv -f "$tmp" "$BEAMER_WEB_INDEX"
}

if ! LISTING=$(mdir -b -i "$IMG" ::/SLIPPI 2>/dev/null); then
    minfo -i "$IMG" :: >/dev/null 2>&1 || exit 0
    LISTING=
fi

NEWEST=()
if [[ -n $LISTING ]]; then
    mapfile -t NEWEST < <(printf '%s\n' "$LISTING" \
        | grep -Ei '/[^/.][^/]*\.slp$' | sort | tail -n "$KEEP")
fi

beamer_dirs
mkdir -p "$DEST"

CHANGED=0

declare -A KEEPING=()
for f in "${NEWEST[@]}"; do KEEPING[${f##*/}]=1; done
for f in "$DEST"/*; do
    [[ -f $f ]] || continue
    if [[ $f == "$BEAMER_WEB_INDEX" ]]; then continue; fi
    if [[ -z ${KEEPING[${f##*/}]:-} ]]; then
        rm -f -- "$f"
        CHANGED=1
    fi
done

TOCOPY=()
WITHHELD=()
for f in "${NEWEST[@]}"; do
    if [[ -e "$DEST/${f##*/}" ]]; then continue; fi
    if beamer_peek_slp "$f" && [[ $BEAMER_GAME_STATE == done ]]; then
        TOCOPY+=("$f")
    else
        WITHHELD+=("${f##*/}")
    fi
done

WITHHELD_NOW=${WITHHELD[*]:-}
WITHHELD_WAS=
[[ -r $BEAMER_WITHHELD ]] && read -r WITHHELD_WAS < "$BEAMER_WITHHELD" || true
if [[ $WITHHELD_NOW != "$WITHHELD_WAS" ]]; then
    if (( ${#WITHHELD[@]} )); then
        beamer_log flush "not publishing ${#WITHHELD[@]} unfinished replay(s): ${WITHHELD[*]}"
    fi
    printf '%s\n' "$WITHHELD_NOW" > "$BEAMER_WITHHELD"
fi

if (( ${#TOCOPY[@]} )); then
    beamer_log flush "publishing ${#TOCOPY[@]} finished replay(s): ${TOCOPY[*]}"
    mcopy -n -i "$IMG" "${TOCOPY[@]}" "$DEST/"
    CHANGED=1
fi

PUBLISHED=()
for f in "${NEWEST[@]}"; do
    [[ -f "$DEST/${f##*/}" ]] && PUBLISHED+=("$DEST/${f##*/}")
done

if (( CHANGED )) || [[ ! -s "$BEAMER_WEB_INDEX" ]]; then
    write_index "${PUBLISHED[@]}"
fi
