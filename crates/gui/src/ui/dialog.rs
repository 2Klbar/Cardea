//! Stop and Delete dialogs.

use iced::widget::{column, container, row, text_input};
use iced::{Alignment, Length};

use super::kit::{El, Size, bold, grow, heading, rule, small, txt};
use crate::app::{App, Dialog, Message, PASSWORD, Risk};

/// How to run an app on Wayland before giving it the host's X server.
const GUIDE: [(&str, &str); 8] = [
    ("Qt 5 / 6", "interface type Qt"),
    ("GTK 3 / 4", "interface type GTK"),
    ("Electron, Chromium", "interface type Electron"),
    (
        "Firefox, Thunderbird",
        "MOZ_ENABLE_WAYLAND=1 (default in recent versions)",
    ),
    ("SDL 2 / 3 (games)", "SDL_VIDEODRIVER=wayland"),
    ("GLFW, winit, Flutter", "usually Wayland already"),
    ("Java (Swing, JavaFX)", "X11 isolated"),
    ("Wine, Motif, Tk, Xlib", "X11 isolated"),
];
use crate::style::{self, Kind, Tone};

impl App {
    pub fn dialog_view<'a>(&'a self, d: &'a Dialog) -> El<'a> {
        let t = self.t;
        let (title, body, confirm): (String, El<'a>, Option<&str>) = match d {
            Dialog::Stop(id) => {
                let name = self.summary(id).map_or(id.as_str(), |s| s.name.as_str());
                let body = column![
                    txt("Apps open in it will be closed."),
                    small("Files in the home and in shared folders are kept.").color(t.mu),
                ]
                .spacing(8);
                (format!("Stop “{name}”?"), body.into(), Some("Stop"))
            }
            Dialog::Delete { id, keep } => {
                let s = self.summary(id);
                let name = s.map_or(id.as_str(), |s| s.name.as_str());
                let deletable =
                    s.is_some_and(|s| Self::allowed(s, cardea_core::state::Action::Delete));
                let folder = s.map(|s| s.folder.clone()).unwrap_or_default();
                let mut body = column![].spacing(8);
                if !deletable {
                    body = body.push(self.banner(
                        Tone::Warn,
                        small(
                            "It cannot be deleted now: stop it first, or wait for its task to end.",
                        ),
                    ));
                }
                let option = |key: &str, on: bool, title: &'a str, detail: El<'a>, msg| {
                    self.button(
                        key,
                        row![
                            txt(if on { "◉" } else { "○" }).color(if on { t.ac } else { t.mu }),
                            column![txt(title), detail].spacing(2)
                        ]
                        .spacing(10),
                        Kind::Choice(on),
                        Size::Normal,
                        Some(msg),
                    )
                    .width(Length::Fill)
                };
                body = body
                    .push(option(
                        "dlg:keep",
                        *keep,
                        "Keep its folder",
                        small(format!("{folder} · reusable later"))
                            .color(t.mu)
                            .into(),
                        Message::KeepHome(true),
                    ))
                    .push(option(
                        "dlg:wipe",
                        !*keep,
                        "Also delete its folder and home",
                        small("Permanent").color(t.err).into(),
                        Message::KeepHome(false),
                    ));
                let shared: Vec<String> = self
                    .configs
                    .get(id)
                    .map(|c| c.folders.iter().map(|f| f.host.clone()).collect())
                    .unwrap_or_default();
                body = body.push(
                    small(if shared.is_empty() {
                        "Shared folders are never touched.".to_owned()
                    } else {
                        format!(
                            "Your shared folders ({}) are never touched.",
                            shared.join(", ")
                        )
                    })
                    .color(t.mu),
                );
                (
                    format!("Delete the “{name}” sandbox?"),
                    body.into(),
                    deletable.then_some("Delete"),
                )
            }
            Dialog::Risk { risk, ack, .. } => {
                let (title, what, advice, button) = match risk {
                    Risk::FirewallOff => (
                        "Turn off the firewall?",
                        "The sandbox will reach your whole PC, the whole local network and the whole internet, \
                         whatever the lists on the Network page say.",
                        "Only do this while troubleshooting.",
                        "Turn off firewall",
                    ),
                    Risk::Blocklist(0) => (
                        "Use a blocklist for your PC?",
                        "Every service on your PC becomes reachable from the sandbox, except the ones you list.",
                        "An allowlist with only the ports you need is safer.",
                        "Use blocklist",
                    ),
                    Risk::Blocklist(_) => (
                        "Use a blocklist for the local network?",
                        "Every device on your network (printers, NAS, routers…) becomes reachable from the sandbox, \
                         except the ones you list.",
                        "An allowlist with only the devices you need is safer.",
                        "Use blocklist",
                    ),
                };
                let body = column![
                    self.banner(Tone::Warn, small(what)),
                    small(advice),
                    self.button(
                        "dlg:ack",
                        row![
                            txt(if *ack { "☑" } else { "☐" }),
                            small("I understand the risk")
                        ]
                        .spacing(8),
                        Kind::Choice(*ack),
                        Size::Small,
                        Some(Message::RiskAck(!*ack)),
                    ),
                ]
                .spacing(10);
                (format!("! {title}"), body.into(), ack.then_some(button))
            }
            Dialog::Change { warnings, ack, .. } => {
                let mut body = column![].spacing(10);
                for w in warnings {
                    body = body.push(self.banner(Tone::Warn, small(w.clone())));
                }
                if warnings.iter().any(|w| w.starts_with("host X11")) {
                    body = body.push(small("Try first to run the app on Wayland:").font(bold()));
                    for (made, try_) in GUIDE {
                        body = body.push(
                            row![container(small(made)).width(170), small(try_).color(t.mu)]
                                .spacing(8),
                        );
                    }
                    body = body.push(
                        small("`cardea app <id> <app> --test-wayland` runs the app once without X server.").color(t.mu),
                    );
                }
                body = body.push(
                    self.button(
                        "dlg:ack",
                        row![
                            txt(if *ack { "☑" } else { "☐" }),
                            small("I understand the risk")
                        ]
                        .spacing(8),
                        Kind::Choice(*ack),
                        Size::Small,
                        Some(Message::RiskAck(!*ack)),
                    ),
                );
                (
                    "! Turn on a risky setting?".to_owned(),
                    body.into(),
                    ack.then_some("Turn on"),
                )
            }
            Dialog::Password { pending, value } => {
                let focused =
                    self.target(PASSWORD, crate::focus::Group::Main, None, Some(PASSWORD));
                let field = text_input("Password", value)
                    .id(PASSWORD)
                    .secure(true)
                    .on_input(Message::PasswordInput)
                    .on_submit(Message::Confirm)
                    .size(style::BASE)
                    .padding([6, 10])
                    .style(move |_, status| style::input(t, focused, status));
                let name = self
                    .summary(pending.id())
                    .map_or(pending.id(), |s| s.name.as_str());
                let body = column![
                    small("Its keyring asks for its password at each start. It is not stored anywhere."),
                    field,
                ]
                .spacing(10);
                (
                    format!("Unlock the keyring of “{name}”"),
                    body.into(),
                    Some("Unlock"),
                )
            }
        };
        let footer = row![
            grow(),
            self.btn("dlg:cancel", "Cancel", Kind::Normal, Some(Message::Cancel)),
            self.btn(
                "dlg:confirm",
                confirm.unwrap_or(match d {
                    Dialog::Stop(_) => "Stop",
                    Dialog::Delete { .. } => "Delete",
                    Dialog::Risk { .. } | Dialog::Change { .. } => "Turn on",
                    Dialog::Password { .. } => "Unlock",
                }),
                Kind::DangerFill,
                confirm.map(|_| Message::Confirm),
            ),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        container(
            column![
                container(heading(title, 15.0).font(bold()))
                    .padding(iced::padding::top(14).left(18).right(18)),
                container(body).padding([12, 18]),
                rule(self),
                container(footer).padding([12, 18]),
            ]
            .width(480),
        )
        .style(move |_| container::Style {
            background: Some(t.surface.into()),
            border: style::border(t.bd, 1.0, 10.0),
            text_color: Some(t.tx),
            shadow: iced::Shadow {
                color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.3),
                offset: iced::Vector::new(0.0, 10.0),
                blur_radius: 30.0,
            },
            ..Default::default()
        })
        .into()
    }
}
