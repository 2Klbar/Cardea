//! A sandbox's pages: breadcrumb, header with state and actions, state banner, then the section.

use cardea_core::state::{Action, State};
use cardea_proto::Summary;
use iced::widget::{column, container, row};
use iced::{Alignment, Length};

use super::kit::{El, Size, bold, heading, icon, section, small, txt};
use super::{look, short_warning};
use crate::app::{App, Message, Page, Section};
use crate::style::{self, Kind, Tone};

impl App {
    pub fn sandbox_view<'a>(&'a self, s: &'a Summary, sec: Section) -> El<'a> {
        let id = &s.id;
        let go = |sec| Message::Go(Page::Sandbox(id.clone(), sec));
        let link = |key: &str, label: &'a str, msg| {
            self.button(key, small(label), Kind::Link, Size::Small, Some(msg))
        };
        let sep = || small("›").color(self.t.mu);
        let mut crumbs = row![
            link("crumb:all", "Sandboxes", Message::Go(Page::All)),
            sep()
        ]
        .spacing(4)
        .align_y(Alignment::Center);
        crumbs = if sec == Section::Overview {
            crumbs.push(small(s.name.as_str()).color(self.t.mu))
        } else {
            crumbs
                .push(link("crumb:sb", s.name.as_str(), go(Section::Overview)))
                .push(sep())
                .push(small(sec.label()).color(self.t.mu))
        };

        let (_, pill, tone) = look(s);
        let mut head = row![
            icon(s.icon.as_ref(), &s.name, 36.0),
            heading(s.name.as_str(), style::H1),
            self.pill(pill, tone),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        if let Some(c) = self.configs.get(id) {
            head = head.push(self.trust_pill(c.trust));
        }
        for w in &s.warnings {
            head = head.push(self.pill(format!("! {}", short_warning(w)), Tone::Err));
        }
        let mut actions = row![].spacing(8).align_y(Alignment::Center);
        let act = |action: Action, label: &'static str, kind: Kind, msg: Message| {
            Self::allowed(s, action)
                .then(|| self.btn(&format!("head:{}", action.label()), label, kind, Some(msg)))
        };
        let acts = match s.state {
            State::Running => vec![
                act(
                    Action::Restart,
                    "↻ Restart",
                    Kind::Normal,
                    Message::Act(id.clone(), Action::Restart),
                ),
                act(
                    Action::Stop,
                    "■ Stop",
                    Kind::Normal,
                    Message::AskStop(id.clone()),
                ),
            ],
            State::Stopped => vec![act(
                Action::Start,
                "▶ Start",
                Kind::Primary,
                Message::Act(id.clone(), Action::Start),
            )],
            State::Error => vec![act(
                Action::Start,
                "↻ Restart",
                Kind::Primary,
                Message::Act(id.clone(), Action::Start),
            )],
            State::BuildFailed => vec![act(
                Action::Rebuild,
                "↻ Retry build",
                Kind::Primary,
                Message::Act(id.clone(), Action::Rebuild),
            )],
            _ => vec![],
        };
        for b in acts.into_iter().flatten() {
            actions = actions.push(b);
        }
        if let Some(b) = act(
            Action::Delete,
            "Delete…",
            Kind::Danger,
            Message::AskDelete(id.clone()),
        ) {
            actions = actions.push(b);
        }
        let head = row![container(head.wrap()).width(Length::Fill), actions]
            .spacing(8)
            .align_y(Alignment::Center);

        let mut page = column![crumbs, head].spacing(12);
        if let Some(b) = self.state_banner(s) {
            page = page.push(b);
        }
        let body = match sec {
            Section::Overview => self.overview_view(s),
            Section::Log => self.log_view(s),
            other => column![
                section(self, other.label()),
                txt("Not available yet.").color(self.t.mu)
            ]
            .spacing(12)
            .into(),
        };
        if sec == Section::Log {
            page = page.height(Length::Fill);
        }
        page.push(body).into()
    }

    fn state_banner<'a>(&'a self, s: &'a Summary) -> Option<El<'a>> {
        let id = &s.id;
        let log = Message::Go(Page::Sandbox(id.clone(), Section::Log));
        let task = s.task.as_ref();
        let step = task.map(|t| t.step_label.clone()).unwrap_or_default();
        let (tone, title, detail, bar, button): (_, String, String, _, Option<(&str, Message)>) =
            match s.state {
                State::Building => (
                    Tone::Ac,
                    task.map_or("Building the image.".into(), |t| {
                        format!("Building the image · step {}/{}.", t.step, t.steps)
                    }),
                    step,
                    true,
                    Some(("View log", log)),
                ),
                State::Rebuilding => (
                    Tone::Ac,
                    format!(
                        "Rebuilding the image · {} %.",
                        task.map_or(0, |t| t.percent)
                    ),
                    "The sandbox stays usable; restart it at the end to use the new image.".into(),
                    true,
                    Some(("View log", log)),
                ),
                State::Starting => (Tone::Ac, "Starting.".into(), step, true, None),
                State::Stopping => (Tone::Ac, "Stopping.".into(), step, true, None),
                State::BuildFailed => (
                    Tone::Err,
                    "The build failed.".into(),
                    s.error.clone().unwrap_or_default(),
                    false,
                    Some(("View log", log)),
                ),
                State::Error => (
                    Tone::Err,
                    "The sandbox is in error.".into(),
                    s.error.clone().unwrap_or_default(),
                    false,
                    Some(("View log", log)),
                ),
                _ if s.flags.rebuild_required && Self::allowed(s, Action::Rebuild) => (
                    Tone::Warn,
                    "↻ Image rebuild required.".into(),
                    "Some changes need a new image.".into(),
                    false,
                    Some(("Rebuild image", Message::Act(id.clone(), Action::Rebuild))),
                ),
                _ if s.flags.restart_required && Self::allowed(s, Action::Restart) => (
                    Tone::Warn,
                    "↻ Restart required.".into(),
                    "Some changes apply at the next start.".into(),
                    false,
                    Some(("Restart now", Message::Act(id.clone(), Action::Restart))),
                ),
                _ => return None,
            };
        let mut text = column![row![txt(title).font(bold()), txt(detail)].spacing(6).wrap()]
            .spacing(6)
            .width(Length::Fill);
        if bar {
            text = text.push(self.bar(task.map_or(0, |t| t.percent)));
        }
        let mut content = row![text].spacing(10).align_y(Alignment::Center);
        if let Some((label, msg)) = button {
            content = content.push(self.btn("banner:act", label, Kind::Normal, Some(msg)));
        }
        Some(self.banner(tone, content))
    }
}
