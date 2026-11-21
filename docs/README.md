# Cardea documentation

Cardea creates, runs and manages isolated desktop sandboxes on Linux. Each sandbox is a rootless Podman
environment built from a profile, with its own home, its own network firewall and only the access to your
desktop that you grant it.

> **Status**: Cardea is in development. These pages describe the behaviour planned for version 1.0 and are
> updated as each feature lands. Installation instructions will be added with the first usable build.

| Page | Content |
|---|---|
| [Overview](overview.md) | What Cardea is made of, and what it needs on your computer |
| [Concepts](concepts.md) | Sandboxes, profiles, trust levels, capabilities, apps, services, packages, lifecycle |
| [Configuration files](configuration.md) | The JSON files, their defaults and what is checked |
| [Commands](commands.md) | The `cardea` command line |
| [The manager window](manager.md) | `cardea-gui`: pages, dialogs, keyboard |
| [Network firewall](network.md) | Zones, allowlists and blocklists, rejected or dropped traffic, warnings |
| [Desktop compatibility](desktop-compatibility.md) | Wayland and X11 sessions, supported desktops, running X11 apps safely |
| [Security model](security.md) | What a sandbox protects you from, and what it does not |
