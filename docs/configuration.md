# Configuration files

Cardea's configuration is plain JSON. It is read and checked, never executed. Every file carries
`"schema": 1`, the version of its format.

## Files

| File | Content |
|---|---|
| `<sandbox folder>/sandbox.json` | One sandbox: image, packages, apps, services, capabilities, firewall, shared folders |
| `~/.config/cardea/profiles/<id>.json` | A profile: same format as a sandbox, without the local settings |
| `~/.config/cardea/settings.json` | Tool settings |

## Sandboxes and profiles

Only `schema`, `id` and `name` are required. Every other setting takes its default, and the defaults follow the
trust level: in a file with `"trust": "untrusted"`, a missing `home.disposable` means a disposable home. A
setting you write wins over the default.

```json
{ "schema": 1, "id": "scratch", "name": "Scratch", "trust": "untrusted" }
```

A profile cannot contain the local settings of a sandbox:

| Not in a profile | Why |
|---|---|
| `folders`, `home.path` | Paths of your computer |
| IP addresses in firewall entries | Only references: `host`, `gateway`, `dns` |
| `identity.hostname`, `autostart`, services' `autostart`, `profile` | Local choices |

To see a built-in profile in full, with every default filled in:

```sh
cardea profile list
cardea profile show claude
```

## What is checked

| Value | Accepted |
|---|---|
| Ids (sandbox, profile, app, service, source, repository) | 1 to 32 lowercase letters, digits and `-`, not starting or ending with `-` |
| Host paths | Absolute and clean: no `.`, `..`, `//`, `:` or trailing `/` |
| Mount points | As host paths, and never on, inside or above a system folder (`/usr`, `/etc`, `/bin`, `/lib`, `/proc`, `/sys`, `/dev`); never two folders on the same point; never over the home |
| Base image | Full reference with its registry, such as `docker.io/library/debian:trixie-slim` |
| Third-party apt repositories | `https://` addresses and the key's 40-character fingerprint |
| Package names and versions | Letters, digits and a few punctuation marks; never starting with `-` |
| Commands (`exec`) | A list of arguments, never a shell line; `{folder}` only as a whole argument of a per-folder service |
| Apps needing X11 | `capabilities.x11` must be `isolated` or `host` |

Unknown fields are errors, so a typo never goes unnoticed.

## Checking a file

`cardea check` reads a file and lists every problem at once, with the field it concerns:

```sh
cardea check sandbox ~/Sandboxes/claude/sandbox.json
```

```text
error: sandbox.json is not a valid sandbox config:
  - folders[0].mount: "/usr/local" is, contains or is inside the system folder /usr; mount points cannot touch it
  - apt_repos[0].url: "http://x.org" must be an https:// address
```

The kinds of file are `sandbox`, `profile`, `settings` and `source`.

## Settings

| Field | Default | Meaning |
|---|---|---|
| `sandbox_location` | `null` (`~/Sandboxes`) | Where new sandboxes go |
| `tray_at_login` | `true` | Start the tray icon with your session |
| `theme` | `system` | `system`, `light` or `dark` |
| `terminal` | `null` (detected) | Your terminal command, such as `["konsole", "-e"]` |
