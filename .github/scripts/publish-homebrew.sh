#!/usr/bin/env bash
# Put a formula (from homebrew-formula.sh) in a Homebrew tap as
# Formula/rettui.rb, and push it: a tap is a git repository (on GitHub,
# named homebrew-<name>). Nothing is pushed if the tap has it already, so
# running it again is harmless. git reaches the tap however it's set up
# to (in the release workflow, GIT_SSH_COMMAND with the tap's deploy key).
#
#   .github/scripts/publish-homebrew.sh v1.7.0 rettui.rb git@github.com:zevaryx/homebrew-rettui.git
set -euo pipefail
tag="${1:?usage: publish-homebrew.sh <tag> <formula> <tap-url>}"
formula="${2:?usage: publish-homebrew.sh <tag> <formula> <tap-url>}"
url="${3:?usage: publish-homebrew.sh <tag> <formula> <tap-url>}"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
# An empty tap starts on its default branch (on GitHub, main).
git -c init.defaultBranch=main clone -q "$url" "$work/tap"
mkdir -p "$work/tap/Formula"
cp "$formula" "$work/tap/Formula/rettui.rb"
cd "$work/tap"
git add Formula/rettui.rb
if git diff --cached --quiet; then
  echo "The tap has rettui ${tag#v} already"
  exit 0
fi
git -c user.name="github-actions[bot]" -c user.email="41898282+github-actions[bot]@users.noreply.github.com" \
  commit -q -m "rettui ${tag#v}"
git push -q origin HEAD
echo "Published rettui ${tag#v} to the tap"
