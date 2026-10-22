#!/bin/sh
# Check scripts/install-desktop.sh without touching the real ~/.local.
# Usage: scripts/test-install.sh <empty scratch dir>
set -eu

[ $# -eq 1 ] || { echo "Usage: $0 <empty scratch dir>" >&2; exit 2; }
root=$(cd "$(dirname "$0")/.." && pwd)
install=$root/scripts/install-desktop.sh
t=$(cd "$1" && pwd)
fail() { echo "FAIL: $*" >&2; exit 1; }

# Spaces, & and | in the paths end up verbatim in Exec.
export XDG_DATA_HOME="$t/data dir" LUMEN_BIN_DIR="$t/bin & dir|x"
out=$("$install")
entry=$XDG_DATA_HOME/applications/lumen.desktop

# Every step is announced, and the last line says what was installed where.
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -n 1)
for step in "[1/4] Building Lumen" "[2/4] Installing the binary" "[3/4] Installing the menu entry and icon" "[4/4] Refreshing the app menu"; do
    printf '%s\n' "$out" | grep -qF "$step" || fail "step not shown: $step"
done
printf '%s\n' "$out" | tail -n 2 | head -n 1 | grep -q "^Done: Lumen $version (.*) is installed at $LUMEN_BIN_DIR/lumen$" ||
    fail "summary: $(printf '%s\n' "$out" | tail -n 2)"
desktop-file-validate "$entry" || fail "installed entry does not validate"
grep -qxF "Exec=\"$LUMEN_BIN_DIR/lumen\" --restore %f" "$entry" || fail "Exec: $(grep Exec "$entry")"
cmp -s "$LUMEN_BIN_DIR/lumen" "${CARGO_TARGET_DIR:-$root/target}/release/lumen" || fail "binary differs"

# A failed build says so, exits non-zero and installs nothing.
mkdir -p "$t/fake"
printf '#!/bin/sh\nexit 101\n' >"$t/fake/cargo"
chmod +x "$t/fake/cargo"
if err=$(PATH="$t/fake:$PATH" XDG_DATA_HOME="$t/fail" LUMEN_BIN_DIR="$t/fail/bin" "$install" 2>&1 >/dev/null); then
    fail "failed build reported success"
fi
printf '%s\n' "$err" | grep -qF "Lumen was NOT installed" || fail "failure message: $err"
[ ! -e "$t/fail/bin/lumen" ] || fail "failed build installed a binary"
rm -rf "$t/fake" "$t/fail"

# Extra arguments are refused and change nothing.
if "$install" --uninstall extra 2>/dev/null; then fail "extra argument accepted"; fi
[ -x "$LUMEN_BIN_DIR/lumen" ] || fail "refused uninstall removed the binary"

# No icon cache is created where there was none, so uninstall leaves nothing.
"$install" --uninstall >/dev/null
left=$(find "$t" -name 'lumen*' -o -name 'icon-theme.cache')
[ -z "$left" ] || fail "left behind: $left"

# A relative LUMEN_BIN_DIR still gives an absolute Exec.
(cd "$t" && LUMEN_BIN_DIR=rel "$install" >/dev/null)
grep -qxF "Exec=\"$t/rel/lumen\" --restore %f" "$entry" || fail "relative Exec: $(grep Exec "$entry")"
(cd "$t" && LUMEN_BIN_DIR=rel "$install" --uninstall >/dev/null)

echo "install-desktop.sh: all checks passed"
