//! `cardea doctor`, Podman side: what the daemon sees of Podman on this host. Session and tray checks belong to
//! the client's desktop session and are done by the CLI.

use std::path::Path;
use std::time::Duration;

use cardea_engine::podman::{Client, IdMap, Info};
use cardea_proto::{Check, CheckStatus};

/// Oldest Podman Cardea supports.
const MIN_VERSION: (u32, u32) = (4, 4);
/// Sub-IDs a sandbox needs: a whole user namespace for the system users of a distribution image.
const MIN_SUB_IDS: u64 = 65536;

pub async fn podman(socket: &Path) -> Vec<Check> {
    let client = Client::new(socket);
    let info = match client.info().await {
        Ok(info) => info,
        Err(e) => {
            return vec![fail(
                "Podman socket",
                e,
                "Install the podman package and enable its socket: \
                 systemctl --user enable --now podman.socket",
            )];
        }
    };
    let events = tokio::time::timeout(Duration::from_secs(3), client.events()).await;
    vec![
        ok("Podman socket", format!("answers on {}", socket.display())),
        version(&info),
        sub_ids(&info),
        pasta(&info),
        match events {
            Ok(Ok(_)) => ok("Podman events", "streamed".to_owned()),
            Ok(Err(e)) => fail(
                "Podman events",
                e,
                "Check `journalctl --user -u podman.service`",
            ),
            Err(_) => fail(
                "Podman events",
                "no answer within 3 s".to_owned(),
                "Check `journalctl --user -u podman.service`",
            ),
        },
    ]
}

fn version(info: &Info) -> Check {
    let v = &info.version.version;
    let mut parts = v.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    let found = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    if found >= MIN_VERSION {
        ok("Podman version", v.clone())
    } else {
        fail(
            "Podman version",
            format!(
                "{v}; Cardea needs {}.{} or later",
                MIN_VERSION.0, MIN_VERSION.1
            ),
            "Install a newer Podman (Debian 13 or later ships one)",
        )
    }
}

/// The rootless mappings beyond the user's own UID are the sub-IDs from `/etc/subuid` and `/etc/subgid`.
fn sub_ids(info: &Info) -> Check {
    let count = |map: &Option<Vec<IdMap>>| {
        map.iter()
            .flatten()
            .filter(|m| m.container_id != 0)
            .map(|m| m.size)
            .sum::<u64>()
    };
    let m = &info.host.id_mappings;
    let (uids, gids) = (count(&m.uidmap), count(&m.gidmap));
    let detail = format!("{uids} sub-UIDs, {gids} sub-GIDs");
    if uids.min(gids) >= MIN_SUB_IDS {
        ok("Sub-UIDs", detail)
    } else {
        fail(
            "Sub-UIDs",
            format!("{detail}; Cardea needs {MIN_SUB_IDS} of each"),
            "sudo usermod --add-subuids 100000-165535 --add-subgids 100000-165535 $USER, \
             then podman system migrate",
        )
    }
}

fn pasta(info: &Info) -> Check {
    if let Some(p) = info
        .host
        .pasta
        .as_ref()
        .filter(|p| !p.executable.is_empty())
    {
        return ok(
            "passt",
            p.version.lines().next().unwrap_or(&p.executable).to_owned(),
        );
    }
    // Podman before 4.5 does not report pasta: look for it where Podman would.
    let found = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .chain([Path::new("/usr/libexec/podman").to_path_buf()])
        .any(|d| d.join("pasta").is_file());
    if found {
        ok("passt", "pasta found".to_owned())
    } else {
        fail(
            "passt",
            "pasta not found".to_owned(),
            "Install the passt package",
        )
    }
}

fn ok(name: &str, detail: String) -> Check {
    Check {
        name: name.to_owned(),
        status: CheckStatus::Ok,
        detail,
        hint: None,
    }
}

fn fail(name: &str, detail: String, hint: &str) -> Check {
    Check {
        name: name.to_owned(),
        status: CheckStatus::Fail,
        detail,
        hint: Some(hint.to_owned()),
    }
}

/// The checks of the user's desktop session, made by the client that runs in it: session type (with the ways out
/// of X11), tray host.
pub fn desktop() -> Vec<Check> {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    vec![
        session(
            var("XDG_SESSION_TYPE").as_deref(),
            var("WAYLAND_DISPLAY").is_some(),
            var("XDG_CURRENT_DESKTOP").as_deref(),
        ),
        tray(var("XDG_CURRENT_DESKTOP").as_deref()),
    ]
}

fn session(kind: Option<&str>, wayland_display: bool, desktop: Option<&str>) -> Check {
    let on = desktop.map(|d| format!(" ({d})")).unwrap_or_default();
    match kind {
        Some("wayland") => check_with(CheckStatus::Ok, "Session", format!("Wayland{on}"), None),
        Some("x11") if !wayland_display => check_with(
            CheckStatus::Fail,
            "Session",
            format!("X11{on}: sandboxed apps cannot show their windows"),
            Some(
                "1. Log out and pick your desktop's Wayland session in the login screen's session menu\n\
                 2. Or install a desktop that runs on Wayland (KDE Plasma: task-kde-desktop, GNOME: task-gnome-desktop)\n\
                 3. Or stay on X11 with `x11: host` in the sandbox config (no display isolation, not recommended)",
            ),
        ),
        _ if wayland_display => {
            check_with(CheckStatus::Ok, "Session", format!("Wayland{on}"), None)
        }
        other => check_with(
            CheckStatus::Warn,
            "Session",
            format!(
                "no graphical session (XDG_SESSION_TYPE is {})",
                other.unwrap_or("not set")
            ),
            Some("Run Cardea from your desktop session to use graphical apps"),
        ),
    }
}

/// A tray host owns `org.kde.StatusNotifierWatcher` on the session bus.
fn tray(desktop: Option<&str>) -> Check {
    let answer = std::process::Command::new("busctl")
        .args([
            "--user",
            "call",
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "NameHasOwner",
            "s",
            "org.kde.StatusNotifierWatcher",
        ])
        .output();
    let hint = match desktop.unwrap_or("").to_ascii_lowercase() {
        d if d.contains("gnome") => {
            "Install and enable the AppIndicator extension (gnome-shell-extension-appindicator)"
        }
        d if d.contains("cinnamon") => "Install xapp-sn-watcher (package xapp) and log in again",
        _ => "Your desktop or bar needs a StatusNotifierItem tray",
    };
    match answer {
        Ok(o) if o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "b true" => {
            check_with(CheckStatus::Ok, "Tray host", "present".to_owned(), None)
        }
        Ok(o) if o.status.success() => check_with(
            CheckStatus::Warn,
            "Tray host",
            "none: the tray icon will not show".to_owned(),
            Some(hint),
        ),
        Ok(o) => check_with(
            CheckStatus::Warn,
            "Tray host",
            format!(
                "cannot ask the session bus: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            ),
            None,
        ),
        Err(e) => check_with(
            CheckStatus::Warn,
            "Tray host",
            format!("cannot run busctl: {e}"),
            None,
        ),
    }
}

fn check_with(status: CheckStatus, name: &str, detail: String, hint: Option<&str>) -> Check {
    Check {
        name: name.to_owned(),
        status,
        detail,
        hint: hint.map(str::to_owned),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(json: &str) -> Info {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn version_needs_4_4() {
        let v = |s: &str| version(&info(&format!(r#"{{"version":{{"Version":"{s}"}}}}"#))).status;
        assert_eq!(v("5.4.2"), CheckStatus::Ok);
        assert_eq!(v("4.4.0"), CheckStatus::Ok);
        assert_eq!(v("4.10.1"), CheckStatus::Ok);
        assert_eq!(v("4.3.1"), CheckStatus::Fail);
        assert_eq!(v(""), CheckStatus::Fail);
    }

    #[test]
    fn sub_ids_ignore_the_users_own_uid() {
        let full = info(
            r#"{"host":{"idMappings":{
            "uidmap":[{"container_id":0,"host_id":1000,"size":1},{"container_id":1,"host_id":100000,"size":65536}],
            "gidmap":[{"container_id":0,"host_id":1000,"size":1},{"container_id":1,"host_id":100000,"size":65536}]}}}"#,
        );
        assert_eq!(sub_ids(&full).status, CheckStatus::Ok);
        let none = info(
            r#"{"host":{"idMappings":{"uidmap":[{"container_id":0,"host_id":1000,"size":1}]}}}"#,
        );
        let c = sub_ids(&none);
        assert_eq!(
            (c.status, c.detail.starts_with("0 sub-UIDs, 0 sub-GIDs")),
            (CheckStatus::Fail, true)
        );
    }

    #[test]
    fn pasta_from_info() {
        let c = pasta(&info(
            r#"{"host":{"pasta":{"executable":"/usr/bin/pasta","version":"pasta 2025_01\nCopyright"}}}"#,
        ));
        assert_eq!(
            (c.status, c.detail.as_str()),
            (CheckStatus::Ok, "pasta 2025_01")
        );
    }

    #[tokio::test]
    async fn no_podman_is_one_failed_check() {
        let checks = podman(Path::new("/nonexistent/podman.sock")).await;
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, CheckStatus::Fail);
        assert!(checks[0].hint.as_deref().unwrap().contains("podman.socket"));
    }

    #[test]
    fn session_kinds() {
        let s = |kind, wd, de| session(kind, wd, de);
        assert_eq!(
            s(Some("wayland"), true, Some("KDE")).detail,
            "Wayland (KDE)"
        );
        assert_eq!(s(Some("x11"), false, None).status, CheckStatus::Fail);
        assert!(
            s(Some("x11"), false, None)
                .hint
                .unwrap()
                .starts_with("1. Log out")
        );
        // A nested or forwarded Wayland display counts even when logind says otherwise.
        assert_eq!(s(Some("tty"), true, None).status, CheckStatus::Ok);
        assert_eq!(s(None, false, None).status, CheckStatus::Warn);
    }
}
