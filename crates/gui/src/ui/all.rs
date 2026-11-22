//! All sandboxes: cards, search and filter, empty state, engine problem.

use cardea_core::state::{Action, State};
use cardea_proto::Summary;
use iced::widget::{Space, column, container, row, text, text_input};
use iced::{Alignment, Length};

use super::kit::{El, bold, grow, heading, icon, small, txt};
use super::{look, short_warning};
use crate::app::{App, Filter, Message, Page, SEARCH, Section};
use crate::focus::Group;
use crate::style::{self, Kind, Tone};

/// Width of the window under which the cards go on one column.
const TWO_COLUMNS: f32 = 950.0;

pub fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / f64::from(1u32 << 30))
}

impl App {
    pub fn all_view(&self) -> El<'_> {
        let running = self
            .sandboxes
            .iter()
            .filter(|s| s.state == State::Running)
            .count();
        let sum = if self.sandboxes.is_empty() {
            String::new()
        } else {
            format!("{} · {running} running", self.sandboxes.len())
        };
        let mut title = row![
            heading("All sandboxes", style::H1),
            txt(sum).color(self.t.mu),
            grow()
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        let mut page = column![].spacing(12);
        let has = !self.sandboxes.is_empty();
        if has {
            let focused = self.target(SEARCH, Group::Main, None, Some(SEARCH));
            let t = self.t;
            title = title.push(
                text_input("Search…", &self.search)
                    .id(SEARCH)
                    .on_input(Message::Search)
                    .size(style::BASE)
                    .padding([6, 10])
                    .width(200)
                    .style(move |_, status| style::input(t, focused, status)),
            );
        }
        page = page.push(title);
        if has {
            let mut seg = row![].spacing(4);
            for f in Filter::ALL {
                seg = seg.push(self.btn(
                    &format!("filter:{}", f.label()),
                    f.label(),
                    Kind::Choice(self.filter == f),
                    Some(Message::Filter(f)),
                ));
            }
            page = page.push(seg);
        }
        if let Some(problem) = self.hello.as_ref().and_then(|h| h.engine_error.clone()) {
            page = page.push(
                self.banner(
                    Tone::Err,
                    row![
                        txt("✕").color(self.t.err),
                        column![
                            txt("No sandbox can start: the engine is not available.").font(bold()),
                            small(problem).color(self.t.mu),
                        ]
                        .spacing(2)
                        .width(Length::Fill),
                        self.btn(
                            "engine:check",
                            "↻ Check again",
                            Kind::Normal,
                            Some(Message::CheckAgain)
                        ),
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                ),
            );
        }
        if !has {
            return page.push(self.empty_view()).into();
        }
        let needle = self.search.to_lowercase();
        let shown: Vec<&Summary> = self
            .sandboxes
            .iter()
            .filter(|s| self.filter.keeps(s) && s.name.to_lowercase().contains(&needle))
            .collect();
        if shown.is_empty() {
            return page
                .push(txt("No sandbox matches.").color(self.t.mu))
                .into();
        }
        let cols = if self.width >= TWO_COLUMNS { 2 } else { 1 };
        let mut grid = column![].spacing(12);
        for chunk in shown.chunks(cols) {
            let mut r = row![].spacing(12);
            for s in chunk {
                r = r.push(self.sandbox_card(s));
            }
            if chunk.len() < cols {
                r = r.push(grow());
            }
            grid = grid.push(r);
        }
        page.push(grid).into()
    }

    fn sandbox_card<'a>(&'a self, s: &'a Summary) -> El<'a> {
        let id = &s.id;
        let key = |what: &str| format!("card:{id}:{what}");
        let (_, pill, tone) = look(s);
        let open = Message::Go(Page::Sandbox(id.clone(), Section::Overview));
        let log = Message::Go(Page::Sandbox(id.clone(), Section::Log));
        let mut head = row![
            icon(s.icon.as_ref(), &s.name, 28.0),
            self.button(
                &key("name"),
                txt(s.name.as_str()).size(15).font(bold()),
                Kind::Title,
                super::kit::Size::Small,
                Some(open.clone()),
            ),
            self.pill(pill, tone),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        if let Some(c) = self.configs.get(id) {
            head = head.push(self.trust_pill(c.trust));
        }
        let mut body = column![head.wrap()].spacing(10);
        for w in &s.warnings {
            body = body.push(self.pill(format!("! {}", short_warning(w)), Tone::Err));
        }

        match (s.state, &s.task) {
            (State::Running, None) => body = body.push(self.card_stats(id)),
            (_, Some(t)) => {
                body = body
                    .push(small(format!(
                        "Step {}/{} · {}",
                        t.step, t.steps, t.step_label
                    )))
                    .push(self.bar(t.percent));
            }
            (State::Error | State::BuildFailed, None) => {
                if let Some(e) = &s.error {
                    body = body.push(small(e.as_str()).color(self.t.err));
                }
            }
            _ => {}
        }
        if s.flags.rebuild_required && Self::allowed(s, Action::Rebuild) {
            body = body.push(self.flag_banner(
                &key("rebuild"),
                "↻ Image rebuild required",
                "Rebuild",
                Message::Act(id.clone(), Action::Rebuild),
            ));
        } else if s.flags.restart_required && Self::allowed(s, Action::Restart) {
            body = body.push(self.flag_banner(
                &key("restart"),
                "↻ Restart required",
                "Restart",
                Message::Act(id.clone(), Action::Restart),
            ));
        }

        let mut acts = row![grow()].spacing(8);
        if matches!(s.state, State::Error | State::BuildFailed) || s.task.is_some() {
            acts = acts.push(self.btn(&key("log"), "View log", Kind::Normal, Some(log)));
        }
        let act = |action: Action, label: &'static str, kind: Kind, msg: Message| {
            Self::allowed(s, action).then(|| self.btn(&key(action.label()), label, kind, Some(msg)))
        };
        let state_act = match s.state {
            State::Running => act(
                Action::Stop,
                "■ Stop",
                Kind::Normal,
                Message::AskStop(id.clone()),
            ),
            State::Stopped => act(
                Action::Start,
                "▶ Start",
                Kind::Primary,
                Message::Act(id.clone(), Action::Start),
            ),
            State::Error => act(
                Action::Start,
                "↻ Retry",
                Kind::Normal,
                Message::Act(id.clone(), Action::Start),
            ),
            State::BuildFailed => act(
                Action::Rebuild,
                "↻ Retry build",
                Kind::Normal,
                Message::Act(id.clone(), Action::Rebuild),
            ),
            _ => None,
        };
        if let Some(b) = state_act {
            acts = acts.push(b);
        }
        acts = acts.push(self.btn(&key("open"), "Open ›", Kind::Normal, Some(open)));
        self.card(body.push(acts)).into()
    }

    fn flag_banner<'a>(&self, key: &str, label: &'a str, button: &'a str, msg: Message) -> El<'a> {
        let t = self.t;
        container(
            row![
                small(label).color(t.warn).width(Length::Fill),
                self.btn(key, button, Kind::Normal, Some(msg))
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding([4, 10])
        .style(move |_| style::banner(t, Tone::Warn))
        .into()
    }

    fn card_stats<'a>(&self, id: &str) -> El<'a> {
        let Some(q) = self.stats.get(id).filter(|q| !q.is_empty()) else {
            return small("Measuring…").color(self.t.mu).into();
        };
        let last = *q.back().expect("not empty");
        // Small bar charts made of plain widgets: several canvases on one page do not all show with the
        // software renderer.
        let spark = |values: Vec<f32>, color: iced::Color| {
            let mut bars = row![].spacing(1).align_y(Alignment::End).height(18);
            for v in values {
                bars = bars.push(
                    container(Space::new())
                        .width(2)
                        .height((v.clamp(0.0, 1.0) * 18.0).max(1.0))
                        .style(move |_| style::fill(color, 1.0)),
                );
            }
            container(bars).width(60).align_x(Alignment::End)
        };
        let recent: Vec<_> = q.iter().rev().take(20).rev().copied().collect();
        let cpu_max = recent.iter().map(|s| s.cpu).fold(10.0, f32::max);
        let mem_max = recent.iter().map(|s| s.memory).max().unwrap_or(1) as f32 * 1.2;
        row![
            small("CPU"),
            small(format!("{:.0} %", last.cpu)).font(bold()),
            spark(recent.iter().map(|s| s.cpu / cpu_max).collect(), self.t.ac),
            small("RAM"),
            small(gb(last.memory)).font(bold()),
            spark(
                recent.iter().map(|s| s.memory as f32 / mem_max).collect(),
                self.t.ac
            ),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .into()
    }

    fn empty_view(&self) -> El<'_> {
        let mut profiles = row![].spacing(12);
        for (i, p) in self.profiles.iter().enumerate() {
            let card = column![
                icon(None, &p.name, 28.0),
                txt(p.name.as_str()).font(bold()),
                small(p.description.clone().unwrap_or_default()).color(self.t.mu),
                self.btn(
                    &format!("empty:create:{}", p.id),
                    "Create",
                    if i == 0 { Kind::Primary } else { Kind::Normal },
                    Some(Message::WizardOpen(Some(p.id.clone()))),
                ),
            ]
            .spacing(6);
            profiles = profiles.push(self.card(card).width(190));
        }
        column![
            text("⬡").size(64).color(self.t.mu),
            heading("No sandbox yet", 18.0),
            container(
                txt("A sandbox is an isolated environment: its apps only see what you share with them. \
                     Start from a profile:")
                .color(self.t.mu)
                .align_x(Alignment::Center)
            )
            .max_width(440),
            profiles.wrap(),
            self.welcome_card(),
        ]
        .spacing(10)
        .padding([40, 0])
        .align_x(Alignment::Center)
        .width(Length::Fill)
        .into()
    }
}
