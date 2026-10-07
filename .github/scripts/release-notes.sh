#!/usr/bin/env bash
# Print a release's notes from CHANGELOG.md: the section headed with its
# tag (`## v1.7.0`), without the heading, up to the next release's. A
# prerelease (`v1.7.0-rc.1`) takes its own section if there is one, else
# the release's. No section: an error, so a release isn't made without one.
#
#   .github/scripts/release-notes.sh v1.7.0 [CHANGELOG.md]
set -euo pipefail
tag="${1:?usage: release-notes.sh <tag> [changelog]}"
changelog="${2:-CHANGELOG.md}"

section() {
  awk -v heading="## $1" '
    $0 == heading { found = 1; next }
    found && /^## / { exit }
    found { print }
  ' "$changelog"
}

notes="$(section "$tag")"
if [ -z "$(printf '%s' "$notes" | tr -d '[:space:]')" ] && [[ "$tag" == *-* ]]; then
  notes="$(section "${tag%%-*}")"
fi
# Without blank lines at either end (awk only: it runs on macOS too).
notes="$(printf '%s\n' "$notes" | awk 'NF { last = NR } { line[NR] = $0 } END { for (i = 1; i <= last; i++) if (started || line[i] ~ /[^[:space:]]/) { started = 1; print line[i] } }')"
if [ -z "$notes" ]; then
  echo "::error::$changelog has no section headed \"## $tag\": add one before tagging" >&2
  exit 1
fi
printf '%s\n' "$notes"
