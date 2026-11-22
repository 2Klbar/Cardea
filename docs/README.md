# Cardea documentation

Cardea creates, runs and manages isolated desktop sandboxes on Linux. Each sandbox is a rootless Podman
environment built from a profile, with its own home, its own network firewall and only the access to your
desktop that you grant it.

These pages describe Cardea 1.0. Start with [Installation](install.md).

| Page | Content |
|---|---|
| [Installation](install.md) | The Debian package, first steps, upgrading, removing, installing from source |
| [Overview](overview.md) | What Cardea is made of, and what it needs on your computer |
| [Concepts](concepts.md) | Sandboxes, profiles, trust levels, capabilities, apps, services, packages, lifecycle |
| [Configuration files](configuration.md) | The JSON files, their defaults and what is checked |
| [Commands](commands.md) | The `cardea` command line |
| [The manager window](manager.md) | `cardea-gui`: pages, dialogs, keyboard |
| [Tray and login](tray.md) | The tray icon, its menu and notifications; what starts at login |
| [Network firewall](network.md) | Zones, allowlists and blocklists, rejected or dropped traffic, warnings |
| [Links](links.md) | Links that open in a sandbox's app, and links a sandbox sends to your browser |
| [Desktop compatibility](desktop-compatibility.md) | Wayland and X11 sessions, supported desktops, running X11 apps safely |
| [Security model](security.md) | What a sandbox protects you from, and what it does not |
| [Architecture](architecture.md) | For contributors: crates, processes, protocol, engine, files on disk |
