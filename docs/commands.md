# Commands

`cardea` is the command line. It talks to the Cardea daemon, which does the work. The daemon normally runs as a
systemd user unit ([Installation](install.md)); if none answers, `cardea` starts one in the background.

## Sandboxes

| Command | What it does |
|---|---|
| `cardea list` | Every sandbox with its state, current task and number of warnings |
| `cardea status <id>` | One sandbox: state, folder, trust level, home, shared folders, capabilities, firewall, apps, services, warnings |
| `cardea create --profile <profile> --name <name> [--location <dir>]` | Create a sandbox from a profile; its id is made from the name (`My Box` → `my-box`); its folder goes in `<dir>` (default: the settings' location, `~/Sandboxes`) |
| `cardea create --from-config <file>` | Create a sandbox from a complete `sandbox.json` |
| `cardea start <id>` | Start it |
| `cardea stop <id>` | Stop it; on a sandbox in error, reset it to stopped |
| `cardea restart <id>` | Stop then start it |
| `cardea rebuild <id> [--fresh]` | Rebuild its image; a running sandbox is restarted afterwards. `--fresh` pulls the base image again and runs every step: the image's packages are updated |
| `cardea delete <id> [--keep-home] [-y]` | Delete it with its folder, or keep the folder with `--keep-home`; asks first unless `-y` |
| `cardea edit <id>` | Edit its `sandbox.json` (see below) |
| `cardea logs <id> [-f]` | Its log; `-f` keeps showing new lines |
| `cardea net <id> …` | Its firewall: see [Network firewall](network.md) |
| `cardea app <id>` | Its apps |
| `cardea app <id> <app> [-- <args>…]` | Launch an app, starting the sandbox when needed (a terminal app runs in this terminal) |
| `cardea app <id> --found` / `--add <app>` | The apps installed in the running sandbox / add one |
| `cardea app <id> <app> --test-wayland` | Run an app once without X server (see [Desktop compatibility](desktop-compatibility.md)) |
| `cardea app <id> <app> --url <link>` | Open a link of one of the app's schemes in it (what its menu entry runs when you click such a link); see [Links](links.md) |
| `cardea links <id> …` | Its links: see [Links](links.md) |
| `cardea service <id> …` | Its background services (below) |
| `cardea exec <id> [--root] [-w <dir>] [-- <command>…]` | A shell (your login shell in the sandbox), or the command, in a running sandbox |

`start`, `restart` and `rebuild` ask for the keyring password in the terminal when the sandbox's keyring asks for
it at each start.

`create`, `start`, `stop`, `restart` and `rebuild` show the progress of the task until it ends
(`Step 3/6 · system packages · 42 %`). Pressing Ctrl+C only stops watching: the task goes on.

A sandbox does one task at a time. What each state allows:

| State | Allowed |
|---|---|
| Stopped | start, rebuild, delete, edit |
| Running | stop, restart, rebuild, edit |
| Build failed | rebuild (retry), delete, edit |
| Error | start (retry), stop (reset), delete, edit |
| Building, Starting, Stopping, Rebuilding | wait for the task to end |

## Running commands

`cardea exec <id>` opens a shell in a running sandbox, as the sandbox user, in its home. `cardea exec <id> -- ls -la`
runs a command instead; `cardea` exits with the command's exit code.

| Option | Effect |
|---|---|
| `--root` | Run as root inside the sandbox, with a few capabilities for package maintenance (`apt install …`). Changes outside the home are lost at the next start; add packages to the configuration to keep them |
| `-w <dir>` | Start in this folder inside the sandbox |
| `--app <app>` | Run this app of the sandbox, with its toolkit's settings, instead of a command |

In a terminal, the command gets a terminal of its own: full-screen programs work and follow the window size.
Without a terminal (`cardea exec box -- cat file > copy`), input and output are passed through as they are, with
errors on stderr.

## Packages

| Command | What it does |
|---|---|
| `cardea packages <id> list` | The packages, where they install (image or home) and the version installed in the running sandbox |
| `cardea packages <id> add <source> <name> [--version <v>] [--when <capability>]` | Add a package: a home package installs at once in a running sandbox, an image package at the next rebuild |
| `cardea packages <id> remove <source> <name>` | Remove it (uninstalled at once from the home of a running sandbox) |
| `cardea packages <id> update [<source> [<name>]]` | Update the home packages: all, one source's, or one |
| `cardea packages <id> update --image` | Update the image's packages: the same as `rebuild --fresh` |
| `cardea sources <id> list` | The sources it can use |
| `cardea sources <id> add <file>` / `remove <source>` | Add a custom source from a JSON file, or remove one |
| `cardea repo <id> list` | Its third-party apt repositories |
| `cardea repo <id> add <name> --url <https://…> --suite <s> [--components main,…] --key-url <https://…> --fingerprint <40 hex>` | Add an apt repository (applies at the next rebuild; apt-based images only) |
| `cardea repo <id> remove <name>` | Remove one |

A custom source file:

```json
{ "id": "gem", "name": "Ruby gems", "scope": "home",
  "setup": "",
  "install": "gem install --user-install \"$1\" ${2:+--version \"$2\"}",
  "remove": "gem uninstall -x \"$1\"",
  "update": "gem update --user-install \"$1\"",
  "list": "gem list --local | sed 's/ (\\(.*\\))$/ \\1/'" }
```

Each command runs inside the sandbox as `sh -c '<command>' sh <name> <version>`: in the command, `$1` is the
package name and `$2` its version (empty when none is given). `list` prints one `name version` line per package.

## Background services

| Command | What it does |
|---|---|
| `cardea service <id> list` | The services and their instances: running, stopped, or exited with its code |
| `cardea service <id> add <service> [--name <n>] [--per-folder] -- <command>…` | Declare a service; with `--per-folder`, one instance per shared folder, running in it, and `{folder}` in the command becomes the folder's name |
| `cardea service <id> remove <service>` | Remove it from the configuration |
| `cardea service <id> start\|stop <service> [<folder>]` | Start (again) or stop an instance |
| `cardea service <id> attach <service> [<folder>]` | Open an instance in this terminal (Ctrl+B then D to detach) |
| `cardea service <id> autostart <service> on\|off [<folder>]` | Start an instance with the sandbox, or not |

## Shared folders

A shared folder is a folder of your computer mounted into a sandbox.

| Command | What it does |
|---|---|
| `cardea folders <id> list` | The shared folders, with their access, the missing ones, and the protected paths |
| `cardea folders <id> add <folder> [--mount <path>] [--ro\|--rw]` | Share a folder. It appears in `/work/<folder name>` unless `--mount` says otherwise. Read-write by default, read-only by default in an Untrusted sandbox |
| `cardea folders <id> remove <folder>` | Stop sharing it; give its path in the sandbox, its name or its path on your computer |

Changes apply at the next start (a running sandbox shows **Restart required**).

- **Protected paths**: inside a read-write folder, paths that can make your computer run code later
  (`.git/hooks`, `.git/config`, `.vscode`, `.envrc`, `.husky`, `.cargo/config.toml`, `.devcontainer`, `.obsidian`)
  are mounted read-only. They are looked for at each start, down to 5 folders deep (`node_modules`, `target`,
  `.venv` and `vendor` are skipped). One created while the sandbox runs is protected from the next start. The list
  and depth are in `protected_paths` of `sandbox.json`.
- **Missing folder**: a shared folder that does not exist is skipped at start, with a line in the log.
- **One home, one sandbox**: a sandbox does not start while another container uses its home folder.

## Editing a sandbox

`cardea edit <id>` opens a copy of the sandbox's `sandbox.json` in your editor (`$VISUAL`, `$EDITOR`, or
`nano`). When you close it, Cardea checks the file ([Configuration files](configuration.md)):

- if it has problems, they are listed and you can edit again; nothing is saved until the file is valid;
- otherwise it is saved, and Cardea tells you whether the change applies now, needs a restart, or needs a
  rebuild. The sandbox then shows **Restart required** or **Rebuild required**.

| Change | Needs |
|---|---|
| Name, icon, firewall, apps, start at login, home packages and sources, asking before links and allowed hosts | Nothing: applies at once |
| Shared folders, home, identity, security settings, trust level, devices (KVM…), clipboard, outgoing links on or off | A restart |
| Base image, image packages and sources, repositories, capabilities that install software (GPU, keyring, isolated X11), languages and theme, build script | A rebuild |

The id of a sandbox cannot be changed.

## Profiles and files

| Command | What it does |
|---|---|
| `cardea profile list` | Built-in profiles and yours (`~/.config/cardea/profiles/`) |
| `cardea profile show <id>` | A profile in full, every default filled in |
| `cardea profile import <file> [--replace] [-y]` | Show what a profile file asks for (items that widen access marked `!`, scripts and custom sources in full), then save it among yours. Profiles never hold host paths, IP addresses or local settings; those are refused |
| `cardea profile export <id> <file>` | Write a profile to a file (`-`: the standard output) |
| `cardea profile save <sandbox> <profile> [--name <n>]` | Save a sandbox's configuration as one of your profiles, without its shared folders, home path, hostname, start at login and IP addresses |
| `cardea profile delete <id>` | Delete one of your profiles (built-in ones cannot be deleted) |
| `cardea check <sandbox\|profile\|settings\|source> <file>` | Check a file and list every problem in it |

## Host check

`cardea doctor` checks that this computer can run sandboxes and prints one line per check, with what to do
under each one that does not pass. It exits with an error when a check fails.

| Check | Passes when |
|---|---|
| Session | Your desktop session is Wayland (on X11 it lists the ways out, see [Desktop compatibility](desktop-compatibility.md)) |
| Tray host | Your desktop shows tray icons (StatusNotifierItem); only a warning otherwise |
| Podman socket | Podman answers on `$XDG_RUNTIME_DIR/podman/podman.sock` (`systemctl --user enable --now podman.socket`) |
| Podman version | 4.4 or later |
| Sub-UIDs | At least 65536 sub-UIDs and sub-GIDs for your user |
| passt | Podman finds `pasta`, its network helper |
| Podman events | Podman streams its events |

When Podman does not answer, only the socket check is shown.

## Daemon

| Command | What it does |
|---|---|
| `cardea gui` | Open the manager window |
| `cardea tray-autostart on\|off` | Start the tray icon at login, or not |
| `cardea daemon` | Run the daemon in the foreground |

Only one daemon runs at a time: when the `cardea` user unit is installed, a client or the tray that finds no daemon
starts the unit rather than a daemon of its own. After an update, restart it: an older daemon refuses newer commands with a
message saying so. It accepts connections only from your own user. Sandbox logs are kept in
`~/.local/share/cardea/logs/<id>/sandbox.log`.
