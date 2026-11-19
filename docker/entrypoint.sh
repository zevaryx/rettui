#!/bin/sh
# Start rettui's web UI (or, given `node`, a headless node) as the user and
# group given by PUID and PGID (GUID is accepted for PGID), so files in the
# mounted data directory belong to that user on the host.
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

# STATS_LISTEN (optional) is an address like 0.0.0.0:8741 for the read-only
# stats page (see wiki/Docker.md). Only characters an address has are allowed.
STATS_LISTEN="${STATS_LISTEN:-}"
case "$STATS_LISTEN" in
    *[!0-9a-fA-F.:\[\]]*)
        echo "rettui: STATS_LISTEN must be an address like 0.0.0.0:8741 (got '$STATS_LISTEN')" >&2
        exit 1
        ;;
esac

mkdir -p "$DATA/reticulum"
# First start: keep the Reticulum config in the data directory too. To use
# another, change rns_config in settings.json while rettui is stopped (the web
# UI can't: a config's pipe interfaces run commands).
if [ ! -e "$DATA/settings.json" ]; then
    printf '{\n  "rns_config": "%s/reticulum"\n}\n' "$DATA" > "$DATA/settings.json"
fi

# `node` runs a headless public node (`rettui node`) instead of the web UI;
# anything else is passed on to rettui after the web UI's flags.
#
# 0.0.0.0 is the container's own address: rettui refuses plain HTTP there
# unless told it is accepted, so the web UI always gets --insecure-http: this
# image accepts plain HTTP by itself. That is safe only because compose.yaml
# publishes the port on 127.0.0.1. Whoever publishes it beyond that
# (`docker run -p 8740:8740`, a Compose file of their own, `network_mode:
# host`) must add --https (for example `command: ["--https"]` in compose.yaml).
if [ "${1:-}" = node ]; then
    shift
    # PN_NAME and NO_PROPAGATION (optional, only for `node`) set the
    # propagation node's name and turn it off (see wiki/Docker.md). Empty
    # counts as unset. rettui limits the name's length itself.
    PN_NAME="${PN_NAME:-}"
    NO_PROPAGATION="${NO_PROPAGATION:-}"
    case "$PN_NAME" in
        *[[:cntrl:]]*)
            echo "rettui: PN_NAME must not contain control characters or line breaks" >&2
            exit 1
            ;;
    esac
    case "$NO_PROPAGATION" in
        '' | [Nn][Oo] | [Ff][Aa][Ll][Ss][Ee] | 0) NO_PROPAGATION= ;;
        [Yy][Ee][Ss] | [Tt][Rr][Uu][Ee] | 1) NO_PROPAGATION=yes ;;
        *)
            echo "rettui: NO_PROPAGATION must be yes, true or 1 (or no, false or 0, for off) (got '$NO_PROPAGATION')" >&2
            exit 1
            ;;
    esac
    if [ -n "$NO_PROPAGATION" ] && [ -n "$PN_NAME" ]; then
        echo "rettui: PN_NAME and NO_PROPAGATION conflict: a node with no propagation node has no name to give it" >&2
        exit 1
    fi
    # --name=VALUE is one argument whatever the value holds, even a leading dash.
    if [ -n "$PN_NAME" ]; then
        set -- "--name=$PN_NAME" "$@"
    fi
    if [ -n "$NO_PROPAGATION" ]; then
        set -- --no-propagation "$@"
    fi
    if [ -n "$STATS_LISTEN" ]; then
        set -- --stats-listen "$STATS_LISTEN" "$@"
    fi
    set -- /usr/local/bin/rettui --data-dir "$DATA" node "$@"
else
    if [ -n "$STATS_LISTEN" ]; then
        set -- --stats-listen "$STATS_LISTEN" "$@"
    fi
    set -- /usr/local/bin/rettui --data-dir "$DATA" --web "0.0.0.0:$PORT" --insecure-http "$@"
fi

if [ "$(id -u)" = 0 ]; then
    # Hand the data directory to that user, touching only what differs.
    find "$DATA" \( ! -user "$PUID" -o ! -group "$PGID" \) -exec chown -h "$PUID:$PGID" {} +
    exec setpriv --reuid="$PUID" --regid="$PGID" --clear-groups -- "$@"
fi

# Already started as a non-root user (docker run --user): run as that user.
exec "$@"
