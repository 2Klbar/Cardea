#!/usr/bin/env bash
# Installs Cardea for the current user from this checkout (no root): release binaries in ~/.local/bin, the daemon's
# systemd user unit, the menu entry and icon; enables podman.socket and the daemon. The tray's login entry is
# written by the daemon from the "tray at login" setting.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release --bins
bin="$HOME/.local/bin"
units="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
install -d "$bin" "$units"
for b in cardea cardea-gui cardea-tray; do
    install -m755 "target/release/$b" "$bin/$b"
done
install -m644 assets/cardea.service "$units/cardea.service"
scripts/install-desktop.sh
systemctl --user daemon-reload
systemctl --user enable --now podman.socket
# A daemon started by hand (or by a command) is replaced by the unit's.
pkill -u "$(id -u)" -f "^$bin/cardea daemon$" 2>/dev/null || true
systemctl --user enable cardea.service
systemctl --user restart cardea.service
echo "Installed in $bin. The daemon runs (systemctl --user status cardea); open the manager with cardea-gui."
case ":$PATH:" in *":$bin:"*) ;; *) echo "Note: $bin is not in PATH; log out and in, or add it." ;; esac
