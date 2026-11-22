//! Risky settings of a sandbox, shown as persistent warnings (security R6, R7, R11).

use crate::model::{Clipboard, Config, Trust, X11, ZoneMode};

/// One line per risky setting that is on.
pub fn warnings(c: &Config) -> Vec<String> {
    let mut w = Vec::new();
    if !c.firewall.enabled {
        w.push(
            "firewall off: the sandbox reaches your computer, your network and the Internet"
                .to_owned(),
        );
    } else {
        if c.firewall.host.mode == ZoneMode::Block {
            w.push("Host zone in blocklist mode: every service on your computer is reachable except the listed ones".into());
        }
        if c.firewall.lan.mode == ZoneMode::Block {
            w.push("LAN zone in blocklist mode: every device on your network is reachable except the listed ones".into());
        }
    }
    if c.capabilities.kvm {
        w.push("KVM on: virtual machines can run inside the sandbox".into());
    }
    if !c.security.seccomp {
        w.push("seccomp off: the system call filter is disabled".into());
    }
    if c.capabilities.x11 == X11::Host {
        w.push("host X11: X11 apps of the sandbox can read keystrokes and screens of your other X11 apps".into());
    }
    if c.trust == Trust::Untrusted && !c.security.isolated_uid {
        w.push("untrusted sandbox running with your own user ID".into());
    }
    if c.trust == Trust::Untrusted && !c.home.disposable {
        w.push("untrusted sandbox with a persistent home".into());
    }
    if c.trust == Trust::Untrusted
        && c.capabilities.wayland
        && c.capabilities.clipboard == Clipboard::Shared
    {
        w.push("untrusted sandbox sharing your clipboard: its apps can read what you copy".into());
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_have_no_warning_and_risky_settings_do() {
        let mut c = Config::defaults(Trust::Work);
        assert!(warnings(&c).is_empty());
        c.firewall.enabled = false;
        c.capabilities.kvm = true;
        assert_eq!(warnings(&c).len(), 2);
    }

    /// R6: loosening an Untrusted default is a risky setting, so the clients ask first.
    #[test]
    fn loosening_untrusted_defaults_warns() {
        let mut c = Config::defaults(Trust::Untrusted);
        assert!(warnings(&c).is_empty());
        c.security.isolated_uid = false;
        c.home.disposable = false;
        c.capabilities.clipboard = Clipboard::Shared;
        let w = warnings(&c);
        assert!(w.iter().any(|w| w.contains("your own user ID")), "{w:?}");
        assert!(w.iter().any(|w| w.contains("persistent home")), "{w:?}");
        assert!(w.iter().any(|w| w.contains("clipboard")), "{w:?}");
    }
}
