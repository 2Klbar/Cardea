//! Design tokens of the mock-ups (light and dark) and the styles built on them.

use iced::widget::{button as btn, container, text_input};
use iced::{Background, Border, Color, Shadow, Theme};

#[derive(Clone, Copy, Debug)]
pub struct Tokens {
    pub bg: Color,
    pub surface: Color,
    pub sunken: Color,
    pub tx: Color,
    pub mu: Color,
    pub bd: Color,
    pub ac: Color,
    pub ac_tx: Color,
    pub ac_soft: Color,
    pub ok: Color,
    pub ok_soft: Color,
    pub warn: Color,
    pub warn_soft: Color,
    pub err: Color,
    pub err_soft: Color,
}

const fn hex(rgb: u32) -> Color {
    Color::from_rgb8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

pub const LIGHT: Tokens = Tokens {
    bg: hex(0xF4F5F7),
    surface: hex(0xFFFFFF),
    sunken: hex(0xECEEF1),
    tx: hex(0x1B1F24),
    mu: hex(0x5A626C),
    bd: hex(0xD5D9DF),
    ac: hex(0x2563C9),
    ac_tx: hex(0xFFFFFF),
    ac_soft: hex(0xE3ECFA),
    ok: hex(0x1F7A45),
    ok_soft: hex(0xE2F3E8),
    warn: hex(0x8F5400),
    warn_soft: hex(0xFBEFD9),
    err: hex(0xB42318),
    err_soft: hex(0xFBE4E2),
};

pub const DARK: Tokens = Tokens {
    bg: hex(0x17191C),
    surface: hex(0x202327),
    sunken: hex(0x1A1C1F),
    tx: hex(0xE8EAED),
    mu: hex(0xA2A9B2),
    bd: hex(0x363A40),
    ac: hex(0x6EA0F5),
    ac_tx: hex(0x0D1B33),
    ac_soft: hex(0x1D2B44),
    ok: hex(0x5CCB8A),
    ok_soft: hex(0x17301F),
    warn: hex(0xF0B048),
    warn_soft: hex(0x33270F),
    err: hex(0xF2827A),
    err_soft: hex(0x3A1A18),
};

/// Font sizes.
pub const BASE: f32 = 13.0;
pub const SMALL: f32 = 12.0;
pub const H1: f32 = 20.0;
pub const H2: f32 = 16.0;
pub const H3: f32 = 14.0;

impl Tokens {
    pub fn theme(&self, dark: bool) -> Theme {
        Theme::custom(
            if dark { "Cardea dark" } else { "Cardea light" },
            iced::theme::Palette {
                background: self.bg,
                text: self.tx,
                primary: self.ac,
                success: self.ok,
                warning: self.warn,
                danger: self.err,
            },
        )
    }

    /// (soft background, strong colour) of a tone.
    pub fn tone(&self, tone: Tone) -> (Color, Color) {
        match tone {
            Tone::Neutral => (self.sunken, self.mu),
            Tone::Ac => (self.ac_soft, self.ac),
            Tone::Ok => (self.ok_soft, self.ok),
            Tone::Warn => (self.warn_soft, self.warn),
            Tone::Err => (self.err_soft, self.err),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Ac,
    Ok,
    Warn,
    Err,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Normal,
    Primary,
    Danger,
    DangerFill,
    /// A sidebar entry; `true` when it is the current page.
    Nav(bool),
    /// A choice in a list; `true` when chosen.
    Choice(bool),
    /// A breadcrumb link.
    Link,
    /// A title that opens something (a card's name).
    Title,
}

pub fn border(color: Color, width: f32, radius: f32) -> Border {
    Border {
        color,
        width,
        radius: radius.into(),
    }
}

pub fn button(t: Tokens, kind: Kind, focused: bool, status: btn::Status) -> btn::Style {
    let hovered = status == btn::Status::Hovered;
    let (bg, fg, bd) = match kind {
        Kind::Normal => (Some(t.surface), t.tx, if hovered { t.ac } else { t.bd }),
        Kind::Primary => (Some(t.ac), t.ac_tx, t.ac),
        Kind::Danger => (Some(t.surface), t.err, t.err),
        Kind::DangerFill => (Some(t.err), Color::WHITE, t.err),
        Kind::Nav(on) => (
            if on {
                Some(t.ac_soft)
            } else if hovered {
                Some(t.sunken)
            } else {
                None
            },
            if on { t.ac } else { t.tx },
            Color::TRANSPARENT,
        ),
        Kind::Choice(on) => (
            Some(if on { t.ac_soft } else { t.surface }),
            t.tx,
            if on || hovered { t.ac } else { t.bd },
        ),
        Kind::Link => (None, if hovered { t.ac } else { t.mu }, Color::TRANSPARENT),
        Kind::Title => (None, if hovered { t.ac } else { t.tx }, Color::TRANSPARENT),
    };
    let width = match kind {
        Kind::Nav(_) | Kind::Link | Kind::Title => 0.0,
        Kind::Choice(true) => 2.0,
        _ => 1.0,
    };
    let mut style = btn::Style {
        background: bg.map(Background::Color),
        text_color: fg,
        border: border(bd, width, 6.0),
        shadow: Shadow::default(),
        snap: true,
    };
    if focused {
        // The focus ring: the accent, or the text colour where the button is filled with the accent.
        let ring = if kind == Kind::Primary { t.tx } else { t.ac };
        style.border = border(ring, 2.0, 6.0);
    }
    if status == btn::Status::Disabled {
        style.text_color = style.text_color.scale_alpha(0.45);
        style.background = style.background.map(|b| b.scale_alpha(0.45));
        style.border.color = style.border.color.scale_alpha(0.45);
    }
    style
}

pub fn card(t: Tokens) -> container::Style {
    container::Style {
        background: Some(t.surface.into()),
        border: border(t.bd, 1.0, 8.0),
        text_color: Some(t.tx),
        ..Default::default()
    }
}

pub fn pill(t: Tokens, tone: Tone) -> container::Style {
    let (bg, fg) = t.tone(tone);
    container::Style {
        background: Some(bg.into()),
        border: border(Color::TRANSPARENT, 0.0, 11.0),
        text_color: Some(fg),
        ..Default::default()
    }
}

pub fn banner(t: Tokens, tone: Tone) -> container::Style {
    let (bg, fg) = t.tone(tone);
    container::Style {
        background: Some(bg.into()),
        border: border(fg, 1.0, 8.0),
        text_color: Some(t.tx),
        ..Default::default()
    }
}

pub fn fill(color: Color, radius: f32) -> container::Style {
    container::Style {
        background: Some(color.into()),
        border: border(Color::TRANSPARENT, 0.0, radius),
        ..Default::default()
    }
}

pub fn input(t: Tokens, focused: bool, status: text_input::Status) -> text_input::Style {
    let active = focused || matches!(status, text_input::Status::Focused { .. });
    text_input::Style {
        background: t.surface.into(),
        border: border(
            if active { t.ac } else { t.bd },
            if focused { 2.0 } else { 1.0 },
            6.0,
        ),
        icon: t.mu,
        placeholder: t.mu,
        value: t.tx,
        selection: t.ac_soft,
    }
}

/// `#rrggbb` to a colour.
pub fn parse_hex(s: &str) -> Option<Color> {
    let v = u32::from_str_radix(s.strip_prefix('#')?, 16).ok();
    v.filter(|_| s.len() == 7).map(hex)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_colours() {
        assert_eq!(
            parse_hex("#2563C9"),
            Some(Color::from_rgb8(0x25, 0x63, 0xC9))
        );
        assert_eq!(parse_hex("2563C9"), None);
        assert_eq!(parse_hex("#25"), None);
    }
}
