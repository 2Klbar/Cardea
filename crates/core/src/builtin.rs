//! Built-in profiles and sources, embedded in the binary as JSON (the same format as user files).

use crate::load;
use crate::model::{Config, Kind, Source};

/// Ids of the built-in sources. `system` follows the base image's package manager.
pub const SOURCE_IDS: [&str; 6] = ["system", "npm", "cargo", "rustup", "mise", "pipx"];

const PROFILES: [(&str, &str); 2] = [
    ("claude", include_str!("../builtin/profiles/claude.json")),
    ("empty", include_str!("../builtin/profiles/empty.json")),
];

/// The system package manager of a base image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Manager {
    Apt,
    Dnf,
    Pacman,
}

const SYSTEM_SOURCES: [(Manager, &str); 3] = [
    (
        Manager::Apt,
        include_str!("../builtin/sources/system-apt.json"),
    ),
    (
        Manager::Dnf,
        include_str!("../builtin/sources/system-dnf.json"),
    ),
    (
        Manager::Pacman,
        include_str!("../builtin/sources/system-pacman.json"),
    ),
];

const OTHER_SOURCES: [&str; 5] = [
    include_str!("../builtin/sources/npm.json"),
    include_str!("../builtin/sources/cargo.json"),
    include_str!("../builtin/sources/rustup.json"),
    include_str!("../builtin/sources/mise.json"),
    include_str!("../builtin/sources/pipx.json"),
];

/// Ids of the built-in profiles, in display order.
pub fn profile_ids() -> impl Iterator<Item = &'static str> {
    PROFILES.iter().map(|(id, _)| *id)
}

/// A built-in profile, with every default filled in.
pub fn profile(id: &str) -> Option<Config> {
    let (_, text) = PROFILES.iter().find(|(p, _)| *p == id)?;
    Some(load::config(text, Kind::Profile).expect("built-in profiles are valid (tested)"))
}

/// The built-in sources for a base image using `manager`.
pub fn sources(manager: Manager) -> Vec<Source> {
    let system = SYSTEM_SOURCES
        .iter()
        .find(|(m, _)| *m == manager)
        .map(|(_, t)| *t);
    system
        .into_iter()
        .chain(OTHER_SOURCES)
        .map(|text| load::source(text).expect("built-in sources are valid (tested)"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Capability, Trust};

    #[test]
    fn profiles_load_and_match_their_id() {
        for id in profile_ids() {
            let p = profile(id).unwrap();
            assert_eq!(p.id, id);
        }
        assert!(profile("nope").is_none());
    }

    #[test]
    fn claude_profile() {
        let p = profile("claude").unwrap();
        assert_eq!(p.trust, Trust::Work);
        assert!(p.capabilities.gpu && !p.capabilities.kvm && p.security.seccomp);
        assert!(
            p.services
                .iter()
                .any(|s| s.id == "remote-control" && s.per_folder)
        );
        let cowork: Vec<_> = p
            .packages
            .iter()
            .filter(|pkg| {
                pkg.when
                    .as_ref()
                    .is_some_and(|w| w.capability == Capability::Kvm)
            })
            .map(|pkg| pkg.name.as_str())
            .collect();
        assert_eq!(cowork, ["qemu-system-x86", "ovmf", "virtiofsd"]);
    }

    #[test]
    fn sources_load_with_the_expected_ids() {
        for m in [Manager::Apt, Manager::Dnf, Manager::Pacman] {
            let ids: Vec<_> = sources(m).into_iter().map(|s| s.id).collect();
            assert_eq!(ids, SOURCE_IDS);
        }
    }
}
