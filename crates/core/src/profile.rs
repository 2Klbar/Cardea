//! Profiles as shared files: what an imported profile asks for (R10), and a sandbox saved as a profile.

use serde::{Deserialize, Serialize};

use crate::firewall::Net;
use crate::model::{Config, KeyringMode, Trust, X11, ZoneMode};
use crate::packages::BASE_IMAGES;

/// One line of an import review.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub title: String,
    pub detail: String,
    /// It widens what the sandbox can reach or run: highlighted, and shown in full.
    pub wide: bool,
}

fn item(title: &str, detail: String, wide: bool) -> Item {
    Item {
        title: title.to_owned(),
        detail,
        wide,
    }
}

/// What a profile asks for, widening items first. Scripts and custom sources are shown in full.
pub fn review(c: &Config) -> Vec<Item> {
    let mut out = Vec::new();
    let script = [
        ("build (root)", &c.setup.build),
        ("first start (you)", &c.setup.first_start),
    ]
    .iter()
    .filter(|(_, s)| !s.trim().is_empty())
    .map(|(when, s)| format!("# {when}\n{}", s.trim_end()))
    .collect::<Vec<_>>();
    if !script.is_empty() {
        out.push(item("Setup script", script.join("\n"), true));
    }
    for s in &c.sources {
        let commands = [
            ("setup", &s.setup),
            ("install", &s.install),
            ("remove", &s.remove),
            ("update", &s.update),
            ("list", &s.list),
        ]
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| format!("{k}: {v}"))
        .collect::<Vec<_>>()
        .join("\n");
        let scope = match s.scope {
            crate::model::Scope::Image => "runs as root in the image",
            crate::model::Scope::Home => "runs as you in the home",
        };
        out.push(item(
            "Custom package source",
            format!("“{}” ({}), {scope}\n{commands}", s.name, s.id),
            true,
        ));
    }
    let caps = &c.capabilities;
    let mut wide = Vec::new();
    if caps.gpu {
        wide.push("GPU".to_owned());
    }
    if caps.kvm {
        wide.push("Virtualization (KVM)".into());
    }
    if !c.security.seccomp {
        wide.push("system call filter off".into());
    }
    if caps.x11 == X11::Host {
        wide.push("your X server (host X11)".into());
    }
    if !wide.is_empty() {
        out.push(item("Capabilities", wide.join(" · "), true));
    }
    let fw = &c.firewall;
    let loosened = !fw.enabled
        || fw.host.mode == ZoneMode::Block
        || fw.lan.mode == ZoneMode::Block
        || !fw.host.allow.is_empty()
        || !fw.lan.allow.is_empty();
    if loosened {
        let zone = |name: &str, z: &crate::model::Zone| {
            let (mode, list) = match z.mode {
                ZoneMode::Allow => ("allowlist", &z.allow),
                ZoneMode::Block => ("blocklist", &z.block),
            };
            let entries: Vec<String> = list.iter().map(|r| r.to.clone()).collect();
            if entries.is_empty() {
                format!("{name}: {mode} (empty)")
            } else {
                format!("{name}: {mode} + {}", entries.join(", "))
            }
        };
        let detail = if fw.enabled {
            format!(
                "{} · {} · {}",
                zone("Host", &fw.host),
                zone("Local network", &fw.lan),
                zone("Internet", &fw.wan)
            )
        } else {
            "Firewall off".to_owned()
        };
        out.push(item("Network", detail, true));
    }
    for r in &c.apt_repos {
        out.push(item(
            "Third-party apt repository",
            format!("{} · {}\nFingerprint {}", r.name, r.url, r.fingerprint),
            true,
        ));
    }
    if c.trust == Trust::Work {
        out.push(item(
            "Trust",
            "Work: your own user ID, a persistent home, writable folders".into(),
            true,
        ));
    } else {
        out.push(item("Trust", "Untrusted".into(), false));
    }
    let known = BASE_IMAGES.iter().find(|(_, r)| *r == c.base_image);
    out.push(item(
        "Base image",
        known.map_or_else(
            || format!("{} (not in the standard list)", c.base_image),
            |(n, _)| (*n).to_owned(),
        ),
        known.is_none(),
    ));
    let apps: Vec<&str> = c.apps.iter().map(|a| a.name.as_str()).collect();
    out.push(item(
        "Apps",
        if apps.is_empty() {
            "none".into()
        } else {
            apps.join(" · ")
        },
        false,
    ));
    let packages: Vec<String> = c
        .packages
        .iter()
        .map(|p| format!("{}: {}", p.source, p.name))
        .collect();
    out.push(item(
        "Packages",
        format!("{}\n{}", packages.len(), packages.join("\n")),
        false,
    ));
    if caps.keyring.mode != KeyringMode::Off {
        out.push(item("Keyring", "a secret store for apps".into(), false));
    }
    if !c.services.is_empty() {
        let names: Vec<&str> = c.services.iter().map(|s| s.name.as_str()).collect();
        out.push(item("Background services", names.join(" · "), false));
    }
    out.sort_by_key(|i| !i.wide);
    out
}

/// A sandbox's configuration as a profile: everything local to it removed (shared folders, home path,
/// hostname, start at login, service autostart, IP addresses in the firewall).
pub fn from_sandbox(c: &Config, id: &str, name: &str) -> Config {
    let mut p = c.clone();
    p.id = id.to_owned();
    p.name = name.to_owned();
    p.profile = None;
    p.folders.clear();
    p.home.path = None;
    p.identity.hostname = None;
    p.autostart = None;
    for s in &mut p.services {
        s.autostart = None;
    }
    for z in [
        &mut p.firewall.host,
        &mut p.firewall.lan,
        &mut p.firewall.wan,
    ] {
        z.allow.retain(|r| Net::parse(&r.to).is_none());
        z.block.retain(|r| Net::parse(&r.to).is_none());
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin;
    use crate::model::{Folder, Kind, Mode, Rule};

    #[test]
    fn risky_items_come_first_and_scripts_in_full() {
        let mut c = builtin::example_claude();
        c.capabilities.kvm = true;
        c.firewall.lan.mode = ZoneMode::Block;
        let items = review(&c);
        let wide: Vec<&str> = items
            .iter()
            .take_while(|i| i.wide)
            .map(|i| i.title.as_str())
            .collect();
        assert_eq!(
            wide,
            [
                "Setup script",
                "Capabilities",
                "Network",
                "Third-party apt repository",
                "Trust"
            ]
        );
        assert!(items[0].detail.contains(c.setup.first_start.trim()));
        assert!(
            items
                .iter()
                .any(|i| i.title == "Capabilities" && i.detail.contains("KVM"))
        );
        assert!(items.iter().all(|i| i.title != "Base image" || !i.wide));
        let empty = builtin::profile("empty").unwrap();
        assert!(
            review(&empty)
                .iter()
                .filter(|i| i.wide)
                .all(|i| i.title == "Trust")
        );
    }

    #[test]
    fn any_lan_entry_widens() {
        let mut c = builtin::profile("empty").unwrap();
        let network = |c: &Config| review(c).iter().any(|i| i.title == "Network" && i.wide);
        assert!(!network(&c));
        c.firewall.lan.allow.push(Rule {
            to: "gateway".into(),
            port: None,
            proto: None,
            comment: None,
        });
        assert!(network(&c));
    }

    #[test]
    fn saved_profiles_hold_nothing_local() {
        let mut c = builtin::example_claude();
        c.id = "work".into();
        c.profile = Some("claude".into());
        c.folders = vec![Folder {
            host: "/home/me/p".into(),
            mount: "/work/p".into(),
            mode: Mode::Rw,
        }];
        c.home.path = Some("/data/home".into());
        c.identity.hostname = Some("box".into());
        c.autostart = Some(true);
        for to in ["192.168.1.30", "gateway"] {
            c.firewall.lan.allow.push(Rule {
                to: to.into(),
                port: None,
                proto: None,
                comment: None,
            });
        }
        let p = from_sandbox(&c, "mine", "Mine");
        crate::validate::config(&p, Kind::Profile).unwrap();
        assert_eq!(p.firewall.lan.allow.len(), 1);
        assert_eq!(p.firewall.lan.allow[0].to, "gateway");
        assert_eq!((p.id.as_str(), p.name.as_str()), ("mine", "Mine"));
    }
}
