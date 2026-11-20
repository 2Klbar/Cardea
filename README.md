# Cardea

A Linux desktop tool to create, run and manage isolated Podman sandboxes from profiles, with a native GUI (Rust +
iced), a tray icon and a command line.

Documentation: [`docs/`](docs/README.md). Logo and icons: [`assets/logo/`](assets/logo/README.md).

Status: in development; no release yet.

## Building from source

Requires a Rust toolchain (1.85 or newer).

```sh
cargo build --release     # binaries: cardea, cardea-gui, cardea-tray
scripts/check.sh          # formatting, lints and tests
```

Licence: AGPL-3.0-or-later (see [`LICENSE`](LICENSE)).
