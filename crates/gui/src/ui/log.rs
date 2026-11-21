//! The Log page: the sandbox log, live, with a text filter and Copy.

use cardea_proto::Summary;
use iced::widget::{column, container, row, text, text_input};
use iced::{Alignment, Font, Length};

use super::kit::{El, section};
use crate::app::{App, LOG_FILTER, LOG_SCROLL, Message};
use crate::focus::Group;
use crate::style::{self, Kind};

impl App {
    pub fn log_view<'a>(&'a self, s: &'a Summary) -> El<'a> {
        let t = self.t;
        let needle = self.log_filter.to_lowercase();
        let lines: Vec<&str> = self
            .logs
            .get(&s.id)
            .map(|l| {
                l.iter()
                    .map(String::as_str)
                    .filter(|l| l.to_lowercase().contains(&needle))
                    .collect()
            })
            .unwrap_or_default();
        let shown = lines.join("\n");
        let focused = self.target(LOG_FILTER, Group::Main, None, Some(LOG_FILTER));
        let filter = text_input("Filter text…", &self.log_filter)
            .id(LOG_FILTER)
            .on_input(Message::LogFilter)
            .size(style::BASE)
            .padding([6, 10])
            .width(Length::Fill)
            .style(move |_, status| style::input(t, focused, status));
        let copy = self.btn(
            "log:copy",
            "Copy",
            Kind::Normal,
            (!shown.is_empty()).then(|| Message::CopyLog(shown.clone())),
        );
        let body = if lines.is_empty() {
            text(if self.log_filter.is_empty() {
                "The log is empty."
            } else {
                "No line matches."
            })
            .size(style::SMALL)
            .color(t.mu)
        } else {
            text(shown).size(style::SMALL).font(Font::MONOSPACE)
        };
        let pre = container(
            super::kit::scroll(container(body).padding(12).width(Length::Fill))
                .id(LOG_SCROLL)
                .anchor_bottom()
                .height(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(t.sunken.into()),
            border: style::border(t.bd, 1.0, 8.0),
            text_color: Some(t.tx),
            ..Default::default()
        });
        column![
            section(self, "Log"),
            row![filter, copy].spacing(8).align_y(Alignment::Center),
            pre
        ]
        .spacing(12)
        .height(Length::Fill)
        .into()
    }
}
