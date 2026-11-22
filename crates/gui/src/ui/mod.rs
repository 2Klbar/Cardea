//! The window: sidebar, page, dialogs.

mod all;
mod apps;
mod dialog;
mod folders;
pub mod kit;
mod links;
mod log;
mod manage;
mod network;
mod overview;
mod packages;
mod sandbox;
mod security;
mod spark;

use cardea_core::model::Trust;
use cardea_core::state::State;
use cardea_proto::Summary;
use iced::widget::{Space, column, container, opaque, row, stack};
use iced::{Alignment, Color, Length};

use crate::app::{App, Link, MAIN_SCROLL, Message, Page, Section};
use crate::style::{self, Kind, Tone};
use kit::{El, Size, bold, grow, icon, rule, small, txt};

/// How a state looks: glyph, pill text, tone.
pub fn look(s: &Summary) -> (&'static str, String, Tone) {
    let pct = s.task.as_ref().map(|t| t.percent);
    match s.state {
        State::Building => (
            "◌",
            format!("◌ Building · {} %", pct.unwrap_or(0)),
            Tone::Ac,
        ),
        State::BuildFailed => ("✕", "✕ Build failed".into(), Tone::Err),
        State::Stopped => ("○", "○ Stopped".into(), Tone::Neutral),
        State::Starting => ("◌", "◌ Starting…".into(), Tone::Ac),
        State::Running => ("●", "● Running".into(), Tone::Ok),
        State::Stopping => ("◌", "◌ Stopping…".into(), Tone::Neutral),
        State::Rebuilding => (
            "◌",
            format!("◌ Rebuilding · {} %", pct.unwrap_or(0)),
            Tone::Ac,
        ),
        State::Error => ("✕", "✕ Error".into(), Tone::Err),
    }
}

/// The short form of a warning: "firewall off: …" gives "Firewall off".
pub fn short_warning(w: &str) -> String {
    let head = w.split(':').next().unwrap_or(w).trim();
    let mut c = head.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

impl App {
    pub fn view(&self) -> El<'_> {
        self.targets.borrow_mut().clear();
        self.layer.set(0);
        let side = self.sidebar();
        let page = self.page_view();
        let t = self.t;
        let base: El = row![
            container(side)
                .width(236)
                .height(Length::Fill)
                .style(move |_| container::Style {
                    background: Some(t.surface.into()),
                    border: style::border(t.bd, 1.0, 0.0),
                    ..Default::default()
                }),
            page
        ]
        .into();
        let base = container(base)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_| container::Style {
                background: Some(t.bg.into()),
                text_color: Some(t.tx),
                ..Default::default()
            });
        match &self.dialog {
            None => base.into(),
            Some(d) => {
                self.layer.set(1);
                let dlg = self.dialog_view(d);
                let scrim = container(dlg)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(Alignment::Center)
                    .align_y(Alignment::Center)
                    .style(|_| style::fill(Color::from_rgba(0.0, 0.0, 0.0, 0.45), 0.0));
                stack![base, opaque(scrim)].into()
            }
        }
    }

    fn sidebar(&self) -> El<'_> {
        let mut top = column![
            container(
                self.button(
                    "side:new",
                    container(txt("+ New sandbox").font(bold())).center_x(Length::Fill),
                    Kind::Primary,
                    Size::Normal,
                    Some(Message::Go(Page::New)),
                )
                .width(Length::Fill)
            )
            .padding(iced::padding::bottom(8)),
            self.nav(
                "side:all",
                row![
                    txt("▦"),
                    txt("All sandboxes"),
                    grow(),
                    small(self.sandboxes.len().to_string()).color(self.t.mu)
                ],
                self.page == Page::All,
                Message::Go(Page::All)
            ),
            container(small("SANDBOXES").font(bold()).color(self.t.mu)).padding([12, 8]),
        ]
        .spacing(1);
        if self.sandboxes.is_empty() {
            top = top.push(container(small("No sandbox yet.").color(self.t.mu)).padding([4, 8]));
        }
        for s in &self.sandboxes {
            let open = matches!(&self.page, Page::Sandbox(id, _) if *id == s.id);
            let (glyph, _, tone) = look(s);
            let (_, glyph_color) = self.t.tone(tone);
            let (meta, meta_warn) = match (s.state, &s.task) {
                (State::Building | State::Rebuilding, Some(t)) => {
                    (format!("{} %", t.percent), false)
                }
                (State::Starting, _) => ("starting".into(), false),
                (State::Stopping, _) => ("stopping".into(), false),
                _ if s.flags.restart_required || s.flags.rebuild_required => ("↻".into(), true),
                _ => (String::new(), false),
            };
            let entry = row![
                small(if open { "▾" } else { "▸" })
                    .color(self.t.mu)
                    .width(10),
                icon(s.icon.as_ref(), &s.name, 18.0),
                txt(s.name.clone()).font(if open { bold() } else { kit::font() }),
                grow(),
                small(meta).color(if meta_warn { self.t.warn } else { self.t.mu }),
                container(small(glyph).color(glyph_color))
                    .width(14)
                    .center_x(14),
            ];
            top = top.push(self.nav(
                &format!("side:sb:{}", s.id),
                entry,
                false,
                Message::Go(Page::Sandbox(s.id.clone(), Section::Overview)),
            ));
            if let Page::Sandbox(id, current) = &self.page
                && *id == s.id
            {
                let mut sub = column![].spacing(1);
                for (n, sec) in Section::ALL.into_iter().enumerate() {
                    sub = sub.push(self.nav(
                        &format!("side:sec:{}:{n}", s.id),
                        row![small(sec.label())],
                        *current == sec,
                        Message::Go(Page::Sandbox(s.id.clone(), sec)),
                    ));
                }
                let bd = self.t.bd;
                top = top.push(
                    row![
                        Space::new().width(17),
                        container(Space::new())
                            .width(1)
                            .height(Length::Fill)
                            .style(move |_| style::fill(bd, 0.0)),
                        container(sub).padding(iced::padding::left(6))
                    ]
                    .height(Length::Shrink),
                );
            }
        }
        column![
            kit::scroll(top).height(Length::Fill),
            rule(self),
            self.nav(
                "side:profiles",
                row![txt("◈"), txt("Profiles")],
                self.page == Page::Profiles,
                Message::Go(Page::Profiles)
            ),
            self.nav(
                "side:settings",
                row![txt("⚙"), txt("Settings")],
                self.page == Page::Settings,
                Message::Go(Page::Settings)
            ),
        ]
        .spacing(6)
        .padding([10, 8])
        .into()
    }

    fn nav<'a>(
        &self,
        key: &str,
        content: iced::widget::Row<'a, Message>,
        on: bool,
        msg: Message,
    ) -> El<'a> {
        self.button(
            key,
            content.spacing(8).align_y(Alignment::Center),
            Kind::Nav(on),
            Size::Normal,
            Some(msg),
        )
        .width(Length::Fill)
        .into()
    }

    fn page_view(&self) -> El<'_> {
        let mut top = column![].spacing(12);
        if let Link::Lost(e) = &self.link {
            top = top.push(
                self.banner(
                    Tone::Err,
                    row![
                        txt("✕").color(self.t.err),
                        txt(format!("Lost the daemon: {e}. Reconnecting…"))
                    ]
                    .spacing(10),
                ),
            );
        } else if matches!(self.link, Link::Connecting) {
            top = top.push(self.banner(Tone::Ac, txt("Connecting to the daemon…")));
        }
        if let Some(n) = &self.notice {
            top = top.push(
                self.banner(
                    Tone::Err,
                    row![
                        txt("✕").color(self.t.err),
                        container(txt(n.clone())).width(Length::Fill),
                        self.btn(
                            "notice:dismiss",
                            "Dismiss",
                            Kind::Normal,
                            Some(Message::Dismiss)
                        ),
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                ),
            );
        }
        let (body, scrolls) = match &self.page {
            Page::All => (self.all_view(), true),
            Page::Sandbox(id, section) => match self.summary(id) {
                Some(s) => (self.sandbox_view(s, *section), *section != Section::Log),
                None => (txt("This sandbox no longer exists.").into(), true),
            },
            Page::New => (self.wizard_view(), true),
            Page::Profiles => (self.profiles_view(), true),
            Page::Profile(id) => (self.profile_view(id), true),
            Page::Import => (self.import_view(), true),
            Page::Settings => (self.settings_view(), true),
        };
        let content = top
            .push(body)
            .padding(iced::padding::top(14).right(24).bottom(24).left(24));
        if scrolls {
            kit::scroll(content)
                .id(MAIN_SCROLL)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else {
            container(content)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        }
    }

    pub fn trust_pill<'a>(&self, trust: Trust) -> El<'a> {
        match trust {
            Trust::Work => self.pill("Work", Tone::Neutral),
            Trust::Untrusted => self.pill("! Untrusted", Tone::Warn),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn short_warnings() {
        assert_eq!(
            super::short_warning("firewall off: the sandbox reaches your computer"),
            "Firewall off"
        );
        assert_eq!(super::short_warning("KVM on"), "KVM on");
    }
}
