#!/bin/sh
# Install Lumen for the current user, without sudo:
#   ~/.local/bin/lumen                                    the release binary
#   ~/.local/share/applications/lumen.desktop             the menu entry
#   ~/.local/share/icons/hicolor/{scalable,256x256}/apps  the icon
#
# Usage: scripts/install-desktop.sh [--uninstall]
# XDG_DATA_HOME and LUMEN_BIN_DIR change where the files go.
# Checked by scripts/test-install.sh.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
data=${XDG_DATA_HOME:-$HOME/.local/share}
bin_dir=${LUMEN_BIN_DIR:-$HOME/.local/bin}
apps=$data/applications
icons=$data/icons/hicolor

refresh() {
    # Both are optional: GNOME notices new files on its own, just later.
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$apps" 2>/dev/null || true
    fi
    # Only update an icon cache that already exists: a new user-level cache
    # would hide icons other apps install later.
    if [ -f "$icons/icon-theme.cache" ] && command -v gtk-update-icon-cache >/dev/null 2>&1; then
        gtk-update-icon-cache -q -t "$icons" 2>/dev/null || true
    fi
}

usage() {
    echo "Usage: $0 [--uninstall]" >&2
    exit 2
}

case $# in
    0) ;;
    1) [ "$1" = "--uninstall" ] || usage ;;
    *) usage ;;
esac

if [ $# -eq 1 ]; then
    rm -f "$bin_dir/lumen" "$apps/lumen.desktop" \
        "$icons/scalable/apps/lumen.svg" "$icons/256x256/apps/lumen.png"
    refresh
    echo "Lumen uninstalled."
    exit 0
fi

# Inside a quoted Exec argument these would need escaping; refuse them instead.
case $bin_dir in
    *[\"\`\$\\]*)
        echo "LUMEN_BIN_DIR must not contain \" \` \$ or \\: $bin_dir" >&2
        exit 1
        ;;
esac

# Any step that fails stops the script (set -e): say so, plainly, and say
# whether the binary had already been replaced.
done_upto=nothing
failed() {
    case $done_upto in
        nothing) echo "Lumen was NOT installed: the step above failed." ;;
        binary) echo "Lumen was only partly installed: $bin_dir/lumen was replaced, but the step above failed." ;;
    esac >&2
}
trap failed EXIT

version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -n 1)
commit=$(git -C "$root" rev-parse --short HEAD 2>/dev/null || echo "no git")
# Edits and new files are built in too (Cargo picks up an untracked build.rs or
# src/bin/), so the commit alone would not name this build. When git fails
# (exit 128), it proves nothing either way.
if [ "$commit" != "no git" ] && changes=$(git -C "$root" status --porcelain 2>/dev/null) && [ -n "$changes" ]; then
    commit="$commit with uncommitted changes"
fi

# Always build: cargo does nothing when the binary is already up to date.
echo "[1/4] Building Lumen $version (release; the first build takes a few minutes)..."
cargo build --release --manifest-path "$root/Cargo.toml"
binary=${CARGO_TARGET_DIR:-$root/target}/release/lumen

echo "[2/4] Installing the binary..."
mkdir -p "$bin_dir" "$apps" "$icons/scalable/apps" "$icons/256x256/apps"
bin_dir=$(cd "$bin_dir" && pwd) # the Exec path must be absolute
# Two commands, not `cp && mv`: set -e does not stop on a failure inside &&.
cp "$binary" "$bin_dir/lumen.tmp"
mv "$bin_dir/lumen.tmp" "$bin_dir/lumen"
done_upto=binary

echo "[3/4] Installing the menu entry and icon..."
cp "$root/assets/lumen.svg" "$icons/scalable/apps/lumen.svg"
cp "$root/assets/lumen.png" "$icons/256x256/apps/lumen.png"
# Absolute Exec path: ~/.local/bin is not always on the PATH GNOME starts apps with.
while IFS= read -r line; do
    case $line in
        "Exec=lumen "*) printf 'Exec="%s" %s\n' "$bin_dir/lumen" "${line#Exec=lumen }" ;;
        *) printf '%s\n' "$line" ;;
    esac
done <"$root/assets/lumen.desktop" >"$apps/lumen.desktop.tmp"
mv "$apps/lumen.desktop.tmp" "$apps/lumen.desktop"

echo "[4/4] Refreshing the app menu..."
refresh

done_upto=all
echo "Done: Lumen $version ($commit) is installed at $bin_dir/lumen"
echo "It shows up in the app menu as \"Lumen\" (it may take a few seconds). Close a running Lumen to use this one."
