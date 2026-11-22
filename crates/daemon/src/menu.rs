//! Host menu entries of sandbox apps (`~/.local/share/applications/cardea-<id>-<app>.desktop`) and their icons.
//!
//! An icon comes from inside a sandbox: it is size-checked, then rasterised or re-encoded to PNG here (F12). Only
//! PNGs are written on the host; SVGs are rendered by `resvg`, which runs no script and loads nothing external.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use cardea_core::apps;
use cardea_core::model::Config;
use resvg::{tiny_skia, usvg};

use crate::paths::Paths;

/// Largest icon file read from a sandbox.
const MAX_ICON_BYTES: usize = 1 << 20;
/// Side of the PNG written on the host.
const ICON_PX: u32 = 256;

pub fn icon_path(paths: &Paths, sandbox: &str, app: &str) -> PathBuf {
    paths.icons.join(sandbox).join(format!("{app}.png"))
}

/// Writes `bytes` (the icon file at `source` in the sandbox) as a PNG at `target`.
pub fn save_icon(target: &Path, source: &str, bytes: &[u8]) -> Result<(), String> {
    let png = rasterize(source, bytes)?;
    if let Some(dir) = target.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(target, png).map_err(|e| e.to_string())
}

fn rasterize(source: &str, bytes: &[u8]) -> Result<Vec<u8>, String> {
    if bytes.len() > MAX_ICON_BYTES {
        return Err("the icon is larger than 1 MiB".into());
    }
    let pixmap = if source.ends_with(".svg") || source.ends_with(".svgz") {
        let tree = usvg::Tree::from_data(bytes, &usvg::Options::default())
            .map_err(|e| format!("not an SVG: {e}"))?;
        let size = tree.size();
        let scale = ICON_PX as f32 / size.width().max(size.height());
        let mut pixmap = tiny_skia::Pixmap::new(ICON_PX, ICON_PX).ok_or("cannot make the image")?;
        resvg::render(
            &tree,
            tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        pixmap
    } else {
        let pixmap = tiny_skia::Pixmap::decode_png(bytes).map_err(|e| format!("not a PNG: {e}"))?;
        if pixmap.width() > 1024 || pixmap.height() > 1024 {
            return Err("the icon is larger than 1024 pixels".into());
        }
        pixmap
    };
    pixmap.encode_png().map_err(|e| e.to_string())
}

/// Makes the menu entries match the sandbox's apps that ask for one or open links; removes the others of this
/// sandbox. With `mime` (off in tests), a scheme with no default handler on the host gets this sandbox's app.
pub fn sync(paths: &Paths, c: &Config, mime: bool) -> std::io::Result<()> {
    fs::create_dir_all(&paths.applications)?;
    let cardea = std::env::current_exe()?;
    let mut wanted = Vec::new();
    for a in c
        .apps
        .iter()
        .filter(|a| a.desktop_entry || !a.schemes.is_empty())
    {
        let name = apps::menu_file(&c.id, &a.id);
        let icon = icon_path(paths, &c.id, &a.id);
        let text = apps::menu_entry(c, a, &cardea, icon.exists().then_some(icon.as_path()));
        let file = paths.applications.join(&name);
        if fs::read_to_string(&file).ok().as_deref() != Some(text.as_str()) {
            let tmp = paths.applications.join(format!(".{name}"));
            fs::write(&tmp, text)?;
            fs::rename(&tmp, &file)?;
        }
        wanted.push(name);
    }
    let dropped = remove_except(paths, &c.id, &wanted, mime)?;
    let with_schemes: Vec<_> = c.apps.iter().filter(|a| !a.schemes.is_empty()).collect();
    if mime && (dropped || !with_schemes.is_empty()) {
        update_database(paths);
    }
    if mime {
        for a in with_schemes {
            for s in &a.schemes {
                // A host app or another sandbox already handling it is only reported, never replaced.
                if default_handler(s).is_none() {
                    let _ = Command::new("xdg-mime")
                        .arg("default")
                        .arg(apps::menu_file(&c.id, &a.id))
                        .arg(format!("x-scheme-handler/{s}"))
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                }
            }
        }
    }
    Ok(())
}

/// Rebuilds `mimeinfo.cache`, which `xdg-mime` reads: after an entry with schemes appears, and after one goes (else
/// the cache still names it as a handler).
fn update_database(paths: &Paths) {
    let _ = Command::new("update-desktop-database")
        .arg(&paths.applications)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// The menu entry (file name) the desktop opens `<scheme>:` links with; `None` when none is set or `xdg-mime` is
/// missing.
pub fn default_handler(scheme: &str) -> Option<String> {
    let out = Command::new("xdg-mime")
        .args(["query", "default"])
        .arg(format!("x-scheme-handler/{scheme}"))
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let name = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (out.status.success() && !name.is_empty()).then_some(name)
}

/// The sandbox a Cardea menu entry belongs to.
pub fn owner(paths: &Paths, file: &str) -> Option<String> {
    if !file.starts_with("cardea-") || file.contains('/') {
        return None;
    }
    let text = fs::read_to_string(paths.applications.join(file)).ok()?;
    text.lines()
        .find_map(|l| l.strip_prefix("X-Cardea-Sandbox="))
        .map(str::to_owned)
}

/// Removes `file` from the handlers of `<scheme>:` in `~/.config/mimeapps.list`, so the desktop falls back to
/// another one (`xdg-mime` has no "unset").
fn forget_handler(paths: &Paths, scheme: &str, file: &str) -> std::io::Result<()> {
    let list = paths.autostart.with_file_name("mimeapps.list");
    let Ok(text) = fs::read_to_string(&list) else {
        return Ok(());
    };
    let key = format!("x-scheme-handler/{scheme}=");
    let mut out = String::new();
    for line in text.lines() {
        match line.strip_prefix(&key) {
            Some(v) => {
                let rest: Vec<&str> = v
                    .split(';')
                    .filter(|e| !e.is_empty() && *e != file)
                    .collect();
                if !rest.is_empty() {
                    out += &format!("{key}{};\n", rest.join(";"));
                }
            }
            None => {
                out += line;
                out.push('\n');
            }
        }
    }
    if out != text {
        let tmp = list.with_file_name(".mimeapps.list.cardea");
        fs::write(&tmp, out)?;
        fs::rename(&tmp, &list)?;
    }
    Ok(())
}

/// The tray's login entry (`~/.config/autostart/cardea-tray.desktop`), present when `on`.
pub fn tray_autostart(paths: &Paths, on: bool) -> std::io::Result<()> {
    let file = paths.autostart.join("cardea-tray.desktop");
    if !on {
        return match fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    let tray = crate::terminal::cardea().with_file_name("cardea-tray");
    let exec = if tray.is_file() {
        tray
    } else {
        PathBuf::from("cardea-tray")
    };
    fs::create_dir_all(&paths.autostart)?;
    fs::write(
        file,
        format!(
            "[Desktop Entry]\nType=Application\nName=Cardea tray\nComment=Sandboxes in the tray\nExec=\"{}\"\nIcon=cardea\n\
             X-GNOME-Autostart-enabled=true\nNoDisplay=true\n",
            exec.display()
        ),
    )
}

/// Removes every menu entry and icon of a sandbox.
pub fn remove_all(paths: &Paths, sandbox: &str, mime: bool) -> std::io::Result<()> {
    if remove_except(paths, sandbox, &[], mime)? && mime {
        update_database(paths);
    }
    match fs::remove_dir_all(paths.icons.join(sandbox)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// An entry belongs to a sandbox by its `X-Cardea-Sandbox` line, not its file name (`a` and `a-b` share a prefix).
/// A default handler left pointing at a removed entry is reset, only if it still names it. True when a removed
/// entry handled schemes.
fn remove_except(
    paths: &Paths,
    sandbox: &str,
    keep: &[String],
    mime: bool,
) -> std::io::Result<bool> {
    let Ok(dir) = fs::read_dir(&paths.applications) else {
        return Ok(false);
    };
    let mut dropped = false;
    let mark = format!("\nX-Cardea-Sandbox={sandbox}\n");
    for e in dir.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with("cardea-") || !name.ends_with(".desktop") || keep.contains(&name) {
            continue;
        }
        let Ok(text) = fs::read_to_string(e.path()) else {
            continue;
        };
        if !text.contains(&mark) {
            continue;
        }
        fs::remove_file(e.path())?;
        let schemes = text
            .lines()
            .filter_map(|l| l.strip_prefix("MimeType="))
            .flat_map(|v| v.split(';'))
            .filter_map(|m| m.strip_prefix("x-scheme-handler/"));
        for s in schemes {
            dropped = true;
            if mime && default_handler(s).as_deref() == Some(name.as_str()) {
                forget_handler(paths, s, &name)?;
            }
        }
    }
    Ok(dropped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cardea_core::apps::Detected;
    use cardea_core::builtin;
    use cardea_core::model::Toolkit;

    fn paths(root: &Path) -> Paths {
        Paths {
            config: root.join("c"),
            data: root.join("d"),
            runtime: root.join("r"),
            sandboxes: root.join("s"),
            home: root.to_path_buf(),
            podman: root.join("p"),
            applications: root.join("applications"),
            icons: root.join("icons"),
            autostart: root.join("autostart"),
        }
    }

    fn config(id: &str, apps: &[&str]) -> Config {
        let mut c = builtin::profile("empty").unwrap();
        c.id = id.into();
        c.apps = apps
            .iter()
            .map(|a| {
                Detected {
                    id: (*a).into(),
                    name: a.to_uppercase(),
                    exec: vec![(*a).into()],
                    toolkit: Toolkit::Qt,
                    icon: None,
                    wm_class: None,
                }
                .app()
            })
            .collect();
        c
    }

    #[test]
    fn entries_follow_the_apps_of_their_sandbox_only() {
        let t = tempfile::tempdir().unwrap();
        let p = paths(t.path());
        sync(&p, &config("a", &["x", "y"]), false).unwrap();
        sync(&p, &config("a-b", &["x"]), false).unwrap();
        let list = || {
            let mut v: Vec<String> = fs::read_dir(&p.applications)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        };
        assert_eq!(
            list(),
            [
                "cardea-a-b-x.desktop",
                "cardea-a-x.desktop",
                "cardea-a-y.desktop"
            ]
        );
        sync(&p, &config("a", &["y"]), false).unwrap();
        assert_eq!(list(), ["cardea-a-b-x.desktop", "cardea-a-y.desktop"]);
        remove_all(&p, "a", false).unwrap();
        assert_eq!(list(), ["cardea-a-b-x.desktop"]);
    }

    #[test]
    fn forgetting_a_handler_keeps_the_others() {
        let t = tempfile::tempdir().unwrap();
        let p = paths(t.path());
        let list = t.path().join("mimeapps.list");
        fs::write(
            &list,
            "[Default Applications]\nx-scheme-handler/claude=cardea-a-x.desktop\ntext/plain=kate.desktop\n\
             [Added Associations]\nx-scheme-handler/claude=other.desktop;cardea-a-x.desktop;\n",
        )
        .unwrap();
        forget_handler(&p, "claude", "cardea-a-x.desktop").unwrap();
        assert_eq!(
            fs::read_to_string(&list).unwrap(),
            "[Default Applications]\ntext/plain=kate.desktop\n[Added Associations]\n\
             x-scheme-handler/claude=other.desktop;\n"
        );
    }

    #[test]
    fn scheme_apps_get_an_entry_even_off_the_menu() {
        let t = tempfile::tempdir().unwrap();
        let p = paths(t.path());
        let mut c = config("a", &["x"]);
        c.apps[0].desktop_entry = false;
        c.apps[0].schemes = vec!["claude".into()];
        sync(&p, &c, false).unwrap();
        let text = fs::read_to_string(p.applications.join("cardea-a-x.desktop")).unwrap();
        assert!(
            text.contains("MimeType=x-scheme-handler/claude;") && text.contains("NoDisplay=true")
        );
        assert_eq!(owner(&p, "cardea-a-x.desktop").as_deref(), Some("a"));
        assert_eq!(owner(&p, "../x.desktop"), None);
    }

    #[test]
    fn tray_login_entry() {
        let t = tempfile::tempdir().unwrap();
        let p = paths(t.path());
        tray_autostart(&p, true).unwrap();
        let text = fs::read_to_string(p.autostart.join("cardea-tray.desktop")).unwrap();
        assert!(
            text.contains("Exec=\"") && text.contains("cardea-tray\""),
            "{text}"
        );
        tray_autostart(&p, false).unwrap();
        tray_autostart(&p, false).unwrap();
        assert!(!p.autostart.join("cardea-tray.desktop").exists());
    }

    #[test]
    fn icons_become_pngs() {
        let t = tempfile::tempdir().unwrap();
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect width="16" height="8" fill="#f00"/></svg>"##;
        let target = t.path().join("i/a.png");
        save_icon(&target, "/usr/share/icons/hicolor/scalable/apps/a.svg", svg).unwrap();
        let png = fs::read(&target).unwrap();
        let back = tiny_skia::Pixmap::decode_png(&png).unwrap();
        assert_eq!((back.width(), back.height()), (256, 256));
        // A PNG is decoded and written again; anything else is refused.
        save_icon(&target, "/x.png", &png).unwrap();
        assert!(save_icon(&target, "/x.png", b"<script>").is_err());
        // F12: scripts and external references in an SVG are never run nor loaded.
        let hostile = br##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="8" height="8">
            <script>alert(1)</script><image xlink:href="file:///etc/passwd" width="8" height="8"/>
            <rect width="8" height="8" fill="#00f"/></svg>"##;
        save_icon(&target, "/x.svg", hostile).unwrap();
        assert!(save_icon(&target, "/x.svg", &vec![b' '; MAX_ICON_BYTES + 1]).is_err());
    }
}
