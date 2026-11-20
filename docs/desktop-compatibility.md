# Desktop compatibility

## Wayland session required

Cardea shows sandbox windows through **Wayland**: a sandbox receives only the Wayland socket, never your whole
desktop session. This works on any desktop running a Wayland session.

`cardea doctor` and the manager's welcome screen report your session type.

## If your session is X11

On an X11 session there is no Wayland socket: sandbox windows can only be shown through your desktop's X server,
which lets sandbox apps read every keystroke and the content of every window of your desktop. Cardea allows it
only as a risky setting, with a strong warning. The ways out, in order:

| Option | What to do | Notes |
|---|---|---|
| 1. Use your desktop's Wayland session | Log out, choose the Wayland session in the login screen's session menu | **Cinnamon**: "Cinnamon on Wayland" exists but is marked experimental: check stability, scaling and multiple screens. **Xfce**, **MATE**: no complete Wayland session yet |
| 2. Switch to a desktop that runs on Wayland | Install it, then choose it in the session menu (your current desktop can stay installed) | See the table below |
| 3. Stay on X11 | Allow "X11: host" for each sandbox that needs windows | Not recommended |

Desktops with a mature Wayland session:

| Desktop | Style | Debian package | Notes |
|---|---|---|---|
| **KDE Plasma 6** | Classic, very configurable | `task-kde-desktop` | Wayland by default; Cardea's reference desktop |
| **GNOME** | Modern, minimal | `task-gnome-desktop` | Wayland by default; tray icons need the AppIndicator extension |
| **COSMIC** | Classic with tiling | — | Wayland only, young |
| **Sway**, **Hyprland**, **niri** | Tiling, keyboard-driven | `sway` | Tray icons through the panel (e.g. waybar) |
| **LXQt 2** | Light, classic | `task-lxqt-desktop` | Wayland session still maturing |

## X11 apps in a sandbox

Some apps only speak X11. Any X11 client can read the keystrokes and screen content of the other clients of the
same X server, so Cardea gives each sandbox a choice:

| X11 setting | How it works | What an X11 app can see |
|---|---|---|
| **Off** | No X server | — |
| **Isolated** (default when an app needs X11) | A private X server runs inside the sandbox (xwayland-satellite); its windows appear as normal windows on your desktop | Only the other X11 apps of the same sandbox |
| **Host** (risky) | The sandbox uses your desktop's X server | Every X11 app of your desktop, or your whole desktop on an X11 session |

Limits of **Isolated**: no shared clipboard with X11 apps outside the sandbox (the Wayland clipboard works), and
apps that expect a full X11 desktop (tray icons, global shortcuts) may misbehave. It needs a Wayland session.

## Running an app on Wayland first

Many apps described as "X11" can run on Wayland with the right setting. Try this before enabling X11:

| App made with | Try |
|---|---|
| Qt 5 / 6 | App toolkit **Qt** (`QT_QPA_PLATFORM=wayland`) |
| GTK 3 / 4 | App toolkit **GTK** (`GDK_BACKEND=wayland`) |
| Electron, Chromium | App toolkit **Electron** (`--ozone-platform=wayland`) |
| Firefox, Thunderbird | Environment variable `MOZ_ENABLE_WAYLAND=1` (default in recent versions) |
| SDL 2 / 3 (games) | Environment variable `SDL_VIDEODRIVER=wayland` |
| GLFW, winit, Flutter | Usually Wayland already |
| Java (Swing, JavaFX) | X11 only (an experimental Wayland toolkit exists on recent JDKs): use **Isolated** |
| Wine, Motif, Tk, plain Xlib | X11 only: use **Isolated** |

**Test on Wayland** (`cardea app <sandbox> <app> --test-wayland`) launches the app once without any X server. If
a window appears, the app works on Wayland and Cardea offers to save the matching toolkit.
