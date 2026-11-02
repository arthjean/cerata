#!/usr/bin/env bash
# Installs Cerata with its desktop entry and icons under an XDG prefix, which
# every freedesktop desktop (GNOME, KDE, Xfce, Cinnamon, ...) reads. Wayland
# compositors match the window's app_id `cerata` to `cerata.desktop` to find
# the icon; X11 also gets it from the window itself (`_NET_WM_ICON`).
# Usage: scripts/install-linux.sh [prefix]   (default: ~/.local, or /usr/local as root)
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

if [[ $# -gt 0 ]]; then
    prefix="$1"
elif [[ $EUID -eq 0 ]]; then
    prefix=/usr/local
else
    prefix="$HOME/.local"
fi

cargo build -p cerata --release
target_dir="$(cargo metadata --format-version 1 --no-deps | sed -E 's/.*"target_directory":"([^"]+)".*/\1/')"
brand=crates/cerata/assets/brand

install -Dm755 "$target_dir/release/cerata" "$prefix/bin/cerata"
install -Dm644 packaging/linux/cerata.desktop "$prefix/share/applications/cerata.desktop"
for size in 16 32 48 64 128 256 512; do
    install -Dm644 "$brand/cerata-icon-$size.png" \
        "$prefix/share/icons/hicolor/${size}x${size}/apps/cerata.png"
done
install -Dm644 "$brand/cerata-icon.svg" "$prefix/share/icons/hicolor/scalable/apps/cerata.svg"

# Cache refreshes are optional and absent on some distros.
command -v gtk-update-icon-cache >/dev/null &&
    gtk-update-icon-cache -qtf "$prefix/share/icons/hicolor" || true
command -v update-desktop-database >/dev/null &&
    update-desktop-database -q "$prefix/share/applications" || true

echo "Installed Cerata under $prefix"
