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
    let toolkits = |c: &Config| {
        let mut t: Vec<_> = c.apps.iter().map(|a| a.toolkit).collect();
        t.sort_by_key(|k| format!("{k:?}"));
        t.dedup();
        t
    };
    // Everything that goes into the image.
    let rebuild = old.base_image != new.base_image
        || old.apt_repos != new.apt_repos
        || old.sources != new.sources
        || old.packages != new.packages
        || old.setup.build != new.setup.build
        || old.identity.user != new.identity.user
        || old.capabilities != new.capabilities
        || old.appearance != new.appearance
        || toolkits(old) != toolkits(new)
        || old.services.is_empty() != new.services.is_empty();
    if rebuild {
        return Impact::Rebuild;
    }
    let restart = old.identity != new.identity
        || old.home != new.home
        || old.folders != new.folders
        || old.protected_paths != new.protected_paths
        || old.security != new.security
        || old.trust != new.trust
        || old.setup != new.setup;
    if restart {
        Impact::Restart
    } else {
        Impact::None
    }
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
}
