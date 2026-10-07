#!/usr/bin/env bash
# Print a Homebrew formula for a release, from its SHA256SUMS: the
# ready-built binaries for macOS (Apple silicon and Intel) and Linux. For a
# tap (a GitHub repository named homebrew-<name>, with it at
# Formula/rettui.rb): `brew install <owner>/<name>/rettui`. Installed this
# way, rettui leaves updating to `brew upgrade`.
#
#   .github/scripts/homebrew-formula.sh v1.7.0 dist/SHA256SUMS > rettui.rb
set -euo pipefail
tag="${1:?usage: homebrew-formula.sh <tag> <SHA256SUMS>}"
sums="${2:?usage: homebrew-formula.sh <tag> <SHA256SUMS>}"
repo="${GITHUB_REPOSITORY:-zevaryx/rettui}"

sha() {
  local name="rettui-$tag-$1.tar.gz"
  local hash
  hash="$(awk -v name="$name" '{ file = $2; sub(/^\*/, "", file) } file == name { print $1 }' "$sums")"
  if [ -z "$hash" ]; then
    echo "::error::$sums has no $name" >&2
    exit 1
  fi
  printf '%s' "$hash"
}
url() { printf 'https://github.com/%s/releases/download/%s/rettui-%s-%s.tar.gz' "$repo" "$tag" "$tag" "$1"; }

cat <<FORMULA
class Rettui < Formula
  desc "Reticulum client for the terminal and the browser"
  homepage "https://github.com/$repo"
  version "${tag#v}"
  license "AGPL-3.0-or-later"

  on_macos do
    on_arm do
      url "$(url aarch64-apple-darwin)"
      sha256 "$(sha aarch64-apple-darwin)"
    end
    on_intel do
      url "$(url x86_64-apple-darwin)"
      sha256 "$(sha x86_64-apple-darwin)"
    end
  end

  on_linux do
    on_arm do
      url "$(url aarch64-unknown-linux-gnu)"
      sha256 "$(sha aarch64-unknown-linux-gnu)"
    end
    on_intel do
      url "$(url x86_64-unknown-linux-gnu)"
      sha256 "$(sha x86_64-unknown-linux-gnu)"
    end
  end

  def install
    bin.install "rettui"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/rettui --version")
  end
end
FORMULA
