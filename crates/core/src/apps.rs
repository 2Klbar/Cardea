//! Apps: how an app is launched for its toolkit, apps found in a sandbox's `.desktop` files, and the host menu
//! entries that launch them.

use std::path::Path;

use crate::model::{App, Capability, Config, KeyringMode, Theme, Toolkit};

/// What to execute in the app container.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub argv: Vec<String>,
    /// `NAME=value`, added to the container's environment.
    pub env: Vec<String>,
}

/// The app's argv and environment for its toolkit (the container already has the display, bus and runtime
/// folder variables).
pub fn launch(c: &Config, app: &App, args: &[String]) -> Launch {
    let mut env: Vec<String> = Vec::new();
    let mut argv: Vec<String> = app.exec.iter().take(1).cloned().collect();
    match app.toolkit {
        Toolkit::Qt => env.push("QT_QPA_PLATFORM=wayland".into()),
        Toolkit::Gtk => {
            env.push("GDK_BACKEND=wayland".into());
            env.push("GTK_DECORATION_LAYOUT=:minimize,maximize,close".into());
        }
        Toolkit::Electron => {
            argv.push("--ozone-platform=wayland".into());
            argv.push("--no-sandbox".into());
            if c.capabilities.keyring.mode != KeyringMode::Off {
                argv.push("--password-store=gnome-libsecret".into());
            }
            if !c.capabilities.gpu {
                argv.push("--disable-gpu".into());
            }
            // Electron learns the dark preference from the desktop portal, absent here; GTK_THEME is not enough.
            if c.appearance.theme == Theme::Dark {
                argv.push("--force-dark-mode".into());
            }
        }
        Toolkit::Terminal | Toolkit::X11 | Toolkit::Other => {}
    }
    argv.extend(app.exec.iter().skip(1).cloned());
    argv.extend(args.iter().cloned());
    env.extend(app.env.iter().map(|(k, v)| format!("{k}={v}")));
    Launch { argv, env }
}

/// "Test on Wayland": the app once, with no X server and every toolkit told to use Wayland.
pub fn test_wayland(app: &App, args: &[String]) -> Launch {
    let env = [
        "DISPLAY=",
        "QT_QPA_PLATFORM=wayland",
        "GDK_BACKEND=wayland",
        "SDL_VIDEODRIVER=wayland",
        "MOZ_ENABLE_WAYLAND=1",
        "ELECTRON_OZONE_PLATFORM_HINT=wayland",
    ];
    let argv = app.exec.iter().chain(args).cloned().collect();
    Launch {
        argv,
        env: env.map(String::from).to_vec(),
    }
}

/// The apps that request a capability, by name (Wayland: every graphical app).
pub fn requested_by(c: &Config, cap: Capability) -> Vec<&str> {
    c.apps
        .iter()
        .filter(|a| {
            a.wants.contains(&cap)
                || match cap {
                    Capability::Wayland => !matches!(a.toolkit, Toolkit::Terminal | Toolkit::X11),
                    Capability::X11 => a.toolkit == Toolkit::X11,
                    Capability::Keyring => a.toolkit == Toolkit::Electron,
                    _ => false,
                }
        })
        .map(|a| a.name.as_str())
        .collect()
}

/// An app found in a `.desktop` file of the sandbox.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Detected {
    pub id: String,
    pub name: String,
    pub exec: Vec<String>,
    pub toolkit: Toolkit,
    pub icon: Option<String>,
    pub wm_class: Option<String>,
}

impl Detected {
    pub fn app(&self) -> App {
        App {
            id: self.id.clone(),
            name: self.name.clone(),
            exec: self.exec.clone(),
            toolkit: self.toolkit,
            wants: Vec::new(),
            env: Default::default(),
            pinned: false,
            desktop_entry: true,
            icon: self.icon.clone(),
            wm_class: self.wm_class.clone(),
            schemes: Vec::new(),
        }
    }
}

/// Fills an app's missing icon and window class from the `.desktop` file found in the sandbox for the same program,
/// so the desktop shows the app's own icon for its windows instead of the process that draws them.
pub fn complete(app: &mut App, found: &[Detected]) {
    let base = |argv: &[String]| {
        argv.first()
            .map(|p| p.rsplit('/').next().unwrap_or(p).to_owned())
    };
    let program = base(&app.exec);
    let Some(d) = found
        .iter()
        .find(|d| program.is_some() && base(&d.exec) == program)
    else {
        return;
    };
    if app.icon.is_none() {
        app.icon = d.icon.clone();
    }
    if app.wm_class.is_none() {
        app.wm_class = d.wm_class.clone();
    }
}

/// Reads the `[Desktop Entry]` group of a `.desktop` file; `None` for hidden entries and non-applications.
pub fn parse_desktop(file_name: &str, text: &str) -> Option<Detected> {
    let mut in_entry = false;
    let mut get = std::collections::HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
        } else if in_entry && let Some((k, v)) = line.split_once('=') {
            // Localised keys (`Name[fr]`) are left out: the plain one is the fallback every file has.
            get.entry(k.trim()).or_insert_with(|| unescape(v.trim()));
        }
    }
    let flag = |k: &str| get.get(k).is_some_and(|v| v == "true");
    if get.get("Type").is_some_and(|t| t != "Application") || flag("NoDisplay") || flag("Hidden") {
        return None;
    }
    let exec = exec_argv(get.get("Exec")?);
    let categories = get.get("Categories").map(String::as_str).unwrap_or("");
    let toolkit = if flag("Terminal") {
        Toolkit::Terminal
    } else if categories.split(';').any(|c| c == "Qt" || c == "KDE") {
        Toolkit::Qt
    } else if categories.split(';').any(|c| c == "GTK" || c == "GNOME") {
        Toolkit::Gtk
    } else {
        Toolkit::Other
    };
    let id = slug(
        file_name
            .strip_suffix(".desktop")
            .unwrap_or(file_name)
            .rsplit('.')
            .next()
            .unwrap_or(""),
    );
    (!exec.is_empty() && !id.is_empty()).then(|| Detected {
        id,
        name: get
            .get("Name")
            .cloned()
            .unwrap_or_else(|| file_name.to_owned()),
        exec,
        toolkit,
        icon: get.get("Icon").cloned().filter(|i| !i.is_empty()),
        // Without StartupWMClass, toolkits (Electron, GTK, Qt) use the file's name as the Wayland app ID.
        wm_class: get
            .get("StartupWMClass")
            .cloned()
            .filter(|w| !w.is_empty())
            .or_else(|| file_name.strip_suffix(".desktop").map(str::to_owned)),
    })
}

/// `org.mozilla.firefox` gives `firefox`; anything else than `[a-z0-9-]` becomes `-`.
fn slug(s: &str) -> String {
    let mut out = String::new();
    for ch in s.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.truncate(32);
    out.trim_end_matches('-').to_owned()
}

/// The string escapes of a `.desktop` value.
fn unescape(v: &str) -> String {
    let mut out = String::new();
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(o) => {
                // Kept for the Exec quoting rules (`\"`, `\$`…).
                out.push('\\');
                out.push(o);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// The arguments of an `Exec` value: quoting resolved, field codes (`%U`, `%f`…) left out.
pub fn exec_argv(exec: &str) -> Vec<String> {
    let mut args = Vec::new();
    let (mut cur, mut quoted, mut started) = (String::new(), false, false);
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            '\\' if quoted => cur.extend(chars.next()),
            ' ' | '\t' if !quoted => {
                if started {
                    args.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            _ => {
                cur.push(c);
                started = true;
            }
        }
    }
    if started {
        args.push(cur);
    }
    args.into_iter()
        .filter(|a| !(a.len() == 2 && a.starts_with('%') && a != "%%"))
        .map(|a| a.replace("%%", "%"))
        .collect()
}

/// File name of an app's host menu entry.
pub fn menu_file(sandbox: &str, app: &str) -> String {
    format!("cardea-{sandbox}-{app}.desktop")
}

/// The host menu entry of an app: "App (Sandbox)", launched by `cardea app <id> <app>`.
pub fn menu_entry(c: &Config, app: &App, cardea: &Path, icon: Option<&Path>) -> String {
    let clean = |s: &str| s.chars().filter(|ch| !ch.is_control()).collect::<String>();
    let quoted = clean(&cardea.display().to_string())
        .replace('\\', "\\\\\\\\")
        .replace('"', "\\\\\"");
    // Schemes are validated (`[a-z][a-z0-9+.-]*`): nothing to escape. The URL goes in its own option (R4).
    let url = if app.schemes.is_empty() {
        ""
    } else {
        " --url %u"
    };
    let mut out = format!(
        "[Desktop Entry]\nType=Application\nName={} ({})\nExec=\"{quoted}\" app {} {}{url}\nX-Cardea-Sandbox={}\n",
        clean(&app.name),
        clean(&c.name),
        c.id,
        app.id,
        c.id
    );
    if app.toolkit == Toolkit::Terminal {
        out += "Terminal=false\n";
    }
    if !app.schemes.is_empty() {
        out += "MimeType=";
        for s in &app.schemes {
            out += &format!("x-scheme-handler/{s};");
        }
        out += "\n";
    }
    if !app.desktop_entry {
        // There only to open its links.
        out += "NoDisplay=true\n";
    }
    if let Some(i) = icon {
        out += &format!("Icon={}\n", clean(&i.display().to_string()));
    }
    if let Some(w) = &app.wm_class {
        out += &format!("StartupWMClass={}\n", clean(w));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin;

    fn app(toolkit: Toolkit, exec: &[&str]) -> App {
        Detected {
            id: "a".into(),
            name: "A".into(),
            exec: exec.iter().map(|s| (*s).to_owned()).collect(),
            toolkit,
            icon: None,
            wm_class: None,
        }
        .app()
    }

    #[test]
    fn menu_entry_with_schemes() {
        let c = builtin::profile("empty").unwrap();
        let mut a = app(Toolkit::Electron, &["e"]);
        let plain = menu_entry(&c, &a, Path::new("/usr/bin/cardea"), None);
        assert!(!plain.contains("--url") && !plain.contains("MimeType"));
        a.schemes = vec!["claude".into(), "web+x".into()];
        a.desktop_entry = false;
        let e = menu_entry(&c, &a, Path::new("/usr/bin/cardea"), None);
        assert!(e.contains(" a --url %u\n"), "{e}");
        assert!(e.contains("MimeType=x-scheme-handler/claude;x-scheme-handler/web+x;\n"));
        assert!(e.contains("NoDisplay=true\n"));
    }

    #[test]
    fn toolkits() {
        let mut c = builtin::profile("empty").unwrap();
        let l = launch(
            &c,
            &app(Toolkit::Electron, &["claude-desktop", "--x"]),
            &["file".into()],
        );
        assert_eq!(
            l.argv,
            [
                "claude-desktop",
                "--ozone-platform=wayland",
                "--no-sandbox",
                "--disable-gpu",
                "--x",
                "file"
            ]
        );
        c.capabilities.gpu = true;
        c.capabilities.keyring.mode = KeyringMode::Auto;
        c.appearance.theme = Theme::Dark;
        let l = launch(&c, &app(Toolkit::Electron, &["e"]), &[]);
        assert_eq!(
            l.argv,
            [
                "e",
                "--ozone-platform=wayland",
                "--no-sandbox",
                "--password-store=gnome-libsecret",
                "--force-dark-mode"
            ]
        );
        let mut qt = app(Toolkit::Qt, &["freecad"]);
        qt.env.insert("A".into(), "1".into());
        assert_eq!(launch(&c, &qt, &[]).env, ["QT_QPA_PLATFORM=wayland", "A=1"]);
        assert!(test_wayland(&qt, &[]).env.contains(&"DISPLAY=".to_owned()));
    }

    #[test]
    fn profile_apps_take_icon_and_class_from_the_image() {
        let found = [parse_desktop(
            "claude-desktop.desktop",
            "[Desktop Entry]\nName=Claude\nExec=/usr/bin/claude-desktop %u\nIcon=claude-desktop\n",
        )
        .unwrap()];
        let mut a = found[0].app();
        (a.icon, a.wm_class, a.exec) = (None, None, vec!["claude-desktop".into()]);
        complete(&mut a, &found);
        assert_eq!(a.icon.as_deref(), Some("claude-desktop"));
        // No StartupWMClass: the file's name.
        assert_eq!(a.wm_class.as_deref(), Some("claude-desktop"));
        // Set values stay; another program is not matched.
        a.wm_class = Some("Mine".into());
        complete(&mut a, &found);
        assert_eq!(a.wm_class.as_deref(), Some("Mine"));
        let mut b = found[0].app();
        (b.icon, b.exec) = (None, vec!["other".into()]);
        complete(&mut b, &found);
        assert_eq!(b.icon, None);
    }

    #[test]
    fn desktop_files() {
        let text = "[Desktop Entry]\nName=FreeCAD\nName[fr]=FreeCAD fr\nExec=freecad - %F\nIcon=org.freecad.FreeCAD\n\
                    Categories=Graphics;Science;Engineering;Qt;\nStartupWMClass=FreeCAD\n\
                    [Desktop Action New]\nName=New\nExec=freecad --new\n";
        let d = parse_desktop("org.freecad.FreeCAD.desktop", text).unwrap();
        assert_eq!(
            (
                d.id.as_str(),
                d.name.as_str(),
                d.exec.clone(),
                d.toolkit,
                d.wm_class.as_deref()
            ),
            (
                "freecad",
                "FreeCAD",
                vec!["freecad".to_owned(), "-".to_owned()],
                Toolkit::Qt,
                Some("FreeCAD")
            )
        );
        assert!(
            parse_desktop(
                "x.desktop",
                "[Desktop Entry]\nName=X\nExec=x\nNoDisplay=true\n"
            )
            .is_none()
        );
        let t = parse_desktop(
            "htop.desktop",
            "[Desktop Entry]\nName=Htop\nExec=htop\nTerminal=true\n",
        )
        .unwrap();
        assert_eq!(t.toolkit, Toolkit::Terminal);
    }

    #[test]
    fn exec_quoting() {
        assert_eq!(
            exec_argv(r#"sh -c "echo \"a b\" \$HOME 100%%" %u"#),
            ["sh", "-c", r#"echo "a b" $HOME 100%"#]
        );
        assert_eq!(exec_argv("  a   b  "), ["a", "b"]);
        assert_eq!(exec_argv(r#"a """#), ["a", ""]);
    }

    #[test]
    fn menu_entries() {
        let mut c = builtin::profile("empty").unwrap();
        c.id = "box".into();
        c.name = "My Box".into();
        let mut a = app(Toolkit::Qt, &["freecad"]);
        a.name = "FreeCAD".into();
        a.wm_class = Some("FreeCAD".into());
        let e = menu_entry(
            &c,
            &a,
            Path::new("/home/me/.local/bin/cardea"),
            Some(Path::new("/i/box-a.png")),
        );
        assert_eq!(
            e,
            "[Desktop Entry]\nType=Application\nName=FreeCAD (My Box)\nExec=\"/home/me/.local/bin/cardea\" app box a\n\
             X-Cardea-Sandbox=box\nIcon=/i/box-a.png\nStartupWMClass=FreeCAD\n"
        );
        assert_eq!(menu_file("box", "a"), "cardea-box-a.desktop");
    }

    #[test]
    fn claude_needs() {
        use crate::image::{Need, needs};
        let c = builtin::example_claude();
        assert_eq!(
            needs(&c),
            [
                Need::Keyring,
                Need::Gpu,
                Need::QtWayland,
                Need::Tmux,
                Need::Locales
            ]
        );
        assert!(requested_by(&c, Capability::Gpu).contains(&"FreeCAD"));
    }
}
