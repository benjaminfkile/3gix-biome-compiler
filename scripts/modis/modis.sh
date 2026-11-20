#!/bin/sh
# The whole MODIS composite: list, then a composite of every tile not yet
# done, JOBS tiles at a time (default 3). Resumable: run it again after a
# stop. The Earthdata login comes from ~/.netrc only.
#
#   sh scripts/modis/modis.sh WORK OUT [JOBS]
set -eu

if [ $# -lt 2 ]; then
    echo "usage: modis.sh WORK OUT [JOBS]" >&2
    exit 2
fi
work=$1
out=$2
jobs=${3:-3}
here=$(cd "$(dirname "$0")" && pwd)
py=${PYTHON:-python3}

mkdir -p "$work" "$out"
"$py" "$here/modis.py" list --year 2023 --year 2024 --every 8 --out "$work/tiles.json"

tiles=$("$py" -c 'import json,sys; print("\n".join(sorted(json.load(open(sys.argv[1]))["tiles"])))' "$work/tiles.json")
total=$(printf '%s\n' "$tiles" | grep -c . || true)

bytes() {
    cat "$work"/bytes/* 2>/dev/null | awk '{s += $1} END {printf "%d", s}'
}

progress() {
    done_n=$(ls "$out" 2>/dev/null | grep -c '\.bgc$' || true)
    echo "modis: $done_n/$total tiles done, $(bytes) bytes downloaded"
}

progress
printf '%s\n' "$tiles" | grep . | while read -r t; do
    if [ ! -f "$out/$t.bgc" ]; then
        echo "$t"
    fi
done | xargs -P "$jobs" -I {} sh -c '
    "$1" "$2/modis.py" composite --tiles "$3/tiles.json" --tile "$4" --work "$3" --out "$5" \
        > "$3/$4.log" 2>&1 || { echo "modis: $4 failed, see $3/$4.log" >&2; exit 0; }
    echo "modis: $4 done"
' sh "$py" "$here" "$work" {} "$out"
progress
