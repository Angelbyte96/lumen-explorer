#!/bin/sh
# Install Lumen for the current user, without sudo:
#   ~/.local/bin/lumen                                    the release binary
#   ~/.local/share/applications/lumen.desktop             the menu entry
#   ~/.local/share/icons/hicolor/{scalable,256x256}/apps  the icon
#
# Usage: scripts/install-desktop.sh [--uninstall]
# XDG_DATA_HOME and LUMEN_BIN_DIR change where the files go.
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
    if command -v gtk-update-icon-cache >/dev/null 2>&1; then
        gtk-update-icon-cache -q -t "$icons" 2>/dev/null || true
    fi
}

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$bin_dir/lumen" "$apps/lumen.desktop" \
        "$icons/scalable/apps/lumen.svg" "$icons/256x256/apps/lumen.png"
    refresh
    echo "Lumen uninstalled."
    exit 0
elif [ $# -gt 0 ]; then
    echo "Usage: $0 [--uninstall]" >&2
    exit 2
fi

# Always build: cargo does nothing when the binary is already up to date.
cargo build --release --manifest-path "$root/Cargo.toml"
binary=$root/target/release/lumen

mkdir -p "$bin_dir" "$apps" "$icons/scalable/apps" "$icons/256x256/apps"
cp "$binary" "$bin_dir/lumen.tmp" && mv "$bin_dir/lumen.tmp" "$bin_dir/lumen"
cp "$root/assets/lumen.svg" "$icons/scalable/apps/lumen.svg"
cp "$root/assets/lumen.png" "$icons/256x256/apps/lumen.png"
# Absolute Exec path: ~/.local/bin is not always on the PATH GNOME starts apps with.
sed "s|^Exec=lumen |Exec=\"$bin_dir/lumen\" |" "$root/assets/lumen.desktop" > "$apps/lumen.desktop"
refresh

echo "Lumen installed: $bin_dir/lumen"
echo "It shows up in the app menu as \"Lumen\" (it may take a few seconds)."
