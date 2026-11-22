#!/usr/bin/env bash
# Builds dist/cardea_<version>_<arch>.deb from this checkout: the three binaries, the daemon's systemd user unit
# (enabled for every user, with podman.socket), the menu entry and icon. Needs cargo, dpkg-deb and dpkg-shlibdeps.
# The maintainer field comes from $DEB_MAINTAINER ("Name <email>").
set -euo pipefail
cd "$(dirname "$0")/.."
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
arch=$(dpkg --print-architecture)
cargo build --release --bins
root=target/deb/root
rm -rf target/deb
install -d "$root/DEBIAN" "$root/usr/bin"
for b in cardea cardea-gui cardea-tray; do
    install -s -m755 "target/release/$b" "$root/usr/bin/$b"
done
install -Dm644 /dev/stdin "$root/usr/lib/systemd/user/cardea.service" \
    < <(sed 's|%h/.local/bin/cardea|/usr/bin/cardea|' assets/cardea.service)
install -Dm644 assets/cardea.desktop "$root/usr/share/applications/cardea.desktop"
install -Dm644 assets/logo/cardea-app.svg "$root/usr/share/icons/hicolor/scalable/apps/cardea.svg"
install -Dm644 LICENSE "$root/usr/share/doc/cardea/copyright"
install -Dm755 scripts/selftest.sh "$root/usr/share/cardea/selftest.sh"
install -Dm644 -t "$root/usr/share/cardea/profiles" profiles/*.json
install -m755 assets/deb/postinst assets/deb/prerm "$root/DEBIAN/"
# Shared libraries the binaries link to; winit opens Wayland and xkbcommon at run time, so they are listed by hand.
mkdir -p target/deb/shlibs/debian
printf 'Source: cardea\n\nPackage: cardea\nArchitecture: any\n' > target/deb/shlibs/debian/control
libs=$(cd target/deb/shlibs && dpkg-shlibdeps -O -e "../../../$root"/usr/bin/* 2>/dev/null | sed -n 's/^shlibs:Depends=//p')
cat > "$root/DEBIAN/control" <<CONTROL
Package: cardea
Version: $version
Architecture: $arch
Maintainer: ${DEB_MAINTAINER:-Cardea maintainers <cardea@localhost>}
Section: utils
Priority: optional
Homepage: https://github.com/2Klbar/Cardea
Depends: ${libs:+$libs, }podman (>= 4.4), passt, catatonit, uidmap, dbus-user-session, systemd, libwayland-client0, libxkbcommon0, fontconfig
Recommends: xdg-utils, xdg-terminal-exec | konsole | gnome-console | gnome-terminal
Installed-Size: $(du -sk "$root/usr" | cut -f1)
Description: Isolated desktop sandboxes on rootless Podman
 Cardea creates, runs and manages isolated sandboxes from profiles: a daemon,
 a command line, a tray icon and a manager window. Each sandbox has its own
 home, shared folders with protected paths, a firewall on by default and
 explicit capabilities.
CONTROL
mkdir -p dist
deb="dist/cardea_${version}_${arch}.deb"
dpkg-deb --root-owner-group --build "$root" "$deb"
echo "Built $deb"
