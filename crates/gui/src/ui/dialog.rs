//! Stop and Delete dialogs.

use iced::widget::{column, container, row};
use iced::{Alignment, Length};

use super::kit::{El, Size, bold, grow, heading, rule, small, txt};
use crate::app::{App, Dialog, Message};
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
        };
        let footer = row![
            grow(),
            self.btn("dlg:cancel", "Cancel", Kind::Normal, Some(Message::Cancel)),
            self.btn(
                "dlg:confirm",
                confirm.unwrap_or(match d {
                    Dialog::Stop(_) => "Stop",
                    Dialog::Delete { .. } => "Delete",
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
