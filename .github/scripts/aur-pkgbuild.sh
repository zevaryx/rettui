#!/usr/bin/env bash
# Write the Arch User Repository's rettui-bin package for a release, from
# its SHA256SUMS: PKGBUILD and .SRCINFO in <out-dir>, to commit to the
# package's AUR repository (ssh://aur@aur.archlinux.org/rettui-bin.git).
# The ready-built Linux binaries, x86-64 and ARM64. Installed this way,
# rettui leaves updating to pacman (or an AUR helper). AUR_MAINTAINER, if
# set ("Name <email>"), is the PKGBUILD's maintainer line.
#
# With `desktop` after the out-dir, it writes rettui-desktop-bin instead:
# the desktop app (rettui's web UI in its own window), from the Desktop
# workflow's Linux archives, with its menu entry and icon.
#
#   .github/scripts/aur-pkgbuild.sh v1.7.0 dist/SHA256SUMS aur
#   .github/scripts/aur-pkgbuild.sh v1.7.0 dist/SHA256SUMS aur-desktop desktop
set -euo pipefail
tag="${1:?usage: aur-pkgbuild.sh <tag> <SHA256SUMS> <out-dir>}"
sums="${2:?usage: aur-pkgbuild.sh <tag> <SHA256SUMS> <out-dir>}"
out="${3:?usage: aur-pkgbuild.sh <tag> <SHA256SUMS> <out-dir>}"
kind="${4:-rettui}"
repo="${GITHUB_REPOSITORY:-zevaryx/rettui}"
version="${tag#v}"
if [[ "$version" == *-* ]]; then
  echo "::error::$tag is a prerelease: the AUR package follows releases only" >&2
  exit 1
fi

case "$kind" in
  rettui)
    program=rettui
    desc="Reticulum client for the terminal and the browser: LXMF messaging, NomadNet browsing and hosting, and RRC chat"
    depends=('glibc' 'gcc-libs')
    ;;
  desktop)
    program=rettui-desktop
    desc="Reticulum client in a window of its own (rettui's web UI): LXMF messaging, NomadNet browsing and hosting, and RRC chat"
    depends=('glibc' 'gcc-libs' 'gtk3' 'webkit2gtk-4.1')
    ;;
  *)
    echo "::error::$kind isn't a package (rettui or desktop)" >&2
    exit 1
    ;;
esac
pkgname="$program-bin"

sha() {
  local name="$program-$tag-$1.tar.gz"
  local hash
  hash="$(awk -v name="$name" '{ file = $2; sub(/^\*/, "", file) } file == name { print $1 }' "$sums")"
  if [ -z "$hash" ]; then
    echo "::error::$sums has no $name" >&2
    exit 1
  fi
  printf '%s' "$hash"
}
x86="$(sha x86_64-unknown-linux-gnu)"
arm="$(sha aarch64-unknown-linux-gnu)"
base="https://github.com/$repo/releases/download/v\${pkgver}/$program-v\${pkgver}"
quoted_depends="$(printf "'%s' " "${depends[@]}")"
srcinfo_depends="$(printf '\tdepends = %s\n' "${depends[@]}")"
if [ "$kind" = desktop ]; then
  install_lines="  install -Dm755 rettui-desktop \"\$pkgdir/usr/bin/rettui-desktop\"
  install -Dm644 rettui-desktop.desktop \"\$pkgdir/usr/share/applications/rettui-desktop.desktop\"
  install -Dm644 rettui-desktop.png \"\$pkgdir/usr/share/icons/hicolor/512x512/apps/rettui-desktop.png\"
  install -Dm644 README.md \"\$pkgdir/usr/share/doc/rettui-desktop/README.md\""
else
  install_lines="  install -Dm755 rettui \"\$pkgdir/usr/bin/rettui\"
  install -Dm644 README.md \"\$pkgdir/usr/share/doc/rettui/README.md\""
fi

mkdir -p "$out"
maintainer=""
if [ -n "${AUR_MAINTAINER:-}" ]; then
  maintainer="# Maintainer: $AUR_MAINTAINER
"
fi
cat > "$out/PKGBUILD" <<PKGBUILD
${maintainer}pkgname=$pkgname
pkgver=$version
pkgrel=1
pkgdesc="$desc"
arch=('x86_64' 'aarch64')
url="https://github.com/$repo"
license=('AGPL-3.0-or-later')
depends=(${quoted_depends% })
provides=('$program')
conflicts=('$program')
source_x86_64=("$base-x86_64-unknown-linux-gnu.tar.gz")
source_aarch64=("$base-aarch64-unknown-linux-gnu.tar.gz")
sha256sums_x86_64=('$x86')
sha256sums_aarch64=('$arm')

package() {
  cd "$program-v\${pkgver}-\${CARCH}-unknown-linux-gnu"
$install_lines
}
PKGBUILD
# What makepkg --printsrcinfo would write (it isn't on GitHub's runners).
cat > "$out/.SRCINFO" <<SRCINFO
pkgbase = $pkgname
	pkgdesc = $desc
	pkgver = $version
	pkgrel = 1
	url = https://github.com/$repo
	arch = x86_64
	arch = aarch64
	license = AGPL-3.0-or-later
$srcinfo_depends
	provides = $program
	conflicts = $program
	source_x86_64 = https://github.com/$repo/releases/download/$tag/$program-$tag-x86_64-unknown-linux-gnu.tar.gz
	sha256sums_x86_64 = $x86
	source_aarch64 = https://github.com/$repo/releases/download/$tag/$program-$tag-aarch64-unknown-linux-gnu.tar.gz
	sha256sums_aarch64 = $arm

pkgname = $pkgname
SRCINFO
echo "$out/PKGBUILD"
