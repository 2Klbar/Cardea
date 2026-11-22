#!/usr/bin/env bash
# Builds dist/cardea-<version>-1.<arch>.rpm from this checkout, with the same content as the Debian package.
# Needs cargo and cargo-generate-rpm (`cargo install cargo-generate-rpm`); no rpmbuild.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release --bins
stage=$PWD/target/rpm
rm -rf "$stage"
install -d "$stage"
for b in cardea cardea-gui cardea-tray; do
    install -s -m755 "target/release/$b" "$stage/$b"
done
sed 's|%h/.local/bin/cardea|/usr/bin/cardea|' assets/cardea.service > "$stage/cardea.service"
assets=""
asset() { assets+="{ source = \"$1\", dest = \"$2\", mode = \"$3\"${4:-} },"$'\n'; }
for b in cardea cardea-gui cardea-tray; do asset "$stage/$b" "/usr/bin/$b" 755; done
asset "$stage/cardea.service" /usr/lib/systemd/user/cardea.service 644
asset "$PWD/assets/cardea.desktop" /usr/share/applications/cardea.desktop 644
asset "$PWD/assets/logo/cardea-app.svg" /usr/share/icons/hicolor/scalable/apps/cardea.svg 644
asset "$PWD/LICENSE" /usr/share/licenses/cardea/LICENSE 644 ', doc = true'
asset "$PWD/scripts/selftest.sh" /usr/share/cardea/selftest.sh 755
for p in profiles/*.json; do asset "$PWD/$p" "/usr/share/cardea/$p" 644; done
cat > "$stage/rpm.toml" <<TOML
name = "cardea"
release = "1"
summary = "Isolated desktop sandboxes on rootless Podman"
url = "https://github.com/2Klbar/Cardea"
description = """
Cardea creates, runs and manages isolated sandboxes from profiles: a daemon,
a command line, a tray icon and a manager window. Each sandbox has its own
home, shared folders with protected paths, a firewall on by default and
explicit capabilities."""
assets = [
$assets]
# Enables, for every user, Podman's API socket and the Cardea daemon; both start with the next desktop login.
post_install_script = '''
if command -v systemctl >/dev/null; then
    systemctl --global enable podman.socket cardea.service || :
    echo "Cardea: log out and in again (or run: systemctl --user daemon-reload && systemctl --user start podman.socket cardea)."
fi
'''
# On removal only, not upgrade; podman.socket stays enabled: other tools may use it.
pre_uninstall_script = '''
if [ "\$1" -eq 0 ] && command -v systemctl >/dev/null; then
    systemctl --global disable cardea.service || :
fi
'''

# Shared libraries are found automatically; winit opens Wayland and xkbcommon at run time, so they are listed here.
[requires]
podman = ">= 4.4"
passt = "*"
catatonit = "*"
shadow-utils = "*"
systemd = "*"
dbus = "*"
fontconfig = "*"
"libwayland-client.so.0()(64bit)" = "*"
"libxkbcommon.so.0()(64bit)" = "*"

[recommends]
xdg-utils = "*"
TOML
mkdir -p dist
cargo generate-rpm -p crates/cli --metadata-overwrite "$stage/rpm.toml" -o dist/
echo "Built $(ls -t dist/cardea-*.rpm | head -n1)"
