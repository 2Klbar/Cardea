//! Shared folders: the table of folders with their access and status, the "Add a folder" card, the protected
//! paths.

use cardea_core::model::{Folder, Mode};
use cardea_core::state::State;
use cardea_proto::Summary;
use iced::widget::{column, container, row, text_input};
use iced::{Alignment, Font, Length};

use super::kit::{El, bold, grow, heading, rule, section, small, txt};
use crate::app::{App, FOLDER_HOST, FOLDER_OTHER, Message, PROTECT_NEW, Place};
use crate::focus::Group;
use crate::style::{self, Kind, Tone};

impl App {
    pub fn folders_view<'a>(&'a self, s: &'a Summary) -> El<'a> {
        let id = &s.id;
        let Some(c) = self.configs.get(id) else {
            return txt("Loading…").color(self.t.mu).into();
        };
        let missing = self.missing.get(id);
        let busy = s.task.is_some();
        let add = (self.folder_draft.is_none() && !busy).then(|| Message::FolderOpen);
        let mut page = column![
            row![
                container(section(self, "Shared folders")).width(Length::Fill),
                self.btn("folders:add", "+ Add folder…", Kind::Primary, add),
            ]
            .spacing(8)
            .align_y(Alignment::End)
        ]
        .spacing(12);

        let cell = |e: El<'a>, portion: u16| container(e).width(Length::FillPortion(portion));
        let head = |label: &'a str| small(label).color(self.t.mu).font(bold());
        let mut table = column![
            row![
                cell(head("On your PC").into(), 3),
                cell(head("In the sandbox").into(), 3),
                cell(head("Access").into(), 2),
                cell(head("Status").into(), 2),
                container(small("")).width(70),
            ]
            .spacing(8)
        ]
        .spacing(8);
        let pending = s.flags.restart_required && s.state == State::Running;
        for (i, f) in c.folders.iter().enumerate() {
            let gone = missing.is_some_and(|m| m.contains(&f.host));
            let (access, other) = match f.mode {
                Mode::Rw => ("Read-write", Mode::Ro),
                Mode::Ro => ("Read-only", Mode::Rw),
            };
            let toggle = (!busy).then(|| Message::FolderMode(f.mount.clone(), other));
            let status: El = if gone {
                small("✕ Missing").color(self.t.err).into()
            } else if pending {
                row![
                    small("✓ Found").color(self.t.ok),
                    self.pill("↻ at restart", Tone::Warn)
                ]
                .spacing(6)
                .wrap()
                .into()
            } else {
                small("✓ Found").color(self.t.ok).into()
            };
            table = table.push(rule(self)).push(
                row![
                    cell(
                        small(self.paths_home(&f.host)).font(Font::MONOSPACE).into(),
                        3
                    ),
                    cell(small(f.mount.as_str()).font(Font::MONOSPACE).into(), 3),
                    cell(
                        self.btn(&format!("folders:mode:{i}"), access, Kind::Normal, toggle),
                        2
                    ),
                    cell(status, 2),
                    container(self.btn(
                        &format!("folders:remove:{i}"),
                        "Remove",
                        Kind::Normal,
                        (!busy).then(|| Message::FolderRemove(f.mount.clone())),
                    ))
                    .width(70),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        }
        if c.folders.is_empty() {
            table = table
                .push(rule(self))
                .push(small("No shared folder.").color(self.t.mu));
        }
        page = page.push(table);
        let when = if s.state == State::Running {
            "at the next start"
        } else {
            "when the sandbox starts"
        };
        page = page.push(
            small(format!(
                "Changes apply {when}. Read-write folders keep their protected paths read-only. Run builds inside the \
                 sandbox: code written here can run later on your computer."
            ))
            .color(self.t.mu),
        );
        if self.folder_draft.is_some() {
            page = page.push(self.add_card());
        }
        page.push(self.protected_card(&c.protected_paths.names, busy))
            .into()
    }

    fn add_card(&self) -> El<'_> {
        let d = self.folder_draft.as_ref().expect("checked by the caller");
        let t = self.t;
        let input = |key: &'static str,
                     value: &str,
                     placeholder: &'static str,
                     on: fn(String) -> Message| {
            let focused = self.target(key, Group::Main, None, Some(key));
            text_input(placeholder, value)
                .id(key)
                .on_input(on)
                .font(Font::MONOSPACE)
                .size(style::BASE)
                .padding([6, 10])
                .width(Length::Fill)
                .style(move |_, status| style::input(t, focused, status))
        };
        let host = self.expand(&d.host);
        let default = Folder::default_mount(&host);
        let choice = |key: &str, place: Place, label: String| {
            let on = d.place == place;
            self.button(
                key,
                row![txt(if on { "◉" } else { "○" }), small(label)]
                    .spacing(8)
                    .align_y(Alignment::Center),
                Kind::Nav(on),
                super::kit::Size::Small,
                Some(Message::FolderPlace(place)),
            )
            .width(Length::Fill)
        };
        let mut places = column![
            choice(
                "folders:place:default",
                Place::Default,
                format!("{default}  · default")
            ),
            choice(
                "folders:place:same",
                Place::Same,
                format!("Same path as on your PC  {host}")
            ),
            choice("folders:place:other", Place::Other, "Somewhere else".into()),
        ]
        .spacing(4);
        if d.place == Place::Other {
            places = places.push(input(
                FOLDER_OTHER,
                &d.other,
                "/data/name",
                Message::FolderOther,
            ));
        }
        places = places.push(
            small("Absolute path. Not allowed: /usr, /etc, /bin, /lib, /proc, /sys, /dev, or a path already in use.")
                .color(self.t.mu),
        );
        let access = row![
            self.btn(
                "folders:ro",
                "Read-only",
                Kind::Nav(d.ro),
                Some(Message::FolderRo(true))
            ),
            self.btn(
                "folders:rw",
                "Read-write",
                Kind::Nav(!d.ro),
                Some(Message::FolderRo(false))
            ),
        ]
        .spacing(6);
        let label = |l: &'static str| container(txt(l)).width(130);
        let mut body = column![
            heading("Add a folder", style::H2),
            row![
                label("On your PC"),
                input(FOLDER_HOST, &d.host, "~/Projects/site", Message::FolderHost)
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            row![label("In the sandbox"), places].spacing(8),
            row![label("Access"), access]
                .spacing(8)
                .align_y(Alignment::Center),
        ]
        .spacing(12);
        if let Some(e) = &self.folder_error {
            body = body.push(small(format!("✕ {e}")).color(self.t.err));
        }
        let ready =
            !d.host.trim().is_empty() && (d.place != Place::Other || !d.other.trim().is_empty());
        body = body.push(
            row![
                grow(),
                self.btn(
                    "folders:cancel",
                    "Cancel",
                    Kind::Normal,
                    Some(Message::FolderCancel)
                ),
                self.btn(
                    "folders:confirm",
                    "Add",
                    Kind::Primary,
                    ready.then_some(Message::FolderAdd)
                ),
            ]
            .spacing(8),
        );
        self.card(body).into()
    }

    fn protected_card<'a>(&'a self, names: &'a [String], busy: bool) -> El<'a> {
        let t = self.t;
        let mut pills = row![].spacing(6).align_y(Alignment::Center);
        for (i, n) in names.iter().enumerate() {
            pills = pills.push(
                self.button(
                    &format!("protect:remove:{i}"),
                    row![
                        small(n.as_str()).font(Font::MONOSPACE),
                        small("×").color(self.t.mu)
                    ]
                    .spacing(6),
                    Kind::Normal,
                    super::kit::Size::Small,
                    (!busy).then(|| Message::ProtectRemove(n.clone())),
                ),
            );
        }
        let focused = self.target(PROTECT_NEW, Group::Main, None, Some(PROTECT_NEW));
        let new = text_input(".env", &self.protect_new)
            .id(PROTECT_NEW)
            .on_input(Message::ProtectInput)
            .on_submit(Message::ProtectAdd)
            .font(Font::MONOSPACE)
            .size(style::SMALL)
            .padding([4, 8])
            .width(140)
            .style(move |_, status| style::input(t, focused, status));
        let add = (!busy && !self.protect_new.trim().is_empty()).then_some(Message::ProtectAdd);
        self.card(
            column![
                heading("Protected paths", style::H2),
                small("Always read-only, even inside a read-write folder. Found down to the folder depth set in the \
                       configuration; select one to remove it.")
                .color(self.t.mu),
                pills.wrap(),
                row![new, self.btn("protect:add", "+ Add", Kind::Normal, add)]
                    .spacing(6)
                    .align_y(Alignment::Center),
            ]
            .spacing(8),
        )
        .into()
    }

    /// `~/` for paths in the user's home.
    fn paths_home(&self, path: &str) -> String {
        match std::env::var("HOME") {
            Ok(home) if !home.is_empty() => match path.strip_prefix(&home) {
                Some(rest) if rest.starts_with('/') => format!("~{rest}"),
                _ => path.to_owned(),
            },
            _ => path.to_owned(),
        }
    }

    /// A typed host path: a leading `~/` is the user's home.
    pub fn expand(&self, path: &str) -> String {
        let path = path.trim();
        match (path.strip_prefix("~/"), std::env::var("HOME")) {
            (Some(rest), Ok(home)) => format!("{}/{rest}", home.trim_end_matches('/')),
            _ => path.trim_end_matches('/').to_owned(),
        }
    }
}
