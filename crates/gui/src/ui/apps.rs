//! Apps: the sandbox's apps (order, interface, pinned, menu entry, launch), the apps found in the sandbox, and the
//! background services with their instances.

use cardea_core::model::{App as SandboxApp, Autostart, Capability, Config, Toolkit};
use cardea_core::services::{self, InstanceState};
use cardea_core::state::State;
use cardea_proto::Summary;
use iced::widget::{column, container, row};
use iced::{Alignment, Length};

use super::kit::{El, bold, grow, heading, rule, section, small};
use crate::app::{App, Message, Page, Section};
use crate::style::{self, Kind, Tone};

const TOOLKITS: [(Toolkit, &str); 6] = [
    (Toolkit::Qt, "Qt"),
    (Toolkit::Gtk, "GTK"),
    (Toolkit::Electron, "Electron"),
    (Toolkit::Terminal, "Terminal"),
    (Toolkit::X11, "X11"),
    (Toolkit::Other, "Other"),
];

fn toolkit_label(t: Toolkit) -> &'static str {
    TOOLKITS
        .iter()
        .find(|(k, _)| *k == t)
        .map_or("Other", |(_, l)| l)
}

impl App {
    /// What a click on an app does: the terminal apps open in a host terminal.
    pub fn launch_message(&self, id: &str, a: &SandboxApp) -> Message {
        if a.toolkit == Toolkit::Terminal {
            Message::Terminal(vec!["app".into(), id.to_owned(), a.id.clone()])
        } else {
            Message::Launch(id.to_owned(), a.id.clone())
        }
    }

    pub fn apps_view<'a>(&'a self, s: &'a Summary) -> El<'a> {
        let Some(c) = self.configs.get(&s.id) else {
            return small("Loading…").color(self.t.mu).into();
        };
        let id = &s.id;
        let busy = s.task.is_some();
        let running = s.state == State::Running;
        let change = |f: &dyn Fn(&mut Config)| {
            let mut new = c.clone();
            f(&mut new);
            (!busy).then(|| Message::Change(Box::new(new)))
        };
        let mut page = column![section(self, "Apps")].spacing(12);

        // Requested capabilities that are off.
        for a in &c.apps {
            for want in &a.wants {
                if !c.capabilities.is_on(*want) {
                    let name = match want {
                        Capability::Wayland => "Wayland display",
                        Capability::X11 => "X11",
                        Capability::Gpu => "GPU",
                        Capability::Kvm => "KVM",
                        Capability::Keyring => "keyring",
                    };
                    let button: El = match want {
                        Capability::Gpu | Capability::Wayland => {
                            let gpu = *want == Capability::Gpu;
                            self.btn(
                                &format!("apps:enable:{}:{name}", a.id),
                                format!("Enable {name}"),
                                Kind::Normal,
                                change(&move |n: &mut Config| {
                                    if gpu {
                                        n.capabilities.gpu = true;
                                    } else {
                                        n.capabilities.wayland = true;
                                    }
                                }),
                            )
                        }
                        _ => self.btn(
                            &format!("apps:security:{}", a.id),
                            "Security & capabilities",
                            Kind::Normal,
                            Some(Message::Go(Page::Sandbox(id.clone(), Section::Security))),
                        ),
                    };
                    page = page.push(
                        self.banner(
                            Tone::Warn,
                            row![
                                small("!").color(self.t.warn).font(bold()),
                                container(small(format!(
                                    "{}: {name} requested, not enabled.",
                                    a.name
                                )))
                                .width(Length::Fill),
                                button,
                            ]
                            .spacing(10)
                            .align_y(Alignment::Center),
                        ),
                    );
                }
            }
        }

        let cell = |e: El<'a>, p: u16| container(e).width(Length::FillPortion(p));
        let head = |l: &'a str| small(l).color(self.t.mu).font(bold());
        let mut table = column![
            row![
                cell(head("Order").into(), 2),
                cell(head("App").into(), 4),
                cell(head("Interface").into(), 2),
                cell(head("Pinned").into(), 2),
                cell(head("Desktop menu").into(), 2),
                cell(head("").into(), 4),
            ]
            .spacing(8)
        ]
        .spacing(8);
        let n = c.apps.len();
        for (i, a) in c.apps.iter().enumerate() {
            let next = TOOLKITS[(TOOLKITS
                .iter()
                .position(|(t, _)| *t == a.toolkit)
                .unwrap_or(5)
                + 1)
                % TOOLKITS.len()]
            .0;
            let swap = |j: usize| change(&move |c: &mut Config| c.apps.swap(i, j));
            table = table.push(rule(self)).push(
                row![
                    cell(
                        row![
                            self.btn(
                                &format!("apps:up:{i}"),
                                "↑",
                                Kind::Normal,
                                (i > 0).then(|| swap(i - 1)).flatten()
                            ),
                            self.btn(
                                &format!("apps:down:{i}"),
                                "↓",
                                Kind::Normal,
                                (i + 1 < n).then(|| swap(i + 1)).flatten()
                            ),
                        ]
                        .spacing(4)
                        .into(),
                        2
                    ),
                    cell(small(a.name.as_str()).font(bold()).into(), 4),
                    cell(
                        self.btn(
                            &format!("apps:toolkit:{i}"),
                            format!("{} ▸", toolkit_label(a.toolkit)),
                            Kind::Normal,
                            change(&move |c: &mut Config| c.apps[i].toolkit = next)
                        ),
                        2
                    ),
                    cell(
                        self.btn(
                            &format!("apps:pin:{i}"),
                            if a.pinned { "● On" } else { "○ Off" },
                            Kind::Nav(a.pinned),
                            change(&move |c: &mut Config| c.apps[i].pinned = !c.apps[i].pinned)
                        ),
                        2
                    ),
                    cell(
                        self.btn(
                            &format!("apps:menu:{i}"),
                            if a.desktop_entry { "● On" } else { "○ Off" },
                            Kind::Nav(a.desktop_entry),
                            change(&move |c: &mut Config| c.apps[i].desktop_entry =
                                !c.apps[i].desktop_entry)
                        ),
                        2
                    ),
                    cell(
                        row![
                            self.btn(
                                &format!("apps:open:{i}"),
                                "Open",
                                Kind::Primary,
                                (!busy).then(|| self.launch_message(id, a))
                            ),
                            self.btn(
                                &format!("apps:remove:{i}"),
                                "Remove",
                                Kind::Normal,
                                change(&move |c: &mut Config| {
                                    c.apps.remove(i);
                                })
                            ),
                        ]
                        .spacing(6)
                        .into(),
                        4
                    ),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        }
        if c.apps.is_empty() {
            table = table
                .push(rule(self))
                .push(small("No app yet: add one found in the sandbox.").color(self.t.mu));
        }
        page = page.push(table);

        // Found in the sandbox.
        let mut found = column![heading("Found in the sandbox", style::H2)].spacing(8);
        match self.detected.get(id).filter(|_| running) {
            Some(list) => {
                let mut buttons = row![].spacing(6);
                let mut any = false;
                for (i, d) in list
                    .iter()
                    .enumerate()
                    .filter(|(_, d)| !c.apps.iter().any(|a| a.id == d.id))
                {
                    any = true;
                    let app = d.app();
                    buttons = buttons.push(self.btn(
                        &format!("apps:add:{i}"),
                        format!("+ {}", d.name),
                        Kind::Normal,
                        change(&move |c: &mut Config| c.apps.push(app.clone())),
                    ));
                }
                found = found.push(if any {
                    buttons.wrap().into()
                } else {
                    El::from(small("Every app found is already in the list.").color(self.t.mu))
                });
            }
            None => {
                found = found.push(small("Apps are found while the sandbox runs.").color(self.t.mu))
            }
        }
        page = page.push(found);

        // Background services.
        let mut svc = column![heading("Background services", style::H2)].spacing(8);
        let instances = match self.services.get(id).filter(|_| running) {
            Some(list) => list.clone(),
            None => services::instances(c, ""),
        };
        if instances.is_empty() {
            svc = svc.push(
                small(format!("No background service. Add one with `cardea service {id} add <name> -- <command>`."))
                    .color(self.t.mu),
            );
        }
        for (i, inst) in instances.iter().enumerate() {
            let name = c
                .services
                .iter()
                .find(|s| s.id == inst.service)
                .map_or(inst.service.as_str(), |s| s.name.as_str());
            let label = match &inst.folder {
                Some(f) => format!("{name} · {f}"),
                None => name.to_owned(),
            };
            let (state, tone) = match inst.state {
                InstanceState::Running => ("● Running".to_owned(), Tone::Ok),
                InstanceState::Exited(code) => (format!("✕ Exited (code {code})"), Tone::Err),
                InstanceState::Stopped => ("○ Stopped".to_owned(), Tone::Neutral),
            };
            let (service, folder) = (inst.service.clone(), inst.folder.clone());
            let on = |m: Message| (running && !busy).then_some(m);
            let start_label = if inst.state == InstanceState::Stopped {
                "Start"
            } else {
                "Restart"
            };
            let mut attach = vec![
                "service".to_owned(),
                id.clone(),
                "attach".into(),
                service.clone(),
            ];
            attach.extend(folder.clone());
            let auto = inst.autostart;
            let (s2, f2) = (service.clone(), folder.clone());
            let toggle_auto = change(&move |c: &mut Config| {
                if let Some(sv) = c.services.iter_mut().find(|x| x.id == s2) {
                    sv.autostart = match (&f2, sv.autostart.take()) {
                        (None, _) => Some(Autostart::Single(!auto)),
                        (Some(f), Some(Autostart::Folders(mut names))) => {
                            names.retain(|n| n != f);
                            if !auto {
                                names.push(f.clone());
                            }
                            Some(Autostart::Folders(names))
                        }
                        (Some(f), _) => Some(Autostart::Folders(if auto {
                            vec![]
                        } else {
                            vec![f.clone()]
                        })),
                    };
                }
            });
            svc = svc.push(rule(self)).push(
                row![
                    container(small(label).font(bold())).width(Length::FillPortion(3)),
                    container(self.pill(state, tone)).width(Length::FillPortion(2)),
                    self.btn(
                        &format!("svc:start:{i}"),
                        start_label,
                        Kind::Normal,
                        on(Message::Service(service.clone(), folder.clone(), true))
                    ),
                    self.btn(
                        &format!("svc:stop:{i}"),
                        "Stop",
                        Kind::Normal,
                        on(Message::Service(service.clone(), folder.clone(), false))
                            .filter(|_| inst.state != InstanceState::Stopped)
                    ),
                    self.btn(
                        &format!("svc:attach:{i}"),
                        "Attach",
                        Kind::Normal,
                        on(Message::Terminal(attach))
                            .filter(|_| inst.state != InstanceState::Stopped)
                    ),
                    self.btn(
                        &format!("svc:auto:{i}"),
                        if auto { "☑ At start" } else { "☐ At start" },
                        Kind::Nav(auto),
                        toggle_auto
                    ),
                    grow(),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        }
        svc = svc.push(
            small("An instance runs in tmux: Attach opens it in a terminal; detach with Ctrl+B then D. One that ends on \
                   its own is flagged on the sandbox.")
            .color(self.t.mu),
        );
        page.push(self.card(svc)).into()
    }
}
