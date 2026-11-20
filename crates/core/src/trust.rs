//! Trust presets: choosing a trust level sets a group of settings, each of which stays editable.

use crate::model::{Blocked, Config, KeyringMode, Trust};

/// The settings a trust level sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preset {
    pub isolated_uid: bool,
    pub disposable_home: bool,
    pub blocked: Blocked,
    pub keyring: KeyringMode,
}

impl Trust {
    pub fn preset(self) -> Preset {
        match self {
            Trust::Work => Preset {
                isolated_uid: false,
                disposable_home: false,
                blocked: Blocked::Reject,
                keyring: KeyringMode::Auto,
            },
            Trust::Untrusted => Preset {
                isolated_uid: true,
                disposable_home: true,
                blocked: Blocked::Drop,
                keyring: KeyringMode::Off,
            },
        }
    }
}

impl Config {
    /// Sets the trust level and the settings of its preset. Settings outside the preset are kept.
    pub fn apply_trust(&mut self, trust: Trust) {
        let p = trust.preset();
        self.trust = trust;
        self.security.isolated_uid = p.isolated_uid;
        self.home.disposable = p.disposable_home;
        self.firewall.blocked = p.blocked;
        self.capabilities.keyring.mode = p.keyring;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::X11;

    #[test]
    fn untrusted_preset() {
        let c = Config::defaults(Trust::Untrusted);
        assert!(c.security.isolated_uid);
        assert!(c.home.disposable);
        assert_eq!(c.firewall.blocked, Blocked::Drop);
        assert_eq!(c.capabilities.keyring.mode, KeyringMode::Off);
        // Risky settings are off in both presets.
        assert!(!c.capabilities.kvm && c.security.seccomp && c.firewall.enabled);
        assert_ne!(c.capabilities.x11, X11::Host);
    }

    #[test]
    fn switching_back_to_work_restores_the_work_preset_only() {
        let mut c = Config::defaults(Trust::Untrusted);
        c.capabilities.gpu = true;
        c.apply_trust(Trust::Work);
        assert_eq!(c, {
            let mut w = Config::defaults(Trust::Work);
            w.capabilities.gpu = true;
            w
        });
    }
}
