#!/usr/bin/env bash
# Push the AUR package (PKGBUILD and .SRCINFO, from aur-pkgbuild.sh) to its
# AUR repository: the first push makes the package. Nothing is pushed if
# the AUR has it already, so running it again is harmless. The commit's
# author is AUR_USERNAME <AUR_EMAIL>; git reaches the AUR however it's set
# up to (in the release workflow, GIT_SSH_COMMAND with the account's key).
#
#   .github/scripts/publish-aur.sh v1.7.0 aur ssh://aur@aur.archlinux.org/rettui-bin.git
set -euo pipefail
tag="${1:?usage: publish-aur.sh <tag> <package-dir> <aur-url>}"
dir="${2:?usage: publish-aur.sh <tag> <package-dir> <aur-url>}"
url="${3:?usage: publish-aur.sh <tag> <package-dir> <aur-url>}"
if [ -z "${AUR_USERNAME:-}" ] || [ -z "${AUR_EMAIL:-}" ]; then
  echo "::error::AUR_USERNAME and AUR_EMAIL are needed: the commit author" >&2
  exit 1
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
# A package that isn't there yet clones as an empty repository; the AUR
# takes master only.
git -c init.defaultBranch=master clone -q "$url" "$work/package"
cp "$dir/PKGBUILD" "$dir/.SRCINFO" "$work/package/"
cd "$work/package"
git add PKGBUILD .SRCINFO
if git diff --cached --quiet; then
  echo "The AUR has ${tag#v} already"
  exit 0
fi
git -c user.name="$AUR_USERNAME" -c user.email="$AUR_EMAIL" commit -q -m "Update to ${tag#v}"
git push -q origin HEAD:master
echo "Published ${tag#v} to the AUR"
