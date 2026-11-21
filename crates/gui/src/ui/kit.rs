//! Small building blocks of the mock-ups: buttons with the focus ring, pills, icons, banners, cards.

use std::sync::OnceLock;

use cardea_core::model::Icon;
use iced::widget::{Space, button, column, container, row, text};
use iced::{Alignment, Color, Element, Font, Length, Padding, font};

use crate::app::{App, Message};
use crate::focus::Group;
use crate::style::{self, Kind, Tone};

pub type El<'a> = Element<'a, Message>;

/// The desktop's sans-serif font: iced looks for "Open Sans" and picks any font when it is missing.
pub fn font() -> Font {
    static FONT: OnceLock<Font> = OnceLock::new();
    *FONT.get_or_init(|| {
        let family = std::process::Command::new("fc-match")
            .args(["-f", "%{family[0]}", "sans-serif"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|f| f.trim().to_owned())
            .filter(|f| !f.is_empty());
        match family {
            // Leaked once: iced wants a static name.
            Some(f) => Font::with_name(Box::leak(f.into_boxed_str())),
            None => Font::DEFAULT,
        }
    })
}

pub fn bold() -> Font {
    Font {
        weight: font::Weight::Bold,
        ..font()
    }
}

pub fn txt<'a>(s: impl text::IntoFragment<'a>) -> text::Text<'a> {
    text(s).size(style::BASE)
}

pub fn small<'a>(s: impl text::IntoFragment<'a>) -> text::Text<'a> {
    text(s).size(style::SMALL)
}

pub fn heading<'a>(s: impl text::IntoFragment<'a>, size: f32) -> text::Text<'a> {
    text(s).size(size).font(bold())
}

pub fn grow<'a>() -> El<'a> {
    Space::new().width(Length::Fill).into()
}

/// A vertical scrollable with a thin bar beside the content.
pub fn scroll<'a>(content: impl Into<El<'a>>) -> iced::widget::Scrollable<'a, Message> {
    use iced::widget::scrollable::{Direction, Scrollbar};
    iced::widget::scrollable(content).direction(Direction::Vertical(
        Scrollbar::new().width(6).scroller_width(6).spacing(2),
    ))
}

/// A one-pixel line.
pub fn rule<'a>(app: &App) -> El<'a> {
    let bd = app.t.bd;
    container(Space::new())
        .width(Length::Fill)
        .height(1)
        .style(move |_| style::fill(bd, 0.0))
        .into()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Size {
    Small,
    Normal,
}

impl App {
    /// A focusable button. `on: None` draws it disabled and keeps it out of the focus order.
    pub fn button<'a>(
        &self,
        key: &str,
        content: impl Into<El<'a>>,
        kind: Kind,
        size: Size,
        on: Option<Message>,
    ) -> button::Button<'a, Message> {
        let group = if matches!(kind, Kind::Nav(_)) || key.starts_with("side:") {
            Group::Side
        } else {
            Group::Main
        };
        let focused = on.is_some() && self.target(key, group, on.clone(), None);
        let t = self.t;
        let padding = match (kind, size) {
            (Kind::Nav(_), _) => Padding::from([5, 8]),
            (Kind::Link | Kind::Title, _) => Padding::from([0, 2]),
            (_, Size::Small) => Padding::from([4, 9]),
            (_, Size::Normal) => Padding::from([6, 12]),
        };
        button(content)
            .padding(padding)
            .on_press_maybe(on)
            .style(move |_, status| style::button(t, kind, focused, status))
    }

    /// A button with a text label.
    pub fn btn<'a>(
        &self,
        key: &str,
        label: impl text::IntoFragment<'a>,
        kind: Kind,
        on: Option<Message>,
    ) -> El<'a> {
        self.button(key, small(label), kind, Size::Small, on).into()
    }

    pub fn pill<'a>(&self, label: impl text::IntoFragment<'a>, tone: Tone) -> El<'a> {
        let t = self.t;
        container(small(label))
            .padding([2, 9])
            .style(move |_| style::pill(t, tone))
            .into()
    }

    pub fn banner<'a>(&self, tone: Tone, content: impl Into<El<'a>>) -> El<'a> {
        let t = self.t;
        container(content)
            .padding([8, 12])
            .width(Length::Fill)
            .style(move |_| style::banner(t, tone))
            .into()
    }

    pub fn card<'a>(&self, content: impl Into<El<'a>>) -> container::Container<'a, Message> {
        let t = self.t;
        container(content)
            .padding(14)
            .width(Length::Fill)
            .style(move |_| style::card(t))
    }

    /// A progress bar: 6 px, accent on the sunken colour.
    pub fn bar<'a>(&self, percent: u8) -> El<'a> {
        let t = self.t;
        let done = u16::from(percent.min(100));
        let mut r = row![];
        if done > 0 {
            r = r.push(
                container(Space::new())
                    .width(Length::FillPortion(done))
                    .height(6)
                    .style(move |_| style::fill(t.ac, 3.0)),
            );
        }
        if done < 100 {
            r = r.push(Space::new().width(Length::FillPortion(100 - done)));
        }
        container(r)
            .width(Length::Fill)
            .height(6)
            .style(move |_| style::fill(t.sunken, 3.0))
            .into()
    }
}

/// The coloured square with a letter.
pub fn icon<'a>(icon: Option<&Icon>, name: &str, px: f32) -> El<'a> {
    let (letter, color) = match icon {
        Some(i) => (
            i.letter.clone(),
            style::parse_hex(&i.color).unwrap_or(Color::from_rgb8(0x6B, 0x72, 0x80)),
        ),
        None => (
            name.chars().next().unwrap_or('?').to_uppercase().collect(),
            Color::from_rgb8(0x6B, 0x72, 0x80),
        ),
    };
    let radius = (px / 4.5).round();
    container(
        text(letter)
            .size((px * 0.46).round())
            .font(bold())
            .color(Color::WHITE),
    )
    .width(px)
    .height(px)
    .align_x(Alignment::Center)
    .align_y(Alignment::Center)
    .style(move |_| style::fill(color, radius))
    .into()
}

/// A section heading with a line above, as in the mock-ups.
pub fn section<'a>(app: &App, title: &'a str) -> El<'a> {
    column![rule(app), heading(title, style::H2)]
        .spacing(12)
        .into()
}
