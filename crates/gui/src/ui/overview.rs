//! Overview: resources with live CPU / RAM lines, pinned apps, at a glance, recent events.

use cardea_core::model::{Config, KeyringMode, X11, Zone, ZoneMode};
use cardea_core::state::State;
use cardea_proto::Summary;
use iced::widget::{Canvas, column, container, row};
use iced::{Alignment, Font, Length};

use super::all::gb;
use super::kit::{El, heading, section, small};
use super::spark::{Chart, Line};
use crate::app::{App, SAMPLES};
use crate::style::{self, Kind};

/// Width of the window under which the Overview cards go on one column.
const TWO_COLUMNS: f32 = 1000.0;

/// "2026-11-20 14:52:03 text" gives ("14:52", "text").
pub fn split_line(line: &str) -> (&str, &str) {
    match (line.get(11..16), line.get(20..)) {
        (Some(time), Some(text)) if line.as_bytes().get(19) == Some(&b' ') => (time, text),
        _ => ("", line),
    }
}

fn zone(name: &str, z: &Zone) -> String {
    let (mode, n) = match z.mode {
        ZoneMode::Allow => ("allowlist", z.allow.len()),
        ZoneMode::Block => ("blocklist", z.block.len()),
    };
    if n == 0 {
        format!("{name}: {mode} (empty)")
    } else {
        format!("{name}: {mode} ({n})")
    }
}

fn capabilities(c: &Config) -> String {
    let caps = &c.capabilities;
    let mut v = Vec::new();
    if caps.wayland {
        v.push("Display");
    }
    match caps.x11 {
        X11::Off => {}
        X11::Isolated => v.push("X11 (isolated)"),
        X11::Host => v.push("X11 (host)"),
    }
    if caps.gpu {
        v.push("GPU");
    }
    if caps.kvm {
        v.push("KVM");
    }
    if caps.keyring.mode != KeyringMode::Off {
        v.push("Keyring");
    }
    if v.is_empty() {
        "None".into()
    } else {
        v.join(", ")
    }
}

impl App {
    pub fn overview_view<'a>(&'a self, s: &'a Summary) -> El<'a> {
        let config = self.configs.get(&s.id);
        let resources = self.card(self.resources(s));

        let mut apps = row![].spacing(8);
        let pinned: Vec<_> = config
            .map(|c| c.apps.iter().filter(|a| a.pinned).collect())
            .unwrap_or_default();
        for a in &pinned {
            apps = apps.push(self.btn(
                &format!("app:{}", a.id),
                a.name.as_str(),
                Kind::Normal,
                s.task.is_none().then(|| self.launch_message(&s.id, a)),
            ));
        }
        let mut right = column![heading("Pinned apps", style::H3)].spacing(8);
        right = if pinned.is_empty() {
            right.push(small("No pinned app.").color(self.t.mu))
        } else {
            right.push(apps.wrap())
        };
        right = right.push(heading("At a glance", style::H3));
        if let Some(c) = config {
            let network = if c.firewall.enabled {
                [
                    zone("Host", &c.firewall.host),
                    zone("Local network", &c.firewall.lan),
                    zone("Internet", &c.firewall.wan),
                ]
                .join(" · ")
            } else {
                "Firewall off: no restriction".into()
            };
            let folders = match c.folders.len() {
                0 => "None".into(),
                n => format!("{n} shared"),
            };
            let home = if c.home.disposable {
                "Disposable: wiped at each start"
            } else {
                "Persistent"
            };
            let kv = |k: &'a str, v: String, mono: bool| {
                row![
                    small(k).color(self.t.mu).width(100),
                    small(v)
                        .font(if mono {
                            Font::MONOSPACE
                        } else {
                            super::kit::font()
                        })
                        .width(Length::Fill)
                ]
                .spacing(14)
            };
            right = right.push(
                column![
                    kv("Image", c.base_image.clone(), false),
                    kv("Folders", folders, false),
                    kv("Network", network, false),
                    kv("Capabilities", capabilities(c), false),
                    kv("Home", home.into(), false),
                    kv("Location", s.folder.clone(), true),
                ]
                .spacing(4),
            );
        }
        let right = self.card(right);

        let mut events = column![].spacing(0);
        let lines = self.logs.get(&s.id).map(Vec::as_slice).unwrap_or_default();
        let t = self.t;
        for (i, line) in lines.iter().rev().take(5).enumerate() {
            let (time, text) = split_line(line);
            let r = container(
                row![
                    small(time).color(t.mu).width(80),
                    small(text).width(Length::Fill)
                ]
                .spacing(10),
            )
            .padding([7, 10])
            .width(Length::Fill);
            if i > 0 {
                events = events.push(super::kit::rule(self));
            }
            events = events.push(r);
        }
        let events: El = if lines.is_empty() {
            small("No event yet.").color(t.mu).into()
        } else {
            container(events)
                .width(Length::Fill)
                .style(move |_| style::card(t))
                .into()
        };
        let events = self.card(column![heading("Recent events", style::H3), events].spacing(8));

        let cards: El = if self.width >= TWO_COLUMNS {
            row![
                resources.width(Length::FillPortion(1)),
                right.width(Length::FillPortion(1))
            ]
            .spacing(12)
            .into()
        } else {
            column![resources, right].spacing(12).into()
        };
        column![section(self, "Overview"), cards, events]
            .spacing(12)
            .into()
    }

    fn resources<'a>(&'a self, s: &'a Summary) -> El<'a> {
        let t = self.t;
        let mut col = column![
            row![
                heading("Resources", style::H3),
                small(s.state.label()).color(t.mu)
            ]
            .spacing(8)
            .align_y(Alignment::Center)
        ]
        .spacing(10);
        let q = self.stats.get(&s.id);
        match (s.state, &s.task) {
            (_, Some(task)) => {
                col = col.push(self.bar(task.percent)).push(small(format!(
                    "Step {}/{} · {}",
                    task.step, task.steps, task.step_label
                )));
            }
            (State::Running, None) => match q.and_then(|q| q.back().map(|l| (q, *l))) {
                None => col = col.push(small("Measuring…").color(t.mu)),
                Some((q, last)) => {
                    let cpu_max = q.iter().map(|x| x.cpu).fold(10.0, f32::max);
                    let mem_top = last
                        .limit
                        .unwrap_or_else(|| q.iter().map(|x| x.memory).max().unwrap_or(1) * 2);
                    let chart = Chart {
                        lines: vec![
                            Line {
                                values: q.iter().map(|x| x.cpu / (cpu_max * 1.1)).collect(),
                                color: t.ac,
                                dashed: false,
                            },
                            Line {
                                values: q
                                    .iter()
                                    .map(|x| x.memory as f32 / mem_top as f32)
                                    .collect(),
                                color: t.warn,
                                dashed: true,
                            },
                        ],
                        slots: SAMPLES,
                        grid: Some(t.bd),
                        width: 2.0,
                    };
                    let ram = match last.limit {
                        Some(l) => format!("┅ RAM {} / {}", gb(last.memory), gb(l)),
                        None => format!("┅ RAM {}", gb(last.memory)),
                    };
                    col = col
                        .push(Canvas::new(chart).width(Length::Fill).height(110))
                        .push(
                            row![
                                small(format!("━ CPU {:.0} % (max {:.0} %)", last.cpu, cpu_max))
                                    .color(t.ac),
                                small(ram).color(t.warn),
                                small("· last minute").color(t.mu),
                            ]
                            .spacing(10)
                            .wrap(),
                        );
                }
            },
            (State::Error | State::BuildFailed, None) => {
                col = col.push(
                    container(small(s.error.clone().unwrap_or_default()).font(Font::MONOSPACE))
                        .padding(12)
                        .width(Length::Fill)
                        .style(move |_| container::Style {
                            background: Some(t.sunken.into()),
                            border: style::border(t.bd, 1.0, 8.0),
                            ..Default::default()
                        }),
                );
            }
            _ => {
                col = col.push(
                    container(small("No data: the sandbox is stopped.").color(t.mu))
                        .padding([10, 12])
                        .width(Length::Fill)
                        .style(move |_| container::Style {
                            border: style::border(t.bd, 1.0, 6.0),
                            ..Default::default()
                        }),
                );
            }
        }
        col.into()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn log_lines() {
        assert_eq!(
            super::split_line("2026-11-20 14:52:03 Step 1/3 · firewall rules"),
            ("14:52", "Step 1/3 · firewall rules")
        );
        assert_eq!(super::split_line("short"), ("", "short"));
    }
}
