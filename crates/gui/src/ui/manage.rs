//! Creating and managing: the New sandbox wizard, Profiles (list, detail, import), Settings, the requirements
//! check (Welcome and Settings), and a sandbox's Advanced page.

use cardea_core::model::{Config, Mode, Theme, Trust};
use cardea_core::packages::BASE_IMAGES;
use cardea_core::profile::review;
use cardea_core::state::State;
use cardea_proto::{CheckStatus, Summary};
use iced::widget::{column, container, row, text};
use iced::{Alignment, Font, Length};

use super::all::gb;
use super::kit::{El, bold, grow, heading, icon, rule, section, small, txt};
use crate::app::{
    ADV_DUP, ADV_EXPORT, ADV_LANGS, ADV_PROFILE, ADV_PROFILE_NAME, ADV_RAM, App, COLORS,
    EXPORT_PATH, IMPORT_PATH, Message, Page, SET_LOCATION, SET_TERMINAL, Section, WIZ_BUILD,
    WIZ_FIRST, WIZ_FOLDER, WIZ_IMAGE, WIZ_LOCATION, WIZ_NAME,
};
use crate::style::{self, Kind, Tone};

const STEPS: [&str; 8] = [
    "1 · Profile",
    "2 · Base image",
    "3 · Name & location",
    "4 · Trust",
    "5 · Folders",
    "6 · Setup script",
    "7 · Summary",
    "8 · Build",
];

/// A short summary of a configuration, as in the wizard and the profile cards.
fn summary_lines(c: &Config) -> Vec<(&'static str, String)> {
    let base = BASE_IMAGES
        .iter()
        .find(|(_, r)| *r == c.base_image)
        .map_or(c.base_image.clone(), |(n, _)| (*n).to_owned());
    let apps: Vec<&str> = c.apps.iter().map(|a| a.name.as_str()).collect();
    let mut caps = Vec::new();
    if c.capabilities.wayland {
        caps.push("Display");
    }
    if c.capabilities.gpu {
        caps.push("GPU");
    }
    if c.capabilities.keyring.mode != cardea_core::model::KeyringMode::Off {
        caps.push("Keyring");
    }
    if c.capabilities.kvm {
        caps.push("KVM");
    }
    let lines = |s: &str| {
        s.lines()
            .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
            .count()
    };
    vec![
        ("Base image", base),
        (
            "Apps",
            if apps.is_empty() {
                "none".into()
            } else {
                apps.join(", ")
            },
        ),
        ("Packages", c.packages.len().to_string()),
        (
            "Setup script",
            format!(
                "{} build commands · {} first-start commands",
                lines(&c.setup.build),
                lines(&c.setup.first_start)
            ),
        ),
        (
            "Capabilities",
            if caps.is_empty() {
                "none".into()
            } else {
                caps.join(", ")
            },
        ),
        ("Trust", format!("{:?}", c.trust)),
    ]
}

impl App {
    fn kv<'a>(&self, rows: Vec<(&'static str, String)>) -> El<'a> {
        let mut col = column![].spacing(6);
        for (k, v) in rows {
            col = col
                .push(row![container(small(k).color(self.t.mu)).width(140), small(v)].spacing(8));
        }
        col.into()
    }

    /// The requirements check: one line per check, with what to do under the failing ones.
    pub fn checks_view(&self) -> El<'_> {
        let Some(checks) = &self.checks else {
            return small("Checking…").color(self.t.mu).into();
        };
        let mut col = column![].spacing(6);
        for c in checks {
            let (glyph, color) = match c.status {
                CheckStatus::Ok => ("✓", self.t.ok),
                CheckStatus::Warn => ("!", self.t.warn),
                CheckStatus::Fail => ("✕", self.t.err),
            };
            col = col.push(
                row![
                    small(glyph).color(color).font(bold()).width(14),
                    container(small(c.name.as_str()).font(bold())).width(130),
                    small(c.detail.as_str())
                ]
                .spacing(8),
            );
            for h in c.hint.iter().flat_map(|h| h.lines()) {
                col = col.push(
                    row![
                        container(small("")).width(152),
                        small(format!("→ {h}")).color(self.t.mu)
                    ]
                    .spacing(8),
                );
            }
        }
        col.into()
    }

    fn report(&self) -> String {
        self.checks
            .iter()
            .flatten()
            .map(|c| {
                format!(
                    "{:?}\t{}\t{}{}",
                    c.status,
                    c.name,
                    c.detail,
                    c.hint
                        .as_ref()
                        .map(|h| format!("\n\t→ {h}"))
                        .unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Welcome: what the computer lacks, before the first sandbox.
    pub fn welcome_card(&self) -> El<'_> {
        self.card(
            column![
                row![
                    heading("Your computer", style::H2),
                    grow(),
                    self.btn(
                        "welcome:check",
                        "Check again",
                        Kind::Normal,
                        Some(Message::RunChecks)
                    )
                ]
                .align_y(Alignment::Center),
                self.checks_view(),
            ]
            .spacing(10),
        )
        .max_width(760)
        .into()
    }

    pub fn wizard_view(&self) -> El<'_> {
        let Some(w) = &self.wizard else {
            return column![
                heading("New sandbox", style::H1),
                self.btn(
                    "wiz:start",
                    "Start",
                    Kind::Primary,
                    Some(Message::WizardOpen(None))
                ),
            ]
            .spacing(12)
            .into();
        };
        let mut steps = row![].spacing(4);
        for (i, label) in STEPS.iter().enumerate() {
            let n = i as u8 + 1;
            let tone = if n == w.step { Tone::Ac } else { Tone::Neutral };
            steps = steps.push(self.pill(*label, tone));
        }
        let profile = self.wiz_config();
        let body: El = match w.step {
            1 => {
                let mut list = column![].spacing(6);
                for p in &self.profiles {
                    let on = w.profile.as_deref() == Some(p.id.as_str());
                    list = list.push(
                        self.button(
                            &format!("wiz:profile:{}", p.id),
                            row![
                                icon(p.icon.as_ref(), &p.name, 24.0),
                                column![txt(p.name.as_str()).font(bold()), small(p.description.clone().unwrap_or_default()).color(self.t.mu)]
                            ]
                            .spacing(10)
                            .align_y(Alignment::Center),
                            Kind::Choice(on),
                            super::kit::Size::Normal,
                            Some(Message::WizProfile(p.id.clone())),
                        )
                        .width(Length::Fill),
                    );
                }
                list = list.push(self.btn("wiz:import", "Import a profile…", Kind::Link, Some(Message::Go(Page::Import))));
                let detail: El = match profile {
                    Some(c) => self
                        .card(column![heading(format!("What “{}” sets up", c.name), style::H3), self.kv(summary_lines(c))].spacing(10))
                        .into(),
                    None => small("Pick a profile.").color(self.t.mu).into(),
                };
                row![container(list).width(Length::FillPortion(1)), container(detail).width(Length::FillPortion(1))].spacing(16).into()
            }
            2 => {
                let current = w.base.clone();
                let mut list = column![small("The base system of the sandbox; the system package manager follows it.").color(self.t.mu)].spacing(6);
                let default = profile.map(|c| c.base_image.clone()).unwrap_or_default();
                for (name, reference) in BASE_IMAGES {
                    let on = current.as_deref().unwrap_or(default.as_str()) == reference && current.is_some() || (current.is_none() && default == reference && self.value(WIZ_IMAGE).is_empty());
                    list = list.push(
                        self.button(
                            &format!("wiz:base:{reference}"),
                            row![txt(name).font(bold()), small(reference).color(self.t.mu).font(Font::MONOSPACE)].spacing(10),
                            Kind::Choice(on),
                            super::kit::Size::Normal,
                            Some(Message::WizBase(Some(reference.to_owned()))),
                        )
                        .width(Length::Fill),
                    );
                }
                list = list.push(row![small("Other image").width(110), self.field(WIZ_IMAGE, "registry/name:tag", Length::Fill)].spacing(8).align_y(Alignment::Center));
                let base = current.unwrap_or_else(|| if self.value(WIZ_IMAGE).is_empty() { default } else { self.value(WIZ_IMAGE) });
                if profile.is_some_and(|c| !c.apt_repos.is_empty()) && !base.contains("debian") && !base.contains("ubuntu") {
                    list = list.push(self.banner(Tone::Warn, small("This profile has apt repositories: its build needs a Debian or Ubuntu base.")));
                }
                container(list).max_width(700).into()
            }
            3 => {
                let mut colors = row![].spacing(6);
                for (i, c) in COLORS.iter().enumerate() {
                    colors = colors.push(self.button(
                        &format!("wiz:color:{i}"),
                        icon(Some(&cardea_core::model::Icon { letter: self.value(WIZ_NAME).chars().next().unwrap_or('S').to_uppercase().collect(), color: (*c).to_owned() }), "", 22.0),
                        Kind::Choice(w.color == i),
                        super::kit::Size::Small,
                        Some(Message::WizColor(i)),
                    ));
                }
                let location = self.settings.as_ref().and_then(|s| s.sandbox_location.clone()).unwrap_or_else(|| "~/Sandboxes".into());
                column![
                    row![small("Name").width(110), self.field(WIZ_NAME, "My sandbox", Length::Fixed(300.0))].spacing(8).align_y(Alignment::Center),
                    row![small("Icon").width(110), colors].spacing(8).align_y(Alignment::Center),
                    row![small("Location").width(110), self.field(WIZ_LOCATION, "", Length::Fixed(300.0)), small(format!("empty: {location}")).color(self.t.mu)]
                        .spacing(8)
                        .align_y(Alignment::Center),
                    small("The sandbox gets its own folder there (home and configuration).").color(self.t.mu),
                ]
                .spacing(10)
                .into()
            }
            4 => {
                let card = |key: &str, trust: Trust, title: &'static str, lines: &'static str| {
                    self.button(
                        key,
                        column![txt(title).font(bold()), small(lines)].spacing(6),
                        Kind::Choice(w.trust == trust),
                        super::kit::Size::Normal,
                        Some(Message::WizTrust(trust)),
                    )
                    .width(Length::Fill)
                };
                column![
                    row![
                        card("wiz:work", Trust::Work, "Work", "For apps you trust.\n· Your own user ID, persistent home\n· Writable folders\n· Blocked connections refused\n· Keyring unlocked automatically"),
                        card("wiz:untrusted", Trust::Untrusted, "! Untrusted", "To try a dubious app.\n· Isolated user ID, fresh home at each start\n· Read-only folders\n· Blocked connections dropped\n· No keyring"),
                    ]
                    .spacing(12),
                    small("Every setting stays editable later in “Security & capabilities” and “Network”.").color(self.t.mu),
                ]
                .spacing(10)
                .into()
            }
            5 => {
                let mut list = column![small("Optional. Folders of your PC made visible in the sandbox.").color(self.t.mu)].spacing(6);
                for (i, f) in w.folders.iter().enumerate() {
                    list = list.push(
                        row![
                            container(small(f.host.as_str()).font(Font::MONOSPACE)).width(Length::FillPortion(3)),
                            container(small(f.mount.as_str()).font(Font::MONOSPACE)).width(Length::FillPortion(3)),
                            self.btn(&format!("wiz:mode:{i}"), if f.mode == Mode::Rw { "Read-write" } else { "Read-only" }, Kind::Normal, Some(Message::WizFolderMode(i))),
                            self.btn(&format!("wiz:rmfolder:{i}"), "Remove", Kind::Normal, Some(Message::WizFolderRemove(i))),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center),
                    );
                }
                list = list.push(
                    row![self.field(WIZ_FOLDER, "~/Projects/site", Length::Fixed(320.0)), self.btn("wiz:addfolder", "+ Add folder", Kind::Normal, Some(Message::WizFolderAdd))]
                        .spacing(8)
                        .align_y(Alignment::Center),
                );
                list.into()
            }
            6 => column![
                small("Optional. A command that customises this sandbox, run after the profile’s own. Edit the scripts later with `cardea edit`.")
                    .color(self.t.mu),
                row![small("At build (root)").width(140), self.field(WIZ_BUILD, "apt-get install -y …", Length::Fill)].spacing(8).align_y(Alignment::Center),
                row![small("At first start (you)").width(140), self.field(WIZ_FIRST, "git config --global …", Length::Fill)]
                    .spacing(8)
                    .align_y(Alignment::Center),
            ]
            .spacing(10)
            .into(),
            7 => {
                let rows = match (profile, self.value(WIZ_NAME)) {
                    (Some(c), name) => {
                        let mut r = vec![("Profile", c.name.clone()), ("Name", name)];
                        r.extend(summary_lines(c).into_iter().filter(|(k, _)| *k != "Trust"));
                        r.push(("Trust", format!("{:?}", w.trust)));
                        r.push(("Shared folders", w.folders.len().to_string()));
                        r
                    }
                    _ => vec![("Profile", "none".to_owned())],
                };
                column![self.card(self.kv(rows)), small("The image is built now; it can take several minutes.").color(self.t.mu)].spacing(10).into()
            }
            _ => match w.created.as_deref().and_then(|id| self.summary(id)) {
                Some(s) => {
                    let task = s.task.as_ref();
                    let lines = self.logs.get(&s.id).map(|l| l.iter().rev().take(8).rev().cloned().collect::<Vec<_>>().join("\n")).unwrap_or_default();
                    let mut col = column![row![
                        txt(match s.state {
                            State::Building => "Building the image…",
                            State::BuildFailed => "The build failed.",
                            _ => "The sandbox is ready.",
                        })
                        .font(bold()),
                        small(task.map(|t| format!("{} · {} %", t.step_label, t.percent)).unwrap_or_default()).color(self.t.mu)
                    ]
                    .spacing(10)]
                    .spacing(10);
                    if let Some(t) = task {
                        col = col.push(self.bar(t.percent));
                    }
                    col = col.push(self.card(text(lines).size(style::SMALL).font(Font::MONOSPACE)));
                    col.push(self.btn("wiz:open", "Open the sandbox", Kind::Primary, Some(Message::Go(Page::Sandbox(s.id.clone(), Section::Overview)))))
                        .into()
                }
                None => small("Creating…").color(self.t.mu).into(),
            },
        };
        let mut nav = row![grow()].spacing(8);
        if (2..=7).contains(&w.step) {
            nav = nav.push(self.btn(
                "wiz:back",
                "Back",
                Kind::Normal,
                Some(Message::WizStep(w.step - 1)),
            ));
        }
        if w.step < 7 {
            nav = nav.push(self.btn(
                "wiz:next",
                "Next",
                Kind::Primary,
                profile.is_some().then_some(Message::WizStep(w.step + 1)),
            ));
        } else if w.step == 7 {
            nav = nav.push(self.btn(
                "wiz:create",
                "Create",
                Kind::Primary,
                Some(Message::WizCreate),
            ));
        }
        column![heading("New sandbox", style::H1), steps.wrap(), body, nav]
            .spacing(14)
            .into()
    }

    pub fn profiles_view<'a>(&'a self) -> El<'a> {
        let card = |p: &'a cardea_proto::ProfileInfo| {
            let base = BASE_IMAGES
                .iter()
                .find(|(_, r)| *r == p.base_image)
                .map_or(p.base_image.as_str(), |(n, _)| n);
            let used = if p.used_by.is_empty() {
                String::new()
            } else {
                format!(" · used by {}", p.used_by.join(", "))
            };
            let mut actions = row![
                self.btn(
                    &format!("prof:create:{}", p.id),
                    "Create sandbox",
                    Kind::Primary,
                    Some(Message::WizardOpen(Some(p.id.clone())))
                ),
                self.btn(
                    &format!("prof:view:{}", p.id),
                    "View",
                    Kind::Normal,
                    Some(Message::Go(Page::Profile(p.id.clone())))
                ),
            ]
            .spacing(6);
            if !p.builtin {
                actions = actions.push(self.btn(
                    &format!("prof:delete:{}", p.id),
                    "Delete",
                    Kind::Danger,
                    Some(Message::ProfileDelete(p.id.clone())),
                ));
            }
            self.card(
                column![
                    row![
                        icon(p.icon.as_ref(), &p.name, 24.0),
                        txt(p.name.as_str()).font(bold())
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                    small(p.description.clone().unwrap_or_default()).color(self.t.mu),
                    small(format!(
                        "{base} · {} packages · {} apps{used}",
                        p.packages, p.apps
                    )),
                    actions.wrap(),
                ]
                .spacing(6),
            )
            .width(300)
        };
        let mut builtin = row![].spacing(12);
        let mut yours = row![].spacing(12);
        for p in &self.profiles {
            if p.builtin {
                builtin = builtin.push(card(p));
            } else {
                yours = yours.push(card(p));
            }
        }
        column![
            row![heading("Profiles", style::H1), grow(), self.btn("prof:import", "Import a profile…", Kind::Normal, Some(Message::Go(Page::Import)))]
                .align_y(Alignment::Center),
            small("A profile is the recipe of a sandbox: base image, packages and sources, apps, capabilities, settings and setup script. It is shared as a JSON file.")
                .color(self.t.mu),
            heading("Built-in", style::H3),
            builtin.wrap(),
            heading("Yours", style::H3),
            if self.profiles.iter().any(|p| !p.builtin) {
                El::from(yours.wrap())
            } else {
                small("None yet: import one, or save a sandbox as a profile from its Advanced page.").color(self.t.mu).into()
            },
        ]
        .spacing(12)
        .into()
    }

    pub fn profile_view<'a>(&'a self, id: &'a str) -> El<'a> {
        let info = self.profiles.iter().find(|p| p.id == id);
        let Some(c) = self.profile_configs.get(id) else {
            return small("Loading…").color(self.t.mu).into();
        };
        let builtin = info.is_some_and(|i| i.builtin);
        let mut items = column![].spacing(6);
        for item in review(c) {
            let title = if item.wide {
                format!("! {}", item.title)
            } else {
                item.title.clone()
            };
            items = items.push(rule(self)).push(
                row![
                    container(small(title).font(bold()).color(if item.wide {
                        self.t.warn
                    } else {
                        self.t.tx
                    }))
                    .width(200),
                    text(item.detail).size(style::SMALL).font(Font::MONOSPACE)
                ]
                .spacing(8),
            );
        }
        let used = info
            .map(|i| i.used_by.join(", "))
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| "no sandbox".into());
        column![
            self.btn(
                "prof:back",
                "‹ Profiles",
                Kind::Link,
                Some(Message::Go(Page::Profiles))
            ),
            row![
                icon(c.icon.as_ref(), &c.name, 36.0),
                heading(c.name.as_str(), style::H1),
                self.pill(if builtin { "built-in" } else { "yours" }, Tone::Neutral),
                grow(),
                self.btn(
                    "prof:create",
                    "Create sandbox",
                    Kind::Primary,
                    Some(Message::WizardOpen(Some(id.to_owned())))
                ),
                self.btn(
                    "prof:delete",
                    "Delete",
                    Kind::Danger,
                    (!builtin).then(|| Message::ProfileDelete(id.to_owned()))
                ),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            small(format!(
                "{} Used by: {used}.",
                c.description.clone().unwrap_or_default()
            ))
            .color(self.t.mu),
            self.card(items),
            row![
                self.field(EXPORT_PATH, "~/profile.json", Length::Fixed(320.0)),
                self.btn(
                    "prof:export",
                    "Export to this file",
                    Kind::Normal,
                    Some(Message::ProfileExport(id.to_owned()))
                )
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(12)
        .into()
    }

    pub fn import_view(&self) -> El<'_> {
        let mut col = column![
            self.btn(
                "imp:back",
                "‹ Profiles",
                Kind::Link,
                Some(Message::Go(Page::Profiles))
            ),
            heading("Import a profile", style::H1),
            row![
                self.field(
                    IMPORT_PATH,
                    "~/Downloads/profile.json",
                    Length::Fixed(380.0)
                ),
                self.btn("imp:read", "Read", Kind::Normal, Some(Message::ImportRead))
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(12);
        match &self.import {
            None => col = col.push(small("A profile may come from someone else: you see everything it asks for before it is saved.").color(self.t.mu)),
            Some(Err(e)) => col = col.push(self.banner(Tone::Err, small(e.clone()))),
            Some(Ok((_, c))) => {
                col = col.push(txt(format!("“{}” ({}) asks for:", c.name, c.id)).font(bold())).push(
                    small("Highlighted rows ! widen what its sandboxes can reach or run; scripts and custom sources are shown in full.")
                        .color(self.t.mu),
                );
                let mut items = column![].spacing(6);
                for item in review(c) {
                    let title = if item.wide { format!("! {}", item.title) } else { item.title.clone() };
                    items = items.push(rule(self)).push(
                        row![
                            container(small(title).font(bold()).color(if item.wide { self.t.warn } else { self.t.tx })).width(200),
                            text(item.detail).size(style::SMALL).font(Font::MONOSPACE)
                        ]
                        .spacing(8),
                    );
                }
                col = col.push(self.card(items)).push(
                    row![grow(), self.btn("imp:cancel", "Cancel", Kind::Normal, Some(Message::Go(Page::Profiles))), self.btn("imp:confirm", "Import", Kind::Primary, Some(Message::ImportConfirm))]
                        .spacing(8),
                );
            }
        }
        col.into()
    }

    pub fn settings_view<'a>(&'a self) -> El<'a> {
        let Some(s) = &self.settings else {
            return small("Loading…").color(self.t.mu).into();
        };
        let change = |f: &dyn Fn(&mut cardea_core::model::Settings)| {
            let mut n = s.clone();
            f(&mut n);
            Some(Message::SettingsSave(Box::new(n)))
        };
        let mut autostart = row![].spacing(6);
        for sb in &self.sandboxes {
            let on = self.configs.get(&sb.id).and_then(|c| c.autostart) == Some(true);
            autostart = autostart.push(self.btn(
                &format!("set:auto:{}", sb.id),
                format!("{} {}", if on { "☑" } else { "☐" }, sb.name),
                Kind::Nav(on),
                Some(Message::Autostart(sb.id.clone(), !on)),
            ));
        }
        let theme = |key: &str, label: &'static str, t: Theme| {
            self.btn(
                key,
                label,
                Kind::Nav(s.theme == t),
                change(&move |n| n.theme = t),
            )
        };
        let location = self.value(SET_LOCATION);
        let terminal = self.value(SET_TERMINAL);
        let mut table = column![].spacing(6);
        for sb in &self.sandboxes {
            let size = self.disks.get(&sb.id).map_or("…".into(), |d| gb(d.folder));
            table = table.push(
                row![
                    container(small(sb.name.as_str())).width(160),
                    container(small(sb.folder.as_str()).font(Font::MONOSPACE)).width(Length::Fill),
                    small(size)
                ]
                .spacing(8),
            );
        }
        let line = |label: &'static str, control: El<'a>| -> El<'a> {
            row![container(small(label)).width(220), control]
                .spacing(10)
                .align_y(Alignment::Center)
                .into()
        };
        column![
            heading("Settings", style::H1),
            self.card(
                column![
                    heading("Startup", style::H2),
                    line(
                        "Tray icon at login",
                        self.btn("set:tray", if s.tray_at_login { "● On" } else { "○ Off" }, Kind::Nav(s.tray_at_login), change(&|n| n.tray_at_login = !n.tray_at_login))
                    ),
                    row![container(small("Start automatically")).width(220), autostart.wrap()].spacing(10),
                ]
                .spacing(10)
            ),
            self.card(
                column![
                    heading("Storage", style::H2),
                    row![
                        container(small("Location for new sandboxes")).width(220),
                        self.field(SET_LOCATION, "~/Sandboxes", Length::Fixed(280.0)),
                        self.btn(
                            "set:location",
                            "Save",
                            Kind::Normal,
                            change(&move |n| n.sandbox_location = Some(location.clone()).filter(|l| !l.is_empty()))
                        ),
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                    small("Each sandbox gets its own folder there. It can also be chosen when creating a sandbox.").color(self.t.mu),
                    table,
                ]
                .spacing(10)
            ),
            self.card(
                column![
                    heading("Appearance and terminal", style::H2),
                    row![
                        container(small("Tool theme")).width(220),
                        theme("set:theme:system", "Follow system", Theme::System),
                        theme("set:theme:light", "Light", Theme::Light),
                        theme("set:theme:dark", "Dark", Theme::Dark)
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                    row![
                        container(small("Terminal command")).width(220),
                        self.field(SET_TERMINAL, "detected (xdg-terminal-exec, Konsole…)", Length::Fixed(280.0)),
                        self.btn(
                            "set:terminal",
                            "Save",
                            Kind::Normal,
                            change(&move |n| {
                                let argv: Vec<String> = terminal.split_whitespace().map(str::to_owned).collect();
                                n.terminal = (!argv.is_empty()).then_some(argv);
                            })
                        ),
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                    small(format!(
                        "Now: {}. The command to run is added after it, such as `konsole -e`.",
                        s.terminal.as_ref().map_or("detected".into(), |t| t.join(" "))
                    ))
                    .color(self.t.mu),
                ]
                .spacing(10)
            ),
            self.card(
                column![
                    row![
                        heading("Diagnostics", style::H2),
                        grow(),
                        self.btn("set:check", "Check again", Kind::Normal, Some(Message::RunChecks)),
                        self.btn("set:copy", "Copy report", Kind::Normal, Some(Message::CopyLog(self.report()))),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                    self.checks_view(),
                    small(format!("Cardea {}", cardea_core::VERSION)).color(self.t.mu),
                ]
                .spacing(10)
            ),
        ]
        .spacing(12)
        .into()
    }

    pub fn advanced_view<'a>(&'a self, s: &'a Summary) -> El<'a> {
        let Some(c) = self.configs.get(&s.id) else {
            return small("Loading…").color(self.t.mu).into();
        };
        let id = &s.id;
        let busy = s.task.is_some();
        let disk = self.disks.get(id);
        let folder = s.folder.clone();
        let home = c
            .home
            .path
            .clone()
            .unwrap_or_else(|| format!("{}/home", s.folder));
        let open = |p: &str| Message::OpenFolder(self.expand(p));
        let change = |f: &dyn Fn(&mut Config)| {
            let mut n = c.clone();
            f(&mut n);
            (!busy).then(|| Message::Change(Box::new(n)))
        };
        let theme = |key: &str, label: &'static str, t: Theme| {
            self.btn(
                key,
                label,
                Kind::Nav(c.appearance.theme == t),
                change(&move |n| n.appearance.theme = t),
            )
        };
        let line = |label: &'a str, control: El<'a>| -> El<'a> {
            row![container(small(label)).width(200), control]
                .spacing(10)
                .align_y(Alignment::Center)
                .into()
        };
        let auto = c.autostart == Some(true);
        let deletable = Self::allowed(s, cardea_core::state::Action::Delete);
        column![
            section(self, "Advanced"),
            self.card(
                column![
                    heading("Storage", style::H2),
                    line(
                        "Sandbox location",
                        row![
                            small(folder.clone()).font(Font::MONOSPACE),
                            small(disk.map_or("…".into(), |d| gb(d.folder))).color(self.t.mu),
                            self.btn("adv:open", "Open", Kind::Normal, Some(open(&folder)))
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center)
                        .into()
                    ),
                    line(
                        if c.home.disposable {
                            "Home (fresh at each start)"
                        } else {
                            "Persistent home"
                        },
                        row![
                            small(home.clone()).font(Font::MONOSPACE),
                            small(disk.map_or("…".into(), |d| gb(d.home))).color(self.t.mu),
                            self.btn("adv:home", "Open", Kind::Normal, Some(open(&home)))
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center)
                        .into()
                    ),
                    line(
                        "RAM limit (GB, empty: none)",
                        self.field(ADV_RAM, "none", Length::Fixed(90.0))
                    ),
                ]
                .spacing(10)
            ),
            self.card(
                column![
                    line(
                        "App theme",
                        row![
                            theme("adv:theme:system", "Follow system", Theme::System),
                            theme("adv:theme:light", "Light", Theme::Light),
                            theme("adv:theme:dark", "Dark", Theme::Dark)
                        ]
                        .spacing(6)
                        .into()
                    ),
                    line(
                        "App languages",
                        self.field(ADV_LANGS, "fr, en", Length::Fixed(240.0))
                    ),
                    row![
                        grow(),
                        self.btn(
                            "adv:save",
                            "Save RAM limit and languages",
                            Kind::Primary,
                            (!busy).then_some(Message::AdvancedSave)
                        )
                    ],
                    line(
                        "Start at login",
                        self.btn(
                            "adv:autostart",
                            if auto { "● On" } else { "○ Off" },
                            Kind::Nav(auto),
                            change(&move |n| n.autostart = Some(!auto))
                        )
                    ),
                    line(
                        "Diagnostics",
                        self.btn(
                            "adv:diag",
                            "Run diagnostics",
                            Kind::Normal,
                            Some(Message::Go(Page::Settings))
                        ),
                    ),
                ]
                .spacing(10)
            ),
            self.card(
                column![
                    row![
                        small("Duplicate as").width(200),
                        self.field(ADV_DUP, "name of the copy", Length::Fixed(240.0)),
                        self.btn(
                            "adv:dup",
                            "Duplicate",
                            Kind::Normal,
                            Some(Message::Duplicate(id.clone()))
                        )
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                    row![
                        small("Save as profile").width(200),
                        self.field(ADV_PROFILE, "profile-id", Length::Fixed(140.0)),
                        self.field(ADV_PROFILE_NAME, "Name", Length::Fixed(140.0)),
                        self.btn(
                            "adv:profile",
                            "Save as profile",
                            Kind::Normal,
                            Some(Message::ProfileSave(id.clone()))
                        )
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                    row![
                        small("Export sandbox.json").width(200),
                        self.field(ADV_EXPORT, "~/sandbox.json", Length::Fixed(240.0)),
                        self.btn(
                            "adv:export",
                            "Export",
                            Kind::Normal,
                            Some(Message::ExportSandbox(id.clone()))
                        )
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                    row![
                        grow(),
                        self.btn(
                            "adv:delete",
                            "Delete sandbox…",
                            Kind::Danger,
                            deletable.then(|| Message::AskDelete(id.clone()))
                        )
                    ],
                ]
                .spacing(10)
            ),
        ]
        .spacing(12)
        .into()
    }
}
