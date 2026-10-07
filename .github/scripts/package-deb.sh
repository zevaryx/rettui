#!/usr/bin/env bash
# Make a Debian package (for Debian, Ubuntu, Raspberry Pi OS and the rest)
# from a release's Linux archive: rettui in /usr/bin, its README in
# /usr/share/doc/rettui. Installed this way, rettui leaves updating to apt
# (or dpkg), as with any package.
#
#   .github/scripts/package-deb.sh <tag> <archive> <out-dir>
#   .github/scripts/package-deb.sh v1.7.0 dist/rettui-v1.7.0-x86_64-unknown-linux-gnu.tar.gz dist
set -euo pipefail
tag="${1:?usage: package-deb.sh <tag> <archive> <out-dir>}"
archive="${2:?usage: package-deb.sh <tag> <archive> <out-dir>}"
out="${3:?usage: package-deb.sh <tag> <archive> <out-dir>}"

case "$archive" in
  *x86_64-unknown-linux-gnu*) arch=amd64 ;;
  *aarch64-unknown-linux-gnu*) arch=arm64 ;;
  *) echo "::error::$archive isn't a Linux release archive" >&2; exit 1 ;;
esac
# A prerelease sorts before its release (1.7.0~rc.1 < 1.7.0), as Debian
# orders versions.
version="${tag#v}"
version="${version/-/\~}"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
tar -xzf "$archive" -C "$work"
unpacked="$(find "$work" -mindepth 1 -maxdepth 1 -type d | head -n1)"
root="$work/root"
install -Dm755 "$unpacked/rettui" "$root/usr/bin/rettui"
install -Dm644 "$unpacked/README.md" "$root/usr/share/doc/rettui/README.md"
cat > "$root/usr/share/doc/rettui/copyright" <<COPYRIGHT
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: rettui
Source: https://github.com/zevaryx/rettui

Files: *
License: AGPL-3.0-or-later
 On Debian systems, the GNU Affero General Public License version 3 is
 at https://www.gnu.org/licenses/agpl-3.0.txt.
COPYRIGHT
mkdir -p "$root/DEBIAN"
cat > "$root/DEBIAN/control" <<CONTROL
Package: rettui
Version: $version
Architecture: $arch
Maintainer: rettui <https://github.com/zevaryx/rettui/issues>
Depends: libc6 (>= 2.35), libgcc-s1
Section: net
Priority: optional
Homepage: https://github.com/zevaryx/rettui
Installed-Size: $(du -sk "$root/usr" | cut -f1)
Description: Reticulum client for the terminal and the browser
 LXMF messaging, NomadNet browsing and hosting, and RRC chat over
 Reticulum, in a terminal UI or a web UI (rettui --web).
CONTROL
mkdir -p "$out"
deb="$out/rettui_${version}_${arch}.deb"
dpkg-deb --root-owner-group --build "$root" "$deb" > /dev/null
echo "$deb"
