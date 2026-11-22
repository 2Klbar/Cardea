# Cardea

A Linux desktop tool to create, run and manage isolated Podman sandboxes from profiles, with a native GUI (Rust +
iced), a tray icon and a command line.

Documentation: [`docs/`](docs/README.md). Logo and icons: [`assets/logo/`](assets/logo/README.md).

Version 1.0: see [Installation](docs/install.md).

## Building from source

Requires a Rust toolchain (1.85 or newer). How the code is organised: [Architecture](docs/architecture.md).

```sh
cargo build --release     # binaries: cardea, cardea-gui, cardea-tray
scripts/check.sh          # formatting, lints and tests
scripts/build-deb.sh      # dist/cardea_<version>_<arch>.deb
scripts/build-rpm.sh      # dist/cardea-<version>-1.<arch>.rpm
```

Licence: AGPL-3.0-or-later (see [`LICENSE`](LICENSE)).
