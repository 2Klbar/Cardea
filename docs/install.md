# Installation

## Package

Cardea 1.0 ships as two packages, for amd64 (x86_64):

| Package | For | Install with |
|---|---|---|
| `cardea_1.0.0_amd64.deb` | Debian 13 (trixie) and later | `sudo apt install ./cardea_1.0.0_amd64.deb` |
| `cardea-1.0.0-1.x86_64.rpm` | Fedora 41 and later | `sudo dnf install ./cardea-1.0.0-1.x86_64.rpm` |

The package manager installs what Cardea needs (Podman, `passt`, `catatonit`, the sub-UID tools). The package
then enables, for every user, Podman's API socket (`podman.socket`) and the Cardea daemon (`cardea.service`); both start with the next
desktop login. To start them in a session that is already open:

```bash
systemctl --user daemon-reload
```

```bash
systemctl --user start podman.socket cardea
```

Then check the computer:

```bash
cardea doctor
```

| Installed | Where |
|---|---|
| `cardea`, `cardea-gui`, `cardea-tray` | `/usr/bin` |
| Daemon unit | `/usr/lib/systemd/user/cardea.service` |
| Menu entry and icon | `/usr/share/applications/cardea.desktop`, `/usr/share/icons/hicolor/scalable/apps/cardea.svg` |

Your user needs sub-UID and sub-GID ranges (`/etc/subuid`, `/etc/subgid`); Debian and Fedora create them for new
users. `cardea doctor` shows the command to add them when they are missing.

## First steps

1. Open **Cardea** from the menu (or run `cardea-gui`).
2. Create a sandbox from a profile (Ctrl+N), or on the command line: `cardea create --profile empty --name Test`.
3. Turn on the tray icon at login in **Settings** if you want it (`cardea tray-autostart on`).

## Checking an installation

`/usr/share/cardea/selftest.sh` checks that sandboxes work on this computer. It builds two throw-away sandboxes
(`selftest-a`, `selftest-b`), checks isolation, the home, shared folders, the firewall, failure handling, apps,
services, packages, the keyring and the tray, deletes them and writes a report (`cardea-selftest-<date>.md`, your
home path shown as `~`). The report is rewritten after each check, so a run that crashes or is stopped still
leaves one, marked incomplete. It takes 10 to 20 minutes; `--quick` skips the image rebuilds, `--claude` also builds a
full Claude sandbox (much longer), `--keep` keeps the test sandboxes.

## Upgrading

Install the new package the same way, then restart the daemon: `systemctl --user restart cardea`. Running
sandboxes and their windows keep running across the restart.

## Removing

```bash
sudo apt remove cardea
```

On Fedora: `sudo dnf remove cardea`.

The daemon's unit is disabled; `podman.socket` stays enabled, as other tools may use it. Sandboxes, their homes
(`~/Sandboxes`), settings (`~/.config/cardea`), logs (`~/.local/share/cardea`), menu entries of sandbox apps
(`~/.local/share/applications/cardea-*.desktop`) and images are kept: delete the sandboxes first
(`cardea delete <id>`) to remove them.

## From a source checkout

Requires a Rust toolchain (1.85 or newer). No root is needed:

```bash
scripts/install-dev.sh
```

It builds the release binaries, installs them in `~/.local/bin` with the daemon's unit in
`~/.config/systemd/user` and the menu entry, then enables `podman.socket` and the daemon. A unit in
`~/.config/systemd/user` takes precedence over the package's: remove it before installing the package.

To build the packages: `scripts/build-deb.sh` (needs `dpkg-deb`) writes `dist/cardea_<version>_<arch>.deb`;
`scripts/build-rpm.sh` (needs `cargo-generate-rpm`) writes `dist/cardea-<version>-1.<arch>.rpm`.
