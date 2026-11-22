//! Security & capabilities: trust preset, security settings, and the capabilities with the apps that request
//! them. Every change is one configuration save; risky ones ask first (`Message::Change`).

use cardea_core::apps::requested_by;
use cardea_core::model::{Capability, Clipboard, Config, KeyringMode, Trust, X11};
use cardea_proto::Summary;
use iced::widget::{column, container, row};
use iced::{Alignment, Length};

use super::kit::{El, bold, grow, heading, rule, section, small};
use crate::app::{App, Message, Page, Section};
use crate::style::{self, Kind, Tone};

impl App {
    pub fn security_view<'a>(&'a self, s: &'a Summary) -> El<'a> {
        let Some(c) = self.configs.get(&s.id) else {
            return small("Loading…").color(self.t.mu).into();
        };
        let busy = s.task.is_some();
        // A changed copy of the configuration, saved by the message.
        let change = |f: &dyn Fn(&mut Config)| {
            let mut new = c.clone();
            f(&mut new);
            (!busy).then(|| Message::Change(Box::new(new)))
        };
        let choice = |key: String, label: &'static str, on: bool, msg: Option<Message>| {
            self.btn(&key, label, Kind::Nav(on), msg)
        };
        let toggle = |key: &str, on: bool, msg: Option<Message>| {
            self.btn(key, if on { "● On" } else { "○ Off" }, Kind::Nav(on), msg)
        };
        let line = |label: &'a str, control: El<'a>, note: El<'a>| -> El<'a> {
            row![container(small(label)).width(230), control, note]
                .spacing(10)
                .align_y(Alignment::Center)
                .into()
        };

        let trust = c.trust;
        let description = match trust {
            Trust::Work => {
                "Work: your own user ID, a persistent home, blocked connections refused, keyring unlocked \
                            automatically."
            }
            Trust::Untrusted => {
                "Untrusted: an isolated user ID, a fresh home at each start, blocked connections dropped \
                                 silently, no keyring. Loosening one of these asks for confirmation."
            }
        };
        let preset = self.card(
            column![
                heading("Trust level", style::H2),
                row![
                    choice(
                        "sec:work".into(),
                        "Work",
                        trust == Trust::Work,
                        change(&|n| n.apply_trust(Trust::Work))
                    ),
                    choice(
                        "sec:untrusted".into(),
                        "! Untrusted",
                        trust == Trust::Untrusted,
                        change(&|n| n.apply_trust(Trust::Untrusted))
                    ),
                    small("A preset: every setting below stays editable.").color(self.t.mu),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                small(description).color(self.t.mu),
                rule(self),
                line(
                    "System call filter (seccomp)",
                    toggle(
                        "sec:seccomp",
                        c.security.seccomp,
                        change(&|n| n.security.seccomp = !n.security.seccomp)
                    ),
                    small("Off is risky: needed by some virtual machine tools only.")
                        .color(self.t.mu)
                        .into(),
                ),
                line(
                    "Isolated user ID",
                    toggle(
                        "sec:isolated",
                        c.security.isolated_uid,
                        change(&|n| n.security.isolated_uid = !n.security.isolated_uid)
                    ),
                    small("Files the sandbox writes are not yours on this computer.")
                        .color(self.t.mu)
                        .into(),
                ),
                line(
                    "Fresh home at each start",
                    toggle(
                        "sec:disposable",
                        c.home.disposable,
                        change(&|n| n.home.disposable = !n.home.disposable)
                    ),
                    small("Everything in the home is wiped when the sandbox stops.")
                        .color(self.t.mu)
                        .into(),
                ),
                line(
                    "Sandbox firewall",
                    self.pill(
                        if c.firewall.enabled {
                            "● On"
                        } else {
                            "! Off"
                        },
                        if c.firewall.enabled {
                            Tone::Ok
                        } else {
                            Tone::Err
                        }
                    ),
                    self.btn(
                        "sec:network",
                        "Network page",
                        Kind::Link,
                        Some(Message::Go(Page::Sandbox(s.id.clone(), Section::Network))),
                    ),
                ),
            ]
            .spacing(10),
        );

        let requested = |cap: Capability| {
            let by = requested_by(c, cap);
            small(if by.is_empty() {
                "—".to_owned()
            } else {
                by.join(", ")
            })
            .color(self.t.mu)
        };
        let cap_row = |name: &'a str, control: El<'a>, by: El<'a>| -> El<'a> {
            row![
                container(small(name).font(bold())).width(Length::FillPortion(2)),
                container(control).width(Length::FillPortion(3)),
                container(by).width(Length::FillPortion(3)),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into()
        };
        let caps = &c.capabilities;
        let keyring_mode = |key: &str, label: &'static str, mode: KeyringMode| {
            choice(
                format!("sec:keyring:{key}"),
                label,
                caps.keyring.mode == mode,
                change(&|n| n.capabilities.keyring.mode = mode),
            )
        };
        let mut keyring = row![
            keyring_mode("off", "Off", KeyringMode::Off),
            keyring_mode("auto", "Unlocked automatically", KeyringMode::Auto),
            keyring_mode("ask", "Ask at each start", KeyringMode::Ask),
        ]
        .spacing(4)
        .align_y(Alignment::Center);
        if c.home.disposable && caps.keyring.mode != KeyringMode::Off {
            keyring = keyring.push(self.btn(
                "sec:keyring:keep",
                if caps.keyring.keep {
                    "☑ Keep across starts"
                } else {
                    "☐ Keep across starts"
                },
                Kind::Nav(caps.keyring.keep),
                change(&|n| n.capabilities.keyring.keep = !n.capabilities.keyring.keep),
            ));
        }
        let x11 = |key: &str, label: &'static str, mode: X11| {
            choice(
                format!("sec:x11:{key}"),
                label,
                caps.x11 == mode,
                change(&|n| n.capabilities.x11 = mode),
            )
        };
        let clipboard = |key: &str, label: &'static str, mode: Clipboard| {
            choice(
                format!("sec:clipboard:{key}"),
                label,
                caps.clipboard == mode,
                change(&|n| n.capabilities.clipboard = mode),
            )
        };
        let capabilities = self.card(
            column![
                heading("Capabilities", style::H2),
                small(
                    "What the sandbox may use on your PC. Profiles and apps can only request a capability; “Requested \
                     by” comes from the apps’ declarations."
                )
                .color(self.t.mu),
                row![
                    container(small("Capability").color(self.t.mu)).width(Length::FillPortion(2)),
                    container(small("").color(self.t.mu)).width(Length::FillPortion(3)),
                    container(small("Requested by").color(self.t.mu)).width(Length::FillPortion(3)),
                ]
                .spacing(8),
                cap_row(
                    "Wayland display",
                    toggle("sec:wayland", caps.wayland, change(&|n| n.capabilities.wayland = !n.capabilities.wayland)),
                    requested(Capability::Wayland).into()
                ),
                cap_row(
                    "Clipboard",
                    row![clipboard("shared", "Shared", Clipboard::Shared), clipboard("off", "Off", Clipboard::Off)]
                        .spacing(4)
                        .into(),
                    small("Never read in the background").color(self.t.mu).into()
                ),
                cap_row(
                    "GPU",
                    toggle("sec:gpu", caps.gpu, change(&|n| n.capabilities.gpu = !n.capabilities.gpu)),
                    requested(Capability::Gpu).into()
                ),
                cap_row("Secret keyring", keyring.wrap().into(), requested(Capability::Keyring).into()),
                cap_row(
                    "Virtualization (KVM) !",
                    toggle("sec:kvm", caps.kvm, change(&|n| n.capabilities.kvm = !n.capabilities.kvm)),
                    requested(Capability::Kvm).into()
                ),
                cap_row(
                    "X11 apps",
                    row![x11("off", "Off", X11::Off), x11("isolated", "Isolated", X11::Isolated), x11("host", "! Host", X11::Host)]
                        .spacing(4)
                        .into(),
                    requested(Capability::X11).into()
                ),
                row![
                    small(
                        "Capability changes apply at the next start; GPU, keyring and X11 add packages to the image (a \
                         rebuild)."
                    )
                    .color(self.t.mu),
                    grow(),
                ],
            ]
            .spacing(10),
        );
        column![
            section(self, "Security & capabilities"),
            preset,
            capabilities
        ]
        .spacing(12)
        .into()
    }
}
