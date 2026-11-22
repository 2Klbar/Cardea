//! Network: the firewall of a sandbox. References, what blocked traffic gets, the three zones with their lists,
//! recently blocked destinations with an "Allow" shortcut.

use cardea_core::model::{Blocked, Proto, Rule, ZoneMode};
use cardea_core::state::State;
use cardea_proto::Summary;
use iced::widget::{column, container, row, text_input};
use iced::{Alignment, Font, Length};

use super::kit::{El, Size, bold, grow, heading, rule, section, small, txt};
use crate::app::{App, Message, NET_COMMENT, NET_PORT, NET_TO};
use crate::focus::Group;
use crate::style::{self, Kind, Tone};

const TITLES: [&str; 3] = ["Host (your PC)", "Local network", "Internet"];
/// What an empty list means, per zone, in allowlist then blocklist mode.
const EMPTY: [[(&str, bool); 2]; 3] = [
    [
        ("Empty list: no port on your PC is reachable.", false),
        ("! Empty list: every port on your PC is reachable.", true),
    ],
    [
        (
            "Empty list: no machine on the local network is reachable, not even the gateway.",
            false,
        ),
        ("! Empty list: the whole local network is reachable.", true),
    ],
    [
        ("Empty list: the internet is fully blocked.", false),
        ("Empty list: the whole internet is reachable.", false),
    ],
];

impl App {
    pub fn network_view<'a>(&'a self, s: &'a Summary) -> El<'a> {
        let Some(c) = self.configs.get(&s.id) else {
            return txt("Loading…").color(self.t.mu).into();
        };
        let fw = &c.firewall;
        let running = s.state == State::Running;
        let net = self.net.get(&s.id).filter(|_| running);
        let busy = s.task.is_some();
        let on = |m: Message| (!busy).then_some(m);

        let toggle = if fw.enabled {
            self.btn(
                "net:off",
                "Turn off…",
                Kind::Danger,
                on(Message::NetEnable(false)),
            )
        } else {
            self.btn(
                "net:on",
                "Turn on",
                Kind::Primary,
                on(Message::NetEnable(true)),
            )
        };
        let mut page = column![
            row![
                container(section(self, "Network (firewall)")).width(Length::Fill),
                if fw.enabled {
                    self.pill("● On", Tone::Ok)
                } else {
                    self.pill("! Off", Tone::Err)
                },
                small("Applied immediately, no restart").color(self.t.mu),
                toggle,
                self.btn(
                    "net:reset",
                    "Restore defaults",
                    Kind::Normal,
                    on(Message::NetReset)
                ),
            ]
            .spacing(8)
            .align_y(Alignment::End)
        ]
        .spacing(12);
        if !fw.enabled {
            page = page.push(
                self.banner(
                    Tone::Err,
                    row![
                        txt("!").color(self.t.err).font(bold()),
                        container(small(
                            "This sandbox’s firewall is off: the lists below do not apply."
                        ))
                        .width(Length::Fill),
                        self.btn(
                            "net:back-on",
                            "Turn back on",
                            Kind::Normal,
                            on(Message::NetEnable(true))
                        ),
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                ),
            );
        }

        // References as resolved now.
        let mut refs = row![
            small("Your PC’s network").font(bold()),
            grow(),
            small("References:").color(self.t.mu)
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        match net {
            Some(n) => {
                for (name, addrs) in &n.references {
                    let label = match name.as_str() {
                        "host" => "Your PC",
                        "gateway" => "Gateway",
                        _ => "DNS",
                    };
                    refs = refs.push(if addrs.is_empty() {
                        self.pill(format!("{label}: unresolved, blocked"), Tone::Warn)
                    } else {
                        self.pill(format!("{label} → {}", addrs.join(", ")), Tone::Neutral)
                    });
                }
            }
            None => refs = refs.push(small("shown while the sandbox runs").color(self.t.mu)),
        }
        page = page.push(self.card(refs.wrap()).padding([10, 14]));

        let answer = |key: &str, b: Blocked, label: &'a str| {
            self.btn(
                key,
                label,
                Kind::Nav(fw.blocked == b),
                on(Message::NetAnswer(b)),
            )
        };
        page = page.push(
            row![
                small("Blocked connections are").color(self.t.mu),
                answer("net:reject", Blocked::Reject, "Refused at once (reject)"),
                answer("net:drop", Blocked::Drop, "Dropped silently (drop)"),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        );
        page = page.push(
            small(
                "Pick a mode for each zone. Allowlist: everything is blocked except the entries. Blocklist: everything \
                 is allowed except the entries. Each mode keeps its own list.",
            )
            .color(self.t.mu),
        );
        for z in 0..3 {
            page = page.push(self.zone_card(z, &s.id, busy, net.map(|n| n.blocked_count[z])));
        }

        // Recently blocked destinations.
        let mut recent = column![heading("Recently blocked connections", style::H2)].spacing(8);
        match net {
            Some(n) if !n.blocked.is_empty() => {
                for (i, b) in n.blocked.iter().enumerate() {
                    let z = ["host", "lan", "wan"]
                        .iter()
                        .position(|n| *n == b.zone)
                        .unwrap_or(2);
                    let label = match self.zone(&s.id, z).map(|zn| zn.mode) {
                        Some(ZoneMode::Block) => "Unblock",
                        _ => "Allow",
                    };
                    let ago = if b.ago < 60 {
                        format!("{} s ago", b.ago)
                    } else {
                        format!("{} min ago", b.ago / 60)
                    };
                    recent = recent.push(rule(self)).push(
                        row![
                            container(small(ago).color(self.t.mu)).width(90),
                            container(small(TITLES[z])).width(Length::FillPortion(2)),
                            container(small(b.address.as_str()).font(Font::MONOSPACE))
                                .width(Length::FillPortion(3)),
                            self.btn(
                                &format!("net:allow:{i}"),
                                label,
                                Kind::Normal,
                                on(Message::NetAllow(z, b.address.clone()))
                            ),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center),
                    );
                }
            }
            Some(_) => {
                recent =
                    recent.push(small("Nothing blocked in the last 10 minutes.").color(self.t.mu))
            }
            None => {
                recent = recent
                    .push(small("Nothing recent: the sandbox is not running.").color(self.t.mu))
            }
        }
        page.push(self.card(recent))
            .push(
                small("i Your PC’s own firewall always applies on top of these rules.")
                    .color(self.t.mu),
            )
            .into()
    }

    fn zone(&self, id: &str, z: usize) -> Option<&cardea_core::model::Zone> {
        self.configs
            .get(id)
            .map(|c| [&c.firewall.host, &c.firewall.lan, &c.firewall.wan][z])
    }

    fn zone_card<'a>(&'a self, z: usize, id: &str, busy: bool, count: Option<u64>) -> El<'a> {
        let zone = self.zone(id, z).expect("config loaded");
        let allow = zone.mode == ZoneMode::Allow;
        let on = |m: Message| (!busy).then_some(m);
        let (list, other) = if allow {
            (&zone.allow, &zone.block)
        } else {
            (&zone.block, &zone.allow)
        };
        let mut card = column![
            row![
                heading(TITLES[z], style::H2),
                self.btn(
                    &format!("net:z{z}:allow"),
                    "Allowlist",
                    Kind::Nav(allow),
                    on(Message::NetMode(z, ZoneMode::Allow))
                ),
                self.btn(
                    &format!("net:z{z}:block"),
                    "Blocklist",
                    Kind::Nav(!allow),
                    on(Message::NetMode(z, ZoneMode::Block))
                ),
                grow(),
                small(count.map_or(String::new(), |n| format!("{n} blocked"))).color(self.t.mu),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
        ]
        .spacing(8);
        card = card.push(
            small(if allow {
                "Everything is blocked except:"
            } else {
                "Everything is allowed except:"
            })
            .font(bold()),
        );
        if list.is_empty() {
            let (text, warn) = EMPTY[z][usize::from(!allow)];
            card = card.push(small(text).color(if warn { self.t.warn } else { self.t.mu }));
        }
        let cell = |e: El<'a>, p: u16| container(e).width(Length::FillPortion(p));
        for (i, r) in list.iter().enumerate() {
            card = card.push(rule(self)).push(
                row![
                    cell(self.destination(id, r), 4),
                    cell(
                        small(r.port.map_or("all".into(), |p| p.to_string())).into(),
                        1
                    ),
                    cell(small(proto_label(r)).into(), 1),
                    cell(
                        small(r.comment.clone().unwrap_or_default())
                            .color(self.t.mu)
                            .into(),
                        3
                    ),
                    self.btn(
                        &format!("net:z{z}:remove:{i}"),
                        "✕",
                        Kind::Normal,
                        on(Message::NetRemove(z, i))
                    ),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        }
        let open = self.net_draft.as_ref().is_some_and(|d| d.zone == z);
        let add_label = if allow {
            "+ Add to allowlist"
        } else {
            "+ Add to blocklist"
        };
        card = card.push(
            row![
                self.btn(
                    &format!("net:z{z}:add"),
                    add_label,
                    Kind::Normal,
                    on(Message::NetOpen(z)).filter(|_| !open)
                ),
                small(if other.is_empty() {
                    String::new()
                } else {
                    format!(
                        "The {} is kept ({} entries), inactive.",
                        if allow { "blocklist" } else { "allowlist" },
                        other.len()
                    )
                })
                .color(self.t.mu),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
        if open {
            card = card.push(self.net_form(z));
        }
        self.card(card).into()
    }

    /// A reference shows its name and what it resolves to now.
    fn destination<'a>(&'a self, id: &str, r: &'a Rule) -> El<'a> {
        let label = match r.to.as_str() {
            "host" => Some("Your PC"),
            "gateway" => Some("Gateway"),
            "dns" => Some("DNS"),
            _ => None,
        };
        match label {
            Some(l) => {
                let addrs = self
                    .net
                    .get(id)
                    .and_then(|n| n.references.iter().find(|(name, _)| *name == r.to))
                    .map(|(_, a)| a.join(", "))
                    .unwrap_or_default();
                row![
                    small(l).font(bold()),
                    small(addrs).font(Font::MONOSPACE).color(self.t.mu)
                ]
                .spacing(6)
                .into()
            }
            None => small(r.to.as_str())
                .font(Font::MONOSPACE)
                .font(bold())
                .into(),
        }
    }

    fn net_form(&self, z: usize) -> El<'_> {
        let d = self.net_draft.as_ref().expect("checked by the caller");
        let t = self.t;
        let input = |key: &'static str,
                     value: &str,
                     placeholder: &'static str,
                     width: Length,
                     on: fn(String) -> Message| {
            let focused = self.target(key, Group::Main, None, Some(key));
            text_input(placeholder, value)
                .id(key)
                .on_input(on)
                .on_submit(Message::NetAdd)
                .size(style::SMALL)
                .padding([4, 8])
                .width(width)
                .style(move |_, status| style::input(t, focused, status))
        };
        let placeholder = match z {
            0 => "host",
            1 => "gateway, dns, 192.168.1.30 or 192.168.1.0/24",
            _ => "203.0.113.0/24 or 2001:db8::1",
        };
        let proto = |key: &str, p: Option<Proto>, label: &'static str| {
            self.btn(
                key,
                label,
                Kind::Nav(d.proto == p),
                Some(Message::NetProto(p)),
            )
        };
        let label = |l: &'static str| container(small(l)).width(110);
        let mut form = column![
            row![
                label("Destination"),
                input(NET_TO, &d.to, placeholder, Length::Fill, Message::NetTo)
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            row![
                label("Port (empty = all)"),
                input(
                    NET_PORT,
                    &d.port,
                    "all",
                    Length::Fixed(90.0),
                    Message::NetPort
                ),
                proto("net:tcp", Some(Proto::Tcp), "TCP"),
                proto("net:udp", Some(Proto::Udp), "UDP"),
                proto("net:both", None, "TCP+UDP"),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
            row![
                label("Comment"),
                input(
                    NET_COMMENT,
                    &d.comment,
                    "",
                    Length::Fill,
                    Message::NetComment
                )
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(8);
        if let Some(e) = &self.net_error {
            form = form.push(small(format!("✕ {e}")).color(self.t.err));
        }
        let ready = !d.to.trim().is_empty();
        form = form.push(
            row![
                grow(),
                self.btn(
                    "net:cancel",
                    "Cancel",
                    Kind::Normal,
                    Some(Message::NetCancel)
                ),
                self.button(
                    "net:confirm",
                    small("Add"),
                    Kind::Primary,
                    Size::Small,
                    ready.then_some(Message::NetAdd)
                ),
            ]
            .spacing(8),
        );
        container(form).padding(iced::padding::top(4)).into()
    }
}

fn proto_label(r: &Rule) -> &'static str {
    match (r.proto, r.port) {
        (Some(Proto::Tcp), _) => "TCP",
        (Some(Proto::Udp), _) => "UDP",
        (None, Some(_)) => "TCP+UDP",
        (None, None) => "all",
    }
}
