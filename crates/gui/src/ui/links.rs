//! Links: the schemes the sandbox's apps open (incoming) and the links its apps may send to your browser
//! (outgoing). Changes are configuration saves (`Message::Change`).

use cardea_core::model::{Config, Trust};
use cardea_core::state::State;
use cardea_proto::{SchemeStatus, Summary};
use iced::widget::{column, container, row};
use iced::{Alignment, Length};

use super::kit::{El, bold, heading, rule, small};
use crate::app::{App, LINK_HOST, Message};
use crate::style::{self, Kind, Tone};

impl App {
    pub fn links_view<'a>(&'a self, s: &'a Summary) -> El<'a> {
        let Some(c) = self.configs.get(&s.id) else {
            return small("Loading…").color(self.t.mu).into();
        };
        let busy = s.task.is_some();
        let change = |f: &dyn Fn(&mut Config)| {
            let mut new = c.clone();
            f(&mut new);
            (!busy).then(|| Message::Change(Box::new(new)))
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

        // Incoming.
        let mut incoming = column![
            heading("Incoming links", style::H2),
            small("A link of one of these schemes, clicked on this computer, opens in the sandbox's app.")
                .color(self.t.mu),
        ]
        .spacing(8);
        match self.schemes.get(&s.id) {
            None => incoming = incoming.push(small("Loading…").color(self.t.mu)),
            Some(list) if list.is_empty() => {
                incoming = incoming.push(
                    small("No app of this sandbox declares a scheme. A profile gives schemes to its apps.")
                        .color(self.t.mu),
                )
            }
            Some(list) => {
                for i in list {
                    let (pill, tone, note) = match &i.status {
                        SchemeStatus::Active => ("● Opens here".to_owned(), Tone::Ok, String::new()),
                        SchemeStatus::HostApp(app) => (
                            "Handled by a host app".to_owned(),
                            Tone::Warn,
                            format!(
                                "{app} opens these links; Cardea leaves it. To change, choose in your desktop's \
                                 default apps settings."
                            ),
                        ),
                        SchemeStatus::OtherSandbox(name) => (
                            format!("Handled by {name}"),
                            Tone::Warn,
                            "Another sandbox declared it first. Choose in your desktop's default apps settings."
                                .to_owned(),
                        ),
                        SchemeStatus::Unknown => (
                            "No handler known".to_owned(),
                            Tone::Neutral,
                            "This computer reports no default app for it (xdg-mime missing?).".to_owned(),
                        ),
                    };
                    incoming = incoming.push(
                        row![
                            container(small(format!("{}:", i.scheme)).font(bold())).width(120),
                            container(small(i.app_name.as_str())).width(140),
                            self.pill(pill, tone),
                            small(note).color(self.t.mu),
                        ]
                        .spacing(10)
                        .align_y(Alignment::Center),
                    );
                }
            }
        }

        // Outgoing.
        let l = &c.links;
        let untrusted = c.trust == Trust::Untrusted;
        let pending = s.flags.restart_required && s.state == State::Running;
        let mut outgoing = column![
            heading("Outgoing links", style::H2),
            small(
                "Links the sandbox's apps open (xdg-open) go to your default browser, after the checks; \
                 http and https only."
            )
            .color(self.t.mu),
            rule(self),
            line(
                "Open links in your browser",
                toggle(
                    "links:outgoing",
                    l.outgoing,
                    change(&|n| n.links.outgoing = !n.links.outgoing)
                ),
                if pending {
                    self.pill("↻ at restart", Tone::Warn)
                } else {
                    small("Applies at the next start.").color(self.t.mu).into()
                },
            ),
        ]
        .spacing(10);
        if l.outgoing {
            outgoing = outgoing.push(line(
                "Ask before opening",
                if untrusted {
                    self.pill("! Always", Tone::Err)
                } else {
                    toggle("links:ask", l.ask, change(&|n| n.links.ask = !n.links.ask))
                },
                small(if untrusted {
                    "Untrusted sandbox: every link is asked, in a red window."
                } else {
                    "A suspicious link is always asked."
                })
                .color(self.t.mu)
                .into(),
            ));
            if !untrusted {
                let mut hosts = column![
                    small("Opened without asking").font(bold()),
                    small("Exact hosts, for this sandbox only: github.com does not cover gist.github.com.")
                        .color(self.t.mu),
                ]
                .spacing(6);
                if l.allowed.is_empty() {
                    hosts = hosts.push(small("None yet.").color(self.t.mu));
                }
                for (i, h) in l.allowed.iter().enumerate() {
                    let host = h.clone();
                    hosts = hosts.push(
                        row![
                            container(small(h.as_str())).width(Length::Fixed(320.0)),
                            self.btn(
                                &format!("links:rm:{i}"),
                                "Remove",
                                Kind::Normal,
                                change(&|n| n.links.allowed.retain(|x| *x != host)),
                            ),
                        ]
                        .spacing(10)
                        .align_y(Alignment::Center),
                    );
                }
                hosts = hosts.push(
                    row![
                        self.field(LINK_HOST, "github.com", Length::Fixed(320.0)),
                        self.btn(
                            "links:add",
                            "Add",
                            Kind::Primary,
                            (!busy && !self.value(LINK_HOST).trim().is_empty())
                                .then_some(Message::LinkAllow),
                        ),
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                );
                outgoing = outgoing.push(hosts);
            }
        }

        column![self.card(incoming), self.card(outgoing)]
            .spacing(14)
            .into()
    }
}
