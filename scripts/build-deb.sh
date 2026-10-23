#!/bin/sh
# Build lumen_<version>_<arch>.deb from a release binary, with no root:
#   /usr/bin/lumen                                        the binary
#   /usr/share/applications/lumen.desktop                 the menu entry
#   /usr/share/icons/hicolor/{scalable,256x256}/apps      the icon
#   /usr/share/doc/lumen/copyright                        the license
# The same files scripts/install-desktop.sh installs per user, system-wide.
# No maintainer scripts: dpkg triggers refresh the menu and the icon cache.
#
# Usage: scripts/build-deb.sh <binary> <out dir>
# Checked by scripts/test-deb.sh.
set -eu

[ $# -eq 2 ] || { echo "Usage: $0 <binary> <out dir>" >&2; exit 2; }
binary=$1
[ -f "$binary" ] || { echo "No binary at $binary" >&2; exit 1; }
[ -d "$2" ] || { echo "No folder at $2" >&2; exit 1; }
# CDPATH unset: a cd through it would also print the folder.
out=$(CDPATH='' cd -- "$2" && pwd)

root=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -n 1)
description=$(sed -n 's/^description = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -n 1)
arch=$(dpkg --print-architecture)
deb=$out/lumen_${version}_$arch.deb

stage=$(mktemp -d)
trap 'rm -rf "$stage" "$deb.tmp"' EXIT
umask 022
pkg=$stage/pkg
install -D -m 0755 "$binary" "$pkg/usr/bin/lumen"
install -D -m 0644 "$root/assets/lumen.desktop" "$pkg/usr/share/applications/lumen.desktop"
install -D -m 0644 "$root/assets/lumen.svg" "$pkg/usr/share/icons/hicolor/scalable/apps/lumen.svg"
install -D -m 0644 "$root/assets/lumen.png" "$pkg/usr/share/icons/hicolor/256x256/apps/lumen.png"
mkdir -p "$pkg/usr/share/doc/lumen"
cat >"$pkg/usr/share/doc/lumen/copyright" <<'EOF'
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: lumen
Source: https://github.com/Angelbyte96/lumen-explorer

Files: *
Copyright: 2026 Angelbyte96
License: MIT
 Permission is hereby granted, free of charge, to any person obtaining a copy
 of this software and associated documentation files (the "Software"), to deal
 in the Software without restriction, including without limitation the rights
 to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 copies of the Software, and to permit persons to whom the Software is
 furnished to do so, subject to the following conditions:
 .
 The above copyright notice and this permission notice shall be included in
 all copies or substantial portions of the Software.
 .
 THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
 THE SOFTWARE.
EOF

# What the binary links (libc, libgcc), versioned by dpkg-shlibdeps, which
# wants a debian/control beside it.
mkdir "$stage/debian"
printf 'Source: lumen\n\nPackage: lumen\nArchitecture: any\n' >"$stage/debian/control"
linked=$(CDPATH='' cd -- "$stage" && dpkg-shlibdeps -O "$pkg/usr/bin/lumen" 2>/dev/null | sed -n 's/^shlibs:Depends=//p')
[ -n "$linked" ] || { echo "dpkg-shlibdeps found no libraries in $binary" >&2; exit 1; }
# What winit and glutin open at run time (Wayland, X11, OpenGL): ldd does not
# show them, and Lumen does not start without them.
loaded="libegl1, libgl1, libxrender1, libwayland-client0, libwayland-egl1, libxkbcommon0, libxkbcommon-x11-0, libx11-6, libx11-xcb1, libxcb1, libxcursor1, libxi6"
size=$(du -k -s "$pkg" | cut -f 1)

mkdir "$pkg/DEBIAN"
cat >"$pkg/DEBIAN/control" <<EOF
Package: lumen
Version: $version
Architecture: $arch
Maintainer: Angelbyte96 <108975993+Angelbyte96@users.noreply.github.com>
Installed-Size: $size
Depends: $linked, $loaded
Recommends: xdg-utils, poppler-utils, libglib2.0-bin
Section: utils
Priority: optional
Homepage: https://github.com/Angelbyte96/lumen-explorer
Description: $description
 Lumen browses folders with the keyboard first: tabs, quick jump,
 thumbnails, drag and drop, and the trash.
EOF

dpkg-deb --root-owner-group --build "$pkg" "$deb.tmp" >/dev/null
mv "$deb.tmp" "$deb"
echo "Built $deb"
