# Commands

`cardea` is the command line. It talks to the Cardea daemon, which does the work; if the daemon is not running,
`cardea` starts it in the background.

> Until the Podman engine is available, run the daemon in demo mode (below) to try the commands on simulated
> sandboxes.

## Sandboxes

| Command | What it does |
|---|---|
| `cardea list` | Every sandbox with its state, current task and number of warnings |
| `cardea status <id>` | One sandbox: state, folder, trust level, home, shared folders, capabilities, firewall, apps, services, warnings |
| `cardea create --profile <profile> --name <name>` | Create a sandbox from a profile; its id is made from the name (`My Box` → `my-box`) |
| `cardea create --from-config <file>` | Create a sandbox from a complete `sandbox.json` |
| `cardea start <id>` | Start it |
| `cardea stop <id>` | Stop it; on a sandbox in error, reset it to stopped |
| `cardea restart <id>` | Stop then start it |
| `cardea rebuild <id>` | Rebuild its image; a running sandbox is restarted afterwards |
| `cardea delete <id> [--keep-home] [-y]` | Delete it with its folder, or keep the folder with `--keep-home`; asks first unless `-y` |
| `cardea edit <id>` | Edit its `sandbox.json` (see below) |
| `cardea logs <id> [-f]` | Its log; `-f` keeps showing new lines |

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

## Editing a sandbox

`cardea edit <id>` opens a copy of the sandbox's `sandbox.json` in your editor (`$VISUAL`, `$EDITOR`, or
`nano`). When you close it, Cardea checks the file ([Configuration files](configuration.md)):

- if it has problems, they are listed and you can edit again; nothing is saved until the file is valid;
- otherwise it is saved, and Cardea tells you whether the change applies now, needs a restart, or needs a
  rebuild. The sandbox then shows **Restart required** or **Rebuild required**.

| Change | Needs |
|---|---|
| Name, icon, firewall, apps, start at login | Nothing: applies at once |
| Shared folders, home, identity, security settings, trust level | A restart |
| Base image, packages, sources, repositories, capabilities, languages and theme, build script | A rebuild |

The id of a sandbox cannot be changed.

## Profiles and files

| Command | What it does |
|---|---|
| `cardea profile list` | Built-in profiles and yours (`~/.config/cardea/profiles/`) |
| `cardea profile show <id>` | A profile in full, every default filled in |
| `cardea check <sandbox\|profile\|settings\|source> <file>` | Check a file and list every problem in it |

## Daemon

| Command | What it does |
|---|---|
| `cardea daemon` | Run the daemon in the foreground |
| `cardea daemon --demo` | Run it with simulated sandboxes. Nothing touches Podman, your settings or your real sandboxes: the demo keeps its files in a temporary folder, reset at each start |

Only one daemon runs at a time. It accepts connections only from your own user. Sandbox logs are kept in
`~/.local/share/cardea/logs/<id>/sandbox.log`.
