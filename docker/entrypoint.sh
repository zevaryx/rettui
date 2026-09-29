#!/bin/sh
# Start rettui's web UI as the user and group given by PUID and PGID (GUID is
# accepted for PGID), so files in the mounted data directory belong to that
# user on the host.
set -eu

PUID="${PUID:-1000}"
PGID="${PGID:-${GUID:-1000}}"
PORT="${PORT:-8740}"
DATA=/data

for value in "$PUID" "$PGID" "$PORT"; do
    case "$value" in
        '' | *[!0-9]*)
            echo "rettui: PUID, PGID and PORT must be numbers (got '$value')" >&2
            exit 1
            ;;
    esac
done

mkdir -p "$DATA/reticulum"
# First start: keep the Reticulum config in the data directory too. To use
# another, change rns_config in settings.json while rettui is stopped (the web
# UI can't: a config's pipe interfaces run commands).
if [ ! -e "$DATA/settings.json" ]; then
    printf '{\n  "rns_config": "%s/reticulum"\n}\n' "$DATA" > "$DATA/settings.json"
fi

set -- /usr/local/bin/rettui --data-dir "$DATA" --web "0.0.0.0:$PORT" "$@"

if [ "$(id -u)" = 0 ]; then
    # Hand the data directory to that user, touching only what differs.
    find "$DATA" \( ! -user "$PUID" -o ! -group "$PGID" \) -exec chown -h "$PUID:$PGID" {} +
    exec setpriv --reuid="$PUID" --regid="$PGID" --clear-groups -- "$@"
fi

# Already started as a non-root user (docker run --user): run as that user.
exec "$@"
