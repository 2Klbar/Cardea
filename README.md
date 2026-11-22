# Cardea

Isolated desktop sandboxes for Linux, on rootless Podman. Each sandbox is built from a profile and gets its own
home, its own firewall and only the access to your desktop that you grant it. Its apps show up as normal windows
and menu entries.

Use it for an AI coding agent, a CAD tool, a browser for untrusted sites, or an unknown program you want to try.

- **Manager window** (`cardea-gui`), **tray icon** (`cardea-tray`) and **command line** (`cardea`), all driven by
  one background daemon.
- **Profiles**: declarative JSON recipes (base image, packages, apps, services, capabilities, firewall), shareable
  and free of host paths or secrets.
- **Trust levels** that set safe defaults; risky settings stay visible as warnings.
- **Per-sandbox firewall**, on by default; the app never starts before the rules are applied.
- **Shared folders** with protected paths, **links** that open in the right place, a **Wayland filter** and
  isolated X11 apps.
- No root needed, and your system firewall is never touched.

## Quick start

1. **Install** the package for your system (amd64 / x86_64), from the
   [releases](https://github.com/2Klbar/Cardea/releases):

   | System | Command |
   |---|---|
   | Debian 13 or later | `sudo apt install ./cardea_1.0.0_amd64.deb` |
   | Fedora 41 or later | `sudo dnf install ./cardea-1.0.0-1.x86_64.rpm` |

2. **Log out and back in** once: the daemon and Podman's socket then start with your session.
3. **Check** the computer: `cardea doctor` says what is missing and how to fix it.
4. **Create a sandbox**: open **Cardea** from the menu and press Ctrl+N, or on the command line:

   ```bash
   cardea create --profile empty --name Test
   ```

   ```bash
   cardea start test
   ```

   ```bash
   cardea exec test
   ```

5. **Try a ready-made profile**: download one from [`profiles/`](profiles/) and import it with
   `cardea profile import <file>` (or **Import a profile…** in the manager's new-sandbox wizard).

More: [Installation](docs/install.md), [Concepts](docs/concepts.md), [Commands](docs/commands.md).

## Documentation

| Page | For |
|---|---|
| [Overview](docs/overview.md), [Concepts](docs/concepts.md) | What Cardea is and the words it uses |
| [Manager window](docs/manager.md), [Tray](docs/tray.md), [Commands](docs/commands.md) | Using it day to day |
| [Configuration files](docs/configuration.md), [Network firewall](docs/network.md), [Links](docs/links.md) | Tuning a sandbox |
| [Desktop compatibility](docs/desktop-compatibility.md), [Security model](docs/security.md) | What works where, and what a sandbox protects |
| [Architecture](docs/architecture.md) | Reading or changing the code |

The full index is [`docs/README.md`](docs/README.md).

## Working on the code

Requires Linux and a Rust toolchain (1.85 or newer); running sandboxes also needs rootless Podman 4.4 or later.
Start with [Architecture](docs/architecture.md): crates, processes, control protocol, engine.

| Path | Content |
|---|---|
| `crates/core` | Data model, validation, generated Containerfiles, container specs and firewall rules (pure code) |
| `crates/proto` | Control socket protocol |
| `crates/engine` | Podman engine and its REST client |
| `crates/daemon` | The daemon: state, tasks, the only part that talks to Podman |
| `crates/cli`, `crates/gui`, `crates/tray` | The three clients |
| `profiles/` | Ready-made profiles |
| `docs/` | User and architecture documentation |
| `assets/` | Logo, icons, desktop entry, systemd unit, package scripts |
| `scripts/` | Build, check, install and test scripts |

```bash
scripts/check.sh
```

Formatting, clippy (warnings are errors) and tests: it must pass before every commit.

| Command | Does |
|---|---|
| `cargo build --release` | Builds `cardea`, `cardea-gui` and `cardea-tray` |
| `scripts/install-dev.sh` | Installs this checkout for your user (`~/.local/bin`, user unit, menu entry) |
| `scripts/build-deb.sh` | `dist/cardea_<version>_<arch>.deb` (needs `dpkg-deb`) |
| `scripts/build-rpm.sh` | `dist/cardea-<version>-1.<arch>.rpm` (needs `cargo-generate-rpm`) |
| `scripts/selftest.sh` | End-to-end test on a real install; writes a Markdown report |

Rules the code keeps: only the daemon talks to Podman; neither its socket nor Podman's is ever visible in a
sandbox; configuration is JSON validated by the daemon, never executed; no code path is specific to one app.

## Licence

AGPL-3.0-or-later (see [`LICENSE`](LICENSE)). Logo and icons: [`assets/logo/`](assets/logo/README.md).
