# Tray and login

## The tray icon

`cardea-tray` shows Cardea in your desktop's tray (StatusNotifierItem).

| Icon | Means |
|---|---|
| Outlined, dimmed | No sandbox running (some trays hide it in their expander) |
| Green ✓ badge | At least one sandbox runs |
| Blue arrow badge | A build, start or stop is in progress |
| Red ! badge | A sandbox is in error, a service stopped on its own, or the daemon does not answer; the icon asks for attention so it leaves the expander |

The tooltip lists what runs. A left click opens the manager.

## The menu

With several sandboxes, each has its own submenu; with a single one, its entries come straight in the menu.

| Entry | What it does |
|---|---|
| ! warnings | The sandbox's risky settings and stopped services (not clickable) |
| Pinned apps | Open the app; a terminal app opens in a terminal |
| Services | Each instance with its state, and Start / Restart, Stop, Attach (in a terminal) |
| Open details | The sandbox's page in the manager |
| Open a terminal | A shell in the sandbox |
| Start / Stop | Start or stop the sandbox |
| Open manager | The manager window |
| Quit tray | Closes the tray only: the sandboxes keep running |

A sandbox whose keyring asks for its password is started from the manager: the tray then shows a notification
that opens it.

## Notifications

| When | Buttons |
|---|---|
| A service stops on its own | Attach, Restart |
| A sandbox stops on its own, or its firewall container does | Restart, Details |
| A build, start or other task fails | Details |
| A start from the tray needs the keyring password | Open |

## At login

| What | How |
|---|---|
| The daemon | A systemd user unit, `cardea.service`, started with the graphical session. When a client (tray, manager, `cardea`) finds no daemon, it starts this unit; without a systemd user session, it starts the daemon itself |
| The tray | `~/.config/autostart/cardea-tray.desktop`, written and removed with the **Tray at login** setting (`cardea tray-autostart on\|off`) |
| Sandboxes | Those with **Start at login** (Advanced page, Settings page) start with the daemon, except those whose keyring asks for its password |

| Desktop | Tray host |
|---|---|
| KDE Plasma | Built in |
| GNOME | The AppIndicator extension (`gnome-shell-extension-appindicator`) |
| Cinnamon | `xapp-sn-watcher` (package `xapp`), then log in again |

`cardea doctor` says whether your desktop has a tray host.

## Installing

The package enables the daemon for every user; see [Installation](install.md), which also covers installing from
a source checkout.
