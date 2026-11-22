//! What a config change needs before it applies: nothing, a restart or a rebuild.

use crate::model::Config;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Impact {
    /// Applies at once (names, icon, firewall, autostart, app list).
    None,
    Restart,
    Rebuild,
}

/// The impact of going from `old` to `new`.
pub fn impact(old: &Config, new: &Config) -> Impact {
    // Sources installing in the home run in the sandbox: only those of the image go into it.
    let image_sources = |c: &Config| -> Vec<_> {
        c.sources
            .iter()
            .filter(|s| s.scope == crate::model::Scope::Image)
            .cloned()
            .collect()
    };
    // Everything that goes into the image.
    let rebuild = old.base_image != new.base_image
        || old.apt_repos != new.apt_repos
        || image_sources(old) != image_sources(new)
        || image_packages(old) != image_packages(new)
        || old.setup.build != new.setup.build
        || old.identity.user != new.identity.user
        || old.appearance != new.appearance
        // What capabilities, Qt apps and services install (GPU drivers, keyring, Xwayland, tmux…); the rest of a
        // capability (devices, sockets) applies at the next start.
        || crate::image::needs(old) != crate::image::needs(new);
    if rebuild {
        return Impact::Rebuild;
    }
    let restart = old.identity != new.identity
        || old.home != new.home
        || old.folders != new.folders
        || old.protected_paths != new.protected_paths
        || old.security != new.security
        || old.capabilities != new.capabilities
        || old.trust != new.trust
        // The link channel (relay, FIFO, opener) is set up at start; asking and allowed hosts apply at once.
        || old.links.outgoing != new.links.outgoing
        || old.setup != new.setup;
    if restart {
        Impact::Restart
    } else {
        Impact::None
    }
}

/// Packages of the image; home packages are installed in the running sandbox and need nothing.
fn image_packages(c: &Config) -> Vec<&crate::model::Package> {
    crate::packages::of_scope(c, crate::model::Scope::Image)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Trust;

    #[test]
    fn impacts() {
        let old = Config::defaults(Trust::Work);
        let mut new = old.clone();
        new.name = "Renamed".into();
        new.firewall.enabled = false;
        assert_eq!(impact(&old, &new), Impact::None);
        new.security.ram_limit_gb = Some(4);
        assert_eq!(impact(&old, &new), Impact::Restart);
        new.base_image = "docker.io/library/debian:sid".into();
        assert_eq!(impact(&old, &new), Impact::Rebuild);
    }

    #[test]
    fn home_packages_apply_at_once() {
        use crate::model::Package;
        let old = Config::defaults(Trust::Work);
        let mut new = old.clone();
        let pkg = |source: &str| Package {
            source: source.into(),
            name: "x".into(),
            version: None,
            when: None,
        };
        new.packages.push(pkg("npm"));
        assert_eq!(impact(&old, &new), Impact::None);
        new.packages.push(pkg("system"));
        assert_eq!(impact(&old, &new), Impact::Rebuild);
    }

    #[test]
    fn only_what_goes_into_the_image_rebuilds() {
        use crate::model::{Scope, Source};
        let old = Config::defaults(Trust::Work);
        let mut new = old.clone();
        new.capabilities.kvm = !old.capabilities.kvm;
        assert_eq!(impact(&old, &new), Impact::Restart);
        let mut new = old.clone();
        let source = |scope| Source {
            id: "st".into(),
            name: "St".into(),
            scope,
            setup: String::new(),
            install: "true".into(),
            remove: "true".into(),
            update: "true".into(),
            list: "true".into(),
        };
        new.sources.push(source(Scope::Home));
        assert_eq!(impact(&old, &new), Impact::None);
        new.sources[0] = source(Scope::Image);
        assert_eq!(impact(&old, &new), Impact::Rebuild);
        let mut new = old.clone();
        new.capabilities.gpu = !old.capabilities.gpu;
        assert_eq!(impact(&old, &new), Impact::Rebuild);
    }
}
