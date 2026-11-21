#!/usr/bin/env bash
# Installs the desktop entry and the app icon for the current user (no root): the window and menus then show the
# Cardea logo. On Wayland the window icon comes from the desktop entry named after the window's app id, `cardea`.
set -euo pipefail
cd "$(dirname "$0")/.."
data="${XDG_DATA_HOME:-$HOME/.local/share}"
install -Dm644 assets/cardea.desktop "$data/applications/cardea.desktop"
install -Dm644 assets/logo/cardea-app.svg "$data/icons/hicolor/scalable/apps/cardea.svg"
command -v update-desktop-database >/dev/null && update-desktop-database "$data/applications" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$data/icons/hicolor" || true
echo "Installed $data/applications/cardea.desktop and the cardea icon."
