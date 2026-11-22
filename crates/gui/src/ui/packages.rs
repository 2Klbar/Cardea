//! Packages (installed, add, apt repository, sources) and Image & setup (base image, setup script, failure policy).

use cardea_core::model::{AptRepo, Config, OnFailure, Scope};
use cardea_core::packages::{self, BASE_IMAGES};
use cardea_core::state::State;
use cardea_proto::Summary;
use iced::widget::{column, container, row, text_input};
use iced::{Alignment, Font, Length};

use super::kit::{El, bold, grow, heading, rule, section, small};
use crate::app::{
    App, IMAGE_CUSTOM, Message, PKG_NAME, PKG_SOURCE, PKG_VERSION, REPO_COMPONENTS,
    REPO_FINGERPRINT, REPO_KEY, REPO_NAME, REPO_SUITE, REPO_URL,
};
use crate::focus::Group;
use crate::style::{self, Kind};

impl App {
    pub fn field(&self, key: &'static str, placeholder: &'static str, width: Length) -> El<'_> {
        let t = self.t;
        let focused = self.target(key, Group::Main, None, Some(key));
        text_input(placeholder, self.fields.get(key).map_or("", String::as_str))
            .id(key)
            .on_input(move |v| Message::Field(key, v))
            .font(Font::MONOSPACE)
            .size(style::SMALL)
            .padding([4, 8])
            .width(width)
            .style(move |_, status| style::input(t, focused, status))
            .into()
    }

    pub fn value(&self, key: &'static str) -> String {
        self.fields
            .get(key)
            .map(|v| v.trim().to_owned())
            .unwrap_or_default()
    }

    pub fn packages_view<'a>(&'a self, s: &'a Summary) -> El<'a> {
        let Some(c) = self.configs.get(&s.id) else {
            return small("Loading…").color(self.t.mu).into();
        };
        let id = &s.id;
        let busy = s.task.is_some();
        let running = s.state == State::Running;
        let on = |m: Message| (!busy).then_some(m);
        let mut page = column![
            row![
                container(section(self, "Packages")).width(Length::Fill),
                self.btn(
                    "pkg:update-all",
                    "↑ Update home packages",
                    Kind::Normal,
                    on(Message::PackageUpdate(None)).filter(|_| running)
                ),
                self.btn(
                    "pkg:fresh",
                    "↻ Rebuild and update the image",
                    Kind::Primary,
                    on(Message::RebuildFresh(id.clone()))
                ),
            ]
            .spacing(8)
            .align_y(Alignment::End)
        ]
        .spacing(12);
        let tabs = [
            "Installed",
            "+ Add package",
            "+ Add apt repository",
            "Sources",
        ];
        let mut bar = row![].spacing(6);
        for (i, label) in tabs.iter().enumerate() {
            bar = bar.push(self.btn(
                &format!("pkg:tab:{i}"),
                *label,
                Kind::Nav(self.packages_tab == i),
                Some(Message::PackagesTab(i)),
            ));
        }
        page = page.push(bar);
        let body: El = match self.packages_tab {
            1 => self.add_package(c),
            2 => self.add_repo(c, busy),
            3 => self.sources_list(c),
            _ => self.installed(s, c, busy),
        };
        page.push(body).into()
    }

    fn installed<'a>(&'a self, s: &'a Summary, c: &'a Config, busy: bool) -> El<'a> {
        let running = s.state == State::Running;
        let list = self
            .packages
            .get(&s.id)
            .cloned()
            .unwrap_or_else(|| packages::infos(c, &[]));
        let cell = |e: El<'a>, p: u16| container(e).width(Length::FillPortion(p));
        let head = |l: &'a str| small(l).color(self.t.mu).font(bold());
        let mut table = column![
            row![
                cell(head("Name").into(), 4),
                cell(head("Source").into(), 2),
                cell(head("Where").into(), 2),
                cell(head("Status").into(), 3),
                cell(head("").into(), 3),
            ]
            .spacing(8)
        ]
        .spacing(8);
        for (i, p) in list.iter().enumerate() {
            let status = match (&p.installed, p.skipped, running) {
                (_, true, _) => "skipped: capability off".to_owned(),
                (Some(v), _, _) => format!("✓ {v}"),
                (None, _, true) if p.image => "⏳ waiting for a rebuild".to_owned(),
                (None, _, true) => "✕ not installed".to_owned(),
                (None, _, false) => "—".to_owned(),
            };
            let name = match &p.version {
                Some(v) => format!("{} ({v})", p.name),
                None => p.name.clone(),
            };
            let can_update = running && !p.image && !busy;
            table = table.push(rule(self)).push(
                row![
                    cell(small(name).font(bold()).into(), 4),
                    cell(small(p.source.clone()).into(), 2),
                    cell(
                        small(if p.image { "image" } else { "home" })
                            .color(self.t.mu)
                            .into(),
                        2
                    ),
                    cell(small(status).into(), 3),
                    cell(
                        row![
                            self.btn(
                                &format!("pkg:update:{i}"),
                                "Update",
                                Kind::Normal,
                                can_update.then(|| Message::PackageUpdate(Some((
                                    p.source.clone(),
                                    p.name.clone()
                                ))))
                            ),
                            self.btn(
                                &format!("pkg:remove:{i}"),
                                "Remove",
                                Kind::Normal,
                                (!busy).then(|| Message::PackageRemove(
                                    p.source.clone(),
                                    p.name.clone()
                                ))
                            ),
                        ]
                        .spacing(6)
                        .into(),
                        3
                    ),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        }
        if list.is_empty() {
            table = table
                .push(rule(self))
                .push(small("No package.").color(self.t.mu));
        }
        table = table.push(
            small(if running {
                "Home packages install and update at once; image packages apply at the next rebuild."
            } else {
                "Installed versions are shown while the sandbox runs."
            })
            .color(self.t.mu),
        );
        table.into()
    }

    fn add_package<'a>(&'a self, c: &'a Config) -> El<'a> {
        let chosen = self.value(PKG_SOURCE);
        let mut sources = row![].spacing(4);
        let mut ids: Vec<String> = vec!["system".into()];
        ids.extend(packages::sources(c, None).into_iter().map(|s| s.id));
        for sid in ids {
            sources = sources.push(self.btn(
                &format!("pkg:source:{sid}"),
                sid.clone(),
                Kind::Nav(chosen == sid),
                Some(Message::Field(PKG_SOURCE, sid.clone())),
            ));
        }
        let image = packages::scope(c, &chosen) == Some(Scope::Image);
        let label = |l: &'static str| container(small(l)).width(110);
        let ready = !chosen.is_empty() && !self.value(PKG_NAME).is_empty();
        self.card(
            column![
                row![label("Source"), sources.wrap()]
                    .spacing(8)
                    .align_y(Alignment::Center),
                row![label("Name"), self.field(PKG_NAME, "ripgrep", Length::Fill)]
                    .spacing(8)
                    .align_y(Alignment::Center),
                row![
                    label("Version"),
                    self.field(PKG_VERSION, "latest", Length::Fixed(160.0))
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                small(if image {
                    "! Installed in the image: it applies at the next rebuild."
                } else {
                    "Installed in the home: at once in a running sandbox, else at its next start."
                })
                .color(if image { self.t.warn } else { self.t.mu }),
                row![
                    grow(),
                    self.btn(
                        "pkg:add",
                        "Add",
                        Kind::Primary,
                        ready.then_some(Message::PackageAdd)
                    )
                ],
            ]
            .spacing(10),
        )
        .into()
    }

    fn add_repo<'a>(&'a self, c: &'a Config, busy: bool) -> El<'a> {
        let label = |l: &'static str| container(small(l)).width(150);
        let repo = AptRepo {
            name: self.value(REPO_NAME),
            url: self.value(REPO_URL),
            suite: self.value(REPO_SUITE),
            components: self
                .value(REPO_COMPONENTS)
                .split([' ', ','])
                .filter(|x| !x.is_empty())
                .map(String::from)
                .collect(),
            key_url: self.value(REPO_KEY),
            fingerprint: self.value(REPO_FINGERPRINT).replace(' ', ""),
        };
        let mut new = c.clone();
        let ready = !repo.name.is_empty() && !repo.fingerprint.is_empty() && !busy;
        new.apt_repos.push(repo);
        let mut list = column![].spacing(6);
        for (i, r) in c.apt_repos.iter().enumerate() {
            let mut without = c.clone();
            without.apt_repos.remove(i);
            list = list.push(
                row![
                    small(r.name.as_str()).font(bold()),
                    small(format!("{} {} · key {}", r.url, r.suite, r.fingerprint))
                        .color(self.t.mu)
                        .font(Font::MONOSPACE),
                    grow(),
                    self.btn(
                        &format!("repo:remove:{i}"),
                        "Remove",
                        Kind::Normal,
                        (!busy).then(|| Message::Change(Box::new(without)))
                    ),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        }
        self.card(
            column![
                heading("apt repositories", style::H2),
                list,
                rule(self),
                row![label("Name"), self.field(REPO_NAME, "nodesource", Length::Fill)].spacing(8).align_y(Alignment::Center),
                row![label("Repository URL"), self.field(REPO_URL, "https://deb.nodesource.com/node_22.x", Length::Fill)]
                    .spacing(8)
                    .align_y(Alignment::Center),
                row![label("Suite"), self.field(REPO_SUITE, "nodistro", Length::Fixed(160.0)), small("Components").color(self.t.mu), self.field(REPO_COMPONENTS, "main", Length::Fill)]
                    .spacing(8)
                    .align_y(Alignment::Center),
                row![label("Signing key URL"), self.field(REPO_KEY, "https://…/key.asc", Length::Fill)].spacing(8).align_y(Alignment::Center),
                row![label("Expected fingerprint *"), self.field(REPO_FINGERPRINT, "6F71 F525 2828 41EE …", Length::Fill)]
                    .spacing(8)
                    .align_y(Alignment::Center),
                small("The fingerprint is required: it proves the key was not swapped. Its packages are added with the \
                       system source; the repository applies at the next rebuild, on an apt-based base image.")
                .color(self.t.mu),
                row![grow(), self.btn("repo:add", "Add repository", Kind::Primary, ready.then(|| Message::Change(Box::new(new))))],
            ]
            .spacing(10),
        )
        .into()
    }

    fn sources_list<'a>(&'a self, c: &'a Config) -> El<'a> {
        let mut list = column![small(
            "A source is a package manager. Built-in sources come with the tool; a custom source is a set of shell \
             commands (add one with `cardea sources <id> add <file>`)."
        )
        .color(self.t.mu)]
        .spacing(8);
        let mut all = vec![(
            "system".to_owned(),
            "System packages (follows the base image)".to_owned(),
            Scope::Image,
            false,
        )];
        all.extend(packages::sources(c, None).into_iter().map(|s| {
            let custom = c.sources.iter().any(|x| x.id == s.id);
            (s.id, s.name, s.scope, custom)
        }));
        for (sid, name, scope, custom) in all {
            let count = c.packages.iter().filter(|p| p.source == sid).count();
            list = list.push(rule(self)).push(
                row![
                    container(small(name).font(bold())).width(Length::FillPortion(4)),
                    container(small(if custom { "custom" } else { "built-in" }).color(self.t.mu))
                        .width(Length::FillPortion(1)),
                    container(small(match scope {
                        Scope::Image => "image · after rebuild",
                        Scope::Home => "home · immediate",
                    }))
                    .width(Length::FillPortion(2)),
                    container(small(format!("{count} packages"))).width(Length::FillPortion(1)),
                ]
                .spacing(8),
            );
        }
        self.card(list).into()
    }

    pub fn image_view<'a>(&'a self, s: &'a Summary) -> El<'a> {
        let Some(c) = self.configs.get(&s.id) else {
            return small("Loading…").color(self.t.mu).into();
        };
        let id = &s.id;
        let busy = s.task.is_some();
        let change = |f: &dyn Fn(&mut Config)| {
            let mut new = c.clone();
            f(&mut new);
            (!busy).then(|| Message::Change(Box::new(new)))
        };
        let current = BASE_IMAGES
            .iter()
            .find(|(_, r)| *r == c.base_image)
            .map_or("Custom image", |(l, _)| l);
        let mut bases = row![].spacing(4);
        for (i, (label, reference)) in BASE_IMAGES.iter().enumerate() {
            let r = (*reference).to_owned();
            bases = bases.push(self.btn(
                &format!("img:base:{i}"),
                *label,
                Kind::Nav(c.base_image == *reference),
                change(&move |n: &mut Config| n.base_image = r.clone()),
            ));
        }
        let custom = self.value(IMAGE_CUSTOM);
        let base = self.card(
            column![
                heading("Base image", style::H2),
                row![small(current).font(bold()), small(c.base_image.as_str()).font(Font::MONOSPACE).color(self.t.mu)].spacing(8),
                bases.wrap(),
                row![
                    self.field(IMAGE_CUSTOM, "registry.example.org/image:tag", Length::Fill),
                    self.btn(
                        "img:custom",
                        "Use this image",
                        Kind::Normal,
                        change(&move |n: &mut Config| n.base_image = custom.clone()).filter(|_| !self.value(IMAGE_CUSTOM).is_empty())
                    ),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                small(
                    "The home and shared folders are kept. The system packages follow the base image: names can differ \
                     between apt, dnf and pacman, and apt repositories need an apt-based image."
                )
                .color(self.t.mu),
            ]
            .spacing(10),
        );
        let script = |title: &'static str, note: &'static str, text: &'a str| -> El<'a> {
            column![
                row![small(title).font(bold()), small(note).color(self.t.mu)].spacing(8),
                container(
                    small(if text.trim().is_empty() {
                        "(empty)"
                    } else {
                        text
                    })
                    .font(Font::MONOSPACE)
                )
                .padding(8)
                .width(Length::Fill)
                .style(move |_| style::fill(self.t.sunken, 6.0)),
            ]
            .spacing(4)
            .into()
        };
        let policy = |key: &str, label: &'static str, p: OnFailure| {
            self.btn(
                key,
                label,
                Kind::Nav(c.setup.on_failure == p),
                change(&move |n: &mut Config| n.setup.on_failure = p),
            )
        };
        let setup = self.card(
            column![
                heading("Setup script", style::H2),
                script(
                    "During image build",
                    "as root · runs again at each rebuild",
                    c.setup.build.as_str()
                ),
                script(
                    "At first start",
                    "as you, in the home · runs once (each start with a fresh home)",
                    c.setup.first_start.as_str()
                ),
                row![
                    small("If a command fails"),
                    policy("img:stop", "Stop (recommended)", OnFailure::Stop),
                    policy("img:continue", "Log it and continue", OnFailure::Continue),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                small(format!("Edit the scripts with `cardea edit {id}`.")).color(self.t.mu),
            ]
            .spacing(10),
        );
        let actions = row![
            grow(),
            self.btn(
                "img:rebuild",
                "↻ Rebuild image",
                Kind::Normal,
                (!busy).then(|| Message::Act(id.clone(), cardea_core::state::Action::Rebuild))
            ),
            self.btn(
                "img:fresh",
                "↻ Rebuild and update",
                Kind::Primary,
                (!busy).then(|| Message::RebuildFresh(id.clone()))
            ),
        ]
        .spacing(8);
        column![section(self, "Image & setup"), base, setup, actions]
            .spacing(12)
            .into()
    }
}
