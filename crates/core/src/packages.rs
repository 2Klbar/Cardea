//! Packages and their sources: which source installs a package, in the image (as root, at build) or in the home
//! (as the user, in the running sandbox), the commands that do it, and the languages of the sandbox.
//!
//! A command is always `sh -c '<template>' sh <name> <version>`: name and version are arguments (R9).

use serde::{Deserialize, Serialize};

use crate::builtin::{self, Manager};
use crate::model::{Config, Package, Scope, Source};

/// Base images offered by the clients: (label, reference). Any other fully qualified reference is allowed.
pub const BASE_IMAGES: [(&str, &str); 5] = [
    ("Debian 13 (trixie)", "docker.io/library/debian:trixie-slim"),
    (
        "Debian 12 (bookworm)",
        "docker.io/library/debian:bookworm-slim",
    ),
    ("Ubuntu 24.04 LTS", "docker.io/library/ubuntu:24.04"),
    ("Fedora 42", "registry.fedoraproject.org/fedora:42"),
    ("Arch Linux", "docker.io/library/archlinux:latest"),
];

pub fn manager_name(m: Manager) -> &'static str {
    match m {
        Manager::Apt => "apt",
        Manager::Dnf => "dnf",
        Manager::Pacman => "pacman",
    }
}

pub fn manager_from(name: &str) -> Option<Manager> {
    [Manager::Apt, Manager::Dnf, Manager::Pacman]
        .into_iter()
        .find(|m| manager_name(*m) == name.trim())
}

/// Every source a sandbox can use: the built-in ones (with `system` for `manager`, left out when unknown) and
/// its custom ones.
pub fn sources(c: &Config, manager: Option<Manager>) -> Vec<Source> {
    let mut all = builtin::sources(manager.unwrap_or(Manager::Apt));
    if manager.is_none() {
        all.retain(|s| s.id != "system");
    }
    all.extend(c.sources.iter().cloned());
    all
}

/// Where a source installs; `system` is the image whatever the manager.
pub fn scope(c: &Config, source: &str) -> Option<Scope> {
    if source == "system" {
        return Some(Scope::Image);
    }
    sources(c, None)
        .into_iter()
        .find(|s| s.id == source)
        .map(|s| s.scope)
}

/// The packages installed now: those whose `when` capability is on.
pub fn wanted(c: &Config) -> impl Iterator<Item = &Package> {
    c.packages.iter().filter(|p| {
        p.when
            .as_ref()
            .is_none_or(|w| c.capabilities.is_on(w.capability))
    })
}

/// The wanted packages of one scope, in list order.
pub fn of_scope(c: &Config, scope: Scope) -> Vec<&Package> {
    wanted(c)
        .filter(|p| self::scope(c, &p.source) == Some(scope))
        .collect()
}

/// `sh -c '<template>' sh <name> <version>`.
pub fn command(template: &str, name: &str, version: Option<&str>) -> Vec<String> {
    ["sh", "-c", template, "sh", name, version.unwrap_or("")]
        .map(String::from)
        .to_vec()
}

/// A package as the clients show it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageInfo {
    pub source: String,
    pub name: String,
    /// The version asked for, if any.
    pub version: Option<String>,
    /// Installed in the image (changes need a rebuild) rather than in the home.
    pub image: bool,
    /// The version found in the running sandbox; `None` when it is not installed or the sandbox is stopped.
    pub installed: Option<String>,
    /// Skipped: its `when` capability is off.
    pub skipped: bool,
}

/// `name version` lines of a source's `list` command.
pub fn parse_list(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            Some((f.next()?.to_owned(), f.next().unwrap_or("").to_owned()))
        })
        .collect()
}

/// The configured packages with what is installed: `installed` holds (source, name, version).
pub fn infos(c: &Config, installed: &[(String, String, String)]) -> Vec<PackageInfo> {
    c.packages
        .iter()
        .map(|p| PackageInfo {
            source: p.source.clone(),
            name: p.name.clone(),
            version: p.version.clone(),
            image: scope(c, &p.source) == Some(Scope::Image),
            installed: installed
                .iter()
                .find(|(s, n, _)| *s == p.source && *n == p.name)
                .map(|(_, _, v)| v.clone()),
            skipped: p
                .when
                .as_ref()
                .is_some_and(|w| !c.capabilities.is_on(w.capability)),
        })
        .collect()
}

/// The locale of a language code: `fr` gives `fr_FR.UTF-8`, `pt_BR` gives `pt_BR.UTF-8`.
pub fn locale(lang: &str) -> String {
    if lang.contains('_') {
        return format!("{lang}.UTF-8");
    }
    let region = match lang {
        "en" => "US",
        "pt" => "PT",
        "zh" => "CN",
        "ja" => "JP",
        "ko" => "KR",
        "sv" => "SE",
        "da" => "DK",
        "cs" => "CZ",
        "el" => "GR",
        "uk" => "UA",
        other => return format!("{other}_{}.UTF-8", other.to_uppercase()),
    };
    format!("{lang}_{region}.UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Capability, When};

    #[test]
    fn scopes_and_order() {
        let mut c = builtin::example_claude();
        c.sources.push(Source {
            id: "gem".into(),
            name: "Ruby gems".into(),
            scope: Scope::Image,
            setup: String::new(),
            install: "gem install \"$1\"".into(),
            remove: "gem uninstall \"$1\"".into(),
            update: "gem update \"$1\"".into(),
            list: "gem list".into(),
        });
        c.packages.push(Package {
            source: "gem".into(),
            name: "rubocop".into(),
            version: None,
            when: None,
        });
        assert_eq!(scope(&c, "system"), Some(Scope::Image));
        assert_eq!(scope(&c, "cargo"), Some(Scope::Home));
        assert_eq!(scope(&c, "gem"), Some(Scope::Image));
        let home: Vec<_> = of_scope(&c, Scope::Home)
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(
            home,
            [
                "node",
                "go",
                "gradle",
                "dotnet",
                "stable",
                "cargo-audit",
                "cargo-deny",
                "graphifyy",
                "codeburn"
            ]
        );
        // `when` keeps the Cowork packages out until KVM is on.
        let image = of_scope(&c, Scope::Image);
        assert!(
            !image.iter().any(|p| p.name == "virtiofsd") && image.last().unwrap().name == "rubocop"
        );
        c.capabilities.kvm = true;
        assert!(
            of_scope(&c, Scope::Image)
                .iter()
                .any(|p| p.name == "virtiofsd")
        );
        assert_eq!(
            command("npm i \"$1\"", "a;b", None),
            ["sh", "-c", "npm i \"$1\"", "sh", "a;b", ""]
        );
        let _ = When {
            capability: Capability::Kvm,
        };
    }

    #[test]
    fn installed_versions() {
        let c = builtin::example_claude();
        let list = parse_list("ripgrep 14.1.1\ncurl 8.14\n\n");
        let installed: Vec<_> = list
            .into_iter()
            .map(|(n, v)| ("system".to_owned(), n, v))
            .collect();
        let infos = infos(&c, &installed);
        let rg = infos.iter().find(|i| i.name == "ripgrep").unwrap();
        assert_eq!((rg.image, rg.installed.as_deref()), (true, Some("14.1.1")));
        let node = infos.iter().find(|i| i.name == "node").unwrap();
        assert_eq!(
            (
                node.image,
                node.installed.as_deref(),
                node.version.as_deref()
            ),
            (false, None, Some("lts"))
        );
        assert!(infos.iter().find(|i| i.name == "ovmf").unwrap().skipped);
    }

    #[test]
    fn locales() {
        assert_eq!(locale("fr"), "fr_FR.UTF-8");
        assert_eq!(locale("en"), "en_US.UTF-8");
        assert_eq!(locale("pt_BR"), "pt_BR.UTF-8");
        assert_eq!(manager_from("dnf\n"), Some(Manager::Dnf));
    }
}
