#!/bin/sh
# Check scripts/build-deb.sh without a display or root: build a package from a
# stand-in binary and inspect it with dpkg-deb.
# Usage: scripts/test-deb.sh <empty scratch dir>
set -eu

[ $# -eq 1 ] || { echo "Usage: $0 <empty scratch dir>" >&2; exit 2; }
root=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)
build=$root/scripts/build-deb.sh
t=$(CDPATH='' cd -- "$1" && pwd)
fail() { echo "FAIL: $*" >&2; exit 1; }

version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -n 1)
arch=$(dpkg --print-architecture)
deb=$t/out/lumen_${version}_$arch.deb

# Any ELF binary will do: the package only copies it.
cp /usr/bin/true "$t/lumen"
mkdir "$t/out"
out=$("$build" "$t/lumen" "$t/out")
[ "$out" = "Built $deb" ] || fail "output: $out"
[ -f "$deb" ] || fail "no package at $deb"
left=$(ls -A "$t/out")
[ "$left" = "$(basename "$deb")" ] || fail "left in the out dir: $left"

field() { dpkg-deb --field "$deb" "$1"; }
[ "$(field Package)" = lumen ] || fail "Package: $(field Package)"
[ "$(field Version)" = "$version" ] || fail "Version: $(field Version)"
[ "$(field Architecture)" = "$arch" ] || fail "Architecture: $(field Architecture)"
[ "$(field Maintainer)" = "Angelbyte96 <108975993+Angelbyte96@users.noreply.github.com>" ] || fail "Maintainer: $(field Maintainer)"
[ "$(field Section)" = utils ] || fail "Section: $(field Section)"
# Optional tools Lumen runs: xdg-open (files), pdftoppm (PDF preview), gio (Open With).
[ "$(field Recommends)" = "xdg-utils, poppler-utils, libglib2.0-bin" ] || fail "Recommends: $(field Recommends)"
# ffmpeg (video frames) is large, so it is only suggested: apt does not install it by default.
[ "$(field Suggests)" = "ffmpeg" ] || fail "Suggests: $(field Suggests)"
[ "$(field Homepage)" = "https://github.com/Angelbyte96/lumen-explorer" ] || fail "Homepage: $(field Homepage)"
field Description | head -n 1 | grep -qxF "A fast, lightweight, keyboard-first file explorer" || fail "Description: $(field Description)"
depends=$(field Depends)
# From dpkg-shlibdeps (linked), then what winit and glutin load at run time.
printf '%s\n' "$depends" | grep -q "^libc6 (>= [0-9.]*)" || fail "Depends has no versioned libc6: $depends"
for lib in libegl1 libgl1 libxrender1 libwayland-client0 libwayland-egl1 libxkbcommon0 libxkbcommon-x11-0 libx11-6 libx11-xcb1 libxcb1 libxcursor1 libxi6; do
    printf '%s\n' "$depends" | tr ',' '\n' | sed 's/^ *//' | grep -qx "$lib" || fail "Depends lacks $lib: $depends"
done

# Every file, its mode and owner (root, though built by this user).
listed=$(dpkg-deb --contents "$deb" | awk '{ print $1, $2, $6 }')
expected="drwxr-xr-x root/root ./
drwxr-xr-x root/root ./usr/
drwxr-xr-x root/root ./usr/bin/
-rwxr-xr-x root/root ./usr/bin/lumen
drwxr-xr-x root/root ./usr/share/
drwxr-xr-x root/root ./usr/share/applications/
-rw-r--r-- root/root ./usr/share/applications/lumen.desktop
drwxr-xr-x root/root ./usr/share/doc/
drwxr-xr-x root/root ./usr/share/doc/lumen/
-rw-r--r-- root/root ./usr/share/doc/lumen/copyright
drwxr-xr-x root/root ./usr/share/icons/
drwxr-xr-x root/root ./usr/share/icons/hicolor/
drwxr-xr-x root/root ./usr/share/icons/hicolor/256x256/
drwxr-xr-x root/root ./usr/share/icons/hicolor/256x256/apps/
-rw-r--r-- root/root ./usr/share/icons/hicolor/256x256/apps/lumen.png
drwxr-xr-x root/root ./usr/share/icons/hicolor/scalable/
drwxr-xr-x root/root ./usr/share/icons/hicolor/scalable/apps/
-rw-r--r-- root/root ./usr/share/icons/hicolor/scalable/apps/lumen.svg"
[ "$listed" = "$expected" ] || fail "contents:
$listed"
[ -z "$(dpkg-deb --ctrl-tarfile "$deb" | tar -t | grep -E 'inst|rm$')" ] || fail "maintainer scripts in the package"

# The files are the ones in the repository, as they are.
dpkg-deb -x "$deb" "$t/x"
cmp -s "$t/x/usr/bin/lumen" "$t/lumen" || fail "binary differs"
cmp -s "$t/x/usr/share/applications/lumen.desktop" "$root/assets/lumen.desktop" || fail "desktop entry differs"
cmp -s "$t/x/usr/share/icons/hicolor/scalable/apps/lumen.svg" "$root/assets/lumen.svg" || fail "svg icon differs"
cmp -s "$t/x/usr/share/icons/hicolor/256x256/apps/lumen.png" "$root/assets/lumen.png" || fail "png icon differs"
desktop-file-validate "$t/x/usr/share/applications/lumen.desktop" || fail "desktop entry does not validate"
grep -qx "Exec=lumen --restore %f" "$t/x/usr/share/applications/lumen.desktop" || fail "Exec is not on the PATH"
grep -qx "License: MIT" "$t/x/usr/share/doc/lumen/copyright" || fail "copyright names no MIT license"

# Wrong arguments: usage, exit 2, nothing built.
rm -rf "$t/out" "$t/x"
mkdir "$t/out"
for args in "" "$t/lumen" "$t/lumen $t/out extra"; do
    # shellcheck disable=SC2086 # split on purpose
    if err=$("$build" $args 2>&1 >/dev/null); then fail "accepted: '$args'"; else code=$?; fi
    [ "$code" -eq 2 ] || fail "'$args' exit $code"
    [ "$err" = "Usage: $build <binary> <out dir>" ] || fail "'$args' said: $err"
done

# A missing binary or out dir: said plainly, exit 1, nothing built.
if err=$("$build" "$t/nope" "$t/out" 2>&1 >/dev/null); then fail "missing binary accepted"; else code=$?; fi
[ "$code" -eq 1 ] && [ "$err" = "No binary at $t/nope" ] || fail "missing binary: exit $code, said: $err"
if err=$("$build" "$t/lumen" "$t/none" 2>&1 >/dev/null); then fail "missing out dir accepted"; else code=$?; fi
[ "$code" -eq 1 ] && [ "$err" = "No folder at $t/none" ] || fail "missing out dir: exit $code, said: $err"
[ -z "$(ls -A "$t/out")" ] || fail "refused builds left: $(ls -A "$t/out")"

# A file that links nothing: refused plainly, exit 1, nothing built.
printf '#!/bin/sh\n' >"$t/script"
if err=$("$build" "$t/script" "$t/out" 2>&1 >/dev/null); then fail "script accepted"; else code=$?; fi
[ "$code" -eq 1 ] && [ "$err" = "dpkg-shlibdeps found no libraries in $t/script" ] || fail "script: exit $code, said: $err"
[ -z "$(ls -A "$t/out")" ] || fail "refused script left: $(ls -A "$t/out")"

# A failed dpkg-deb leaves no half-written package behind.
mkdir -p "$t/fake"
printf '#!/bin/sh\n[ "$1" = --root-owner-group ] || exec /usr/bin/dpkg-deb "$@"\n: >"$4"\nexit 2\n' >"$t/fake/dpkg-deb"
chmod +x "$t/fake/dpkg-deb"
if PATH="$t/fake:$PATH" "$build" "$t/lumen" "$t/out" >/dev/null 2>&1; then fail "failed dpkg-deb reported success"; fi
[ -z "$(ls -A "$t/out")" ] || fail "failed dpkg-deb left: $(ls -A "$t/out")"
rm -rf "$t/fake"

# A relative out dir with CDPATH set still names one absolute package.
mkdir "$t/rel"
out=$(cd "$t" && CDPATH="$t" "$build" lumen rel)
[ "$out" = "Built $t/rel/lumen_${version}_$arch.deb" ] || fail "CDPATH: $out"

echo "build-deb.sh: all checks passed"
