#!/usr/bin/env bash
# Get the checkout ready to tag as a development prerelease, and print its
# tag: v<version>-dev.<n>.
#
# <version> is the one given, else the version in Cargo.toml if it hasn't
# been released yet, else the next minor version after the newest release.
# <n> counts that version's dev prereleases (from the tags). Cargo.toml and
# Cargo.lock get the prerelease's version (so `rettui --version` says it),
# and CHANGELOG.md a section for it: what it is, then the version's own
# section if there is one yet.
#
#   .github/scripts/prepare-prerelease.sh [version]
set -euo pipefail

cargo_version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)"
# Newest release first; prereleases (with a -) left out.
released="$(git tag -l 'v*' --sort=-v:refname | grep -v -- - | head -n1 || true)"

version="${1:-}"
version="${version#v}"
if [ -z "$version" ]; then
  if [ "v${cargo_version%%-*}" != "$released" ]; then
    version="${cargo_version%%-*}"
  else
    IFS=. read -r major minor _ <<< "${released#v}"
    version="$major.$((minor + 1)).0"
  fi
fi
if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "::error::$version isn't a version (like 1.10.0)" >&2
  exit 1
fi
if git rev-parse -q --verify "refs/tags/v$version" > /dev/null; then
  echo "::error::v$version is already released: give a later version" >&2
  exit 1
fi

n=$(( $(git tag -l "v$version-dev.*" | wc -l) + 1 ))
tag="v$version-dev.$n"
while git rev-parse -q --verify "refs/tags/$tag" > /dev/null; do
  n=$((n + 1))
  tag="v$version-dev.$n"
done

# The package's own version, in Cargo.toml and in Cargo.lock's entry for it.
sed -i.bak "0,/^version = \".*\"/s//version = \"${tag#v}\"/" Cargo.toml
awk -v v="${tag#v}" '
  /^name = "rettui"$/ { print; mine = 1; next }
  mine && /^version = / { print "version = \"" v "\""; mine = 0; next }
  { mine = 0; print }
' Cargo.lock > Cargo.lock.new
mv Cargo.lock.new Cargo.lock
rm -f Cargo.toml.bak

# Its notes: what it is, then what's coming, if CHANGELOG.md says yet.
commit="$(git rev-parse --short=7 HEAD)"
coming="$(.github/scripts/release-notes.sh "v$version" 2> /dev/null || true)"
{
  printf '## %s\n\n' "$tag"
  printf 'A development build of `dev` at %s, to try what is coming in v%s before it is released. ' "$commit" "$version"
  printf 'It may have bugs: back up your data directory first.'
  if [ -n "$released" ]; then
    printf ' What changed since %s: [the commits](https://github.com/%s/compare/%s...%s).' \
      "$released" "${GITHUB_REPOSITORY:-zevaryx/rettui}" "$released" "$commit"
  fi
  printf '\n\n'
  if [ -n "$coming" ]; then
    printf '%s\n\n' "$coming"
  fi
} > notes.md
# Before the first release's section.
awk 'FNR == NR { notes = notes $0 "\n"; next } !done && /^## / { printf "%s", notes; done = 1 } { print }' notes.md CHANGELOG.md > CHANGELOG.md.new
mv CHANGELOG.md.new CHANGELOG.md
rm notes.md

printf '%s\n' "$tag"
