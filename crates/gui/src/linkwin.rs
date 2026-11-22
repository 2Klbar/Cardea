//! The link window (`cardea-gui --link <id>`): asks the owner whether a link a sandbox sent may open in the browser
//! (security R3). It reads the link from the daemon by id (never from its arguments) and answers there. It never
//! opens a link from the keyboard: no default button, and Open stays disabled for one second after it shows.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use cardea_daemon::paths::Paths;
use cardea_proto::{Incoming, LinkAsk, Method, PROTOCOL_VERSION, Request, Response};
use iced::widget::{Space, button, checkbox, column, container, rich_text, row, span, text};
use iced::{Alignment, Element, Font, Length, Subscription, Task, Theme, keyboard};
use serde_json::Value;

use crate::style::{self, Kind, Tokens, Tone};
use crate::ui::kit::bold;

/// One request on a fresh connection to the daemon.
pub fn call(method: Method) -> Result<Value, String> {
    let paths = Paths::from_env()?;
    let stream =
        UnixStream::connect(paths.socket()).map_err(|e| format!("cannot reach the daemon: {e}"))?;
    let mut writer = stream.try_clone().map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream);
    let hello = Method::Hello {
        version: PROTOCOL_VERSION,
    };
    for (id, method) in [(0, hello), (1, method)] {
        let mut line = serde_json::to_vec(&Request { id, method }).expect("serializable");
        line.push(b'\n');
        writer.write_all(&line).map_err(|e| e.to_string())?;
        let mut answer = String::new();
        reader.read_line(&mut answer).map_err(|e| e.to_string())?;
        match serde_json::from_str::<Incoming>(&answer).map_err(|e| e.to_string())? {
            Incoming::Response(Response::Ok { result, .. }) if id == 1 => return Ok(result),
            Incoming::Response(Response::Ok { .. }) => {}
            Incoming::Response(Response::Err { error, .. }) => return Err(error.message),
            Incoming::Event { .. } => return Err("unexpected event".into()),
        }
    }
    unreachable!("the loop returns on the second answer")
}

pub struct LinkWindow {
    ask: LinkAsk,
    dark: bool,
    armed: bool,
    always_sandbox: bool,
    always_domain: bool,
    error: Option<String>,
}

#[derive(Clone, Debug)]
pub enum Msg {
    Mode(iced::theme::Mode),
    Armed,
    AlwaysSandbox(bool),
    AlwaysDomain(bool),
    Answer(bool),
    Key(keyboard::Event),
}

/// The part of the link to show in bold: its host, as written in it (`[::1]` for IPv6).
fn host_span(url: &str) -> (usize, usize) {
    let start = url.find("://").map_or(0, |i| i + 3);
    let end = url[start..]
        .find(['/', '?', '#'])
        .map_or(url.len(), |i| start + i);
    let auth = &url[start..end];
    let host_start = start + auth.rfind('@').map_or(0, |i| i + 1);
    let host = &url[host_start..end];
    let len = if host.starts_with('[') {
        host.find(']').map_or(host.len(), |i| i + 1)
    } else {
        host.find(':').unwrap_or(host.len())
    };
    (host_start, host_start + len)
}

impl LinkWindow {
    pub fn new(ask: LinkAsk) -> (LinkWindow, Task<Msg>) {
        let w = LinkWindow {
            ask,
            dark: false,
            armed: false,
            always_sandbox: false,
            always_domain: false,
            error: None,
        };
        let task = Task::batch([
            iced::system::theme().map(Msg::Mode),
            Task::perform(tokio::time::sleep(Duration::from_secs(1)), |()| Msg::Armed),
        ]);
        (w, task)
    }

    pub fn title(&self) -> String {
        format!("Open a link from {}? · Cardea", self.ask.sandbox_name)
    }

    fn t(&self) -> Tokens {
        if self.dark { style::DARK } else { style::LIGHT }
    }

    pub fn theme(&self) -> Theme {
        self.t().theme(self.dark)
    }

    pub fn subscription(&self) -> Subscription<Msg> {
        Subscription::batch([
            keyboard::listen().map(Msg::Key),
            iced::system::theme_changes().map(Msg::Mode),
        ])
    }

    pub fn update(&mut self, msg: Msg) -> Task<Msg> {
        match msg {
            Msg::Mode(m) => self.dark = m == iced::theme::Mode::Dark,
            Msg::Armed => self.armed = true,
            Msg::AlwaysSandbox(b) => self.always_sandbox = b,
            Msg::AlwaysDomain(b) => self.always_domain = b,
            // Escape cancels; no key ever opens.
            Msg::Key(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::Escape),
                ..
            }) => return self.update(Msg::Answer(false)),
            Msg::Key(_) => {}
            Msg::Answer(open) => {
                if open && !self.armed {
                    return Task::none();
                }
                let answer = Method::LinkAnswer {
                    id: self.ask.id,
                    open,
                    always_sandbox: open && self.always_sandbox && self.ask.sandbox_box(),
                    always_domain: open && self.always_domain && self.ask.domain_box(),
                };
                match call(answer) {
                    Ok(_) => return iced::exit(),
                    Err(e) => self.error = Some(e),
                }
            }
        }
        Task::none()
    }

    pub fn view(&self) -> Element<'_, Msg> {
        let t = self.t();
        let a = &self.ask;
        let small = |s: String| text(s).size(style::SMALL);
        let mut body = column![].spacing(12);
        if a.untrusted {
            body = body.push(
                container(
                    text("! Untrusted sandbox: check this link with care.")
                        .font(bold())
                        .color(t.err),
                )
                .padding([8, 12])
                .width(Length::Fill)
                .style(move |_| style::banner(t, Tone::Err)),
            );
        }
        body = body.push(
            text(format!(
                "{} wants to open a link in your browser",
                a.sandbox_name
            ))
            .size(style::H2)
            .font(bold()),
        );
        let url = &a.link.url;
        let (hs, he) = host_span(url);
        body = body.push(
            container(
                rich_text([
                    span::<(), Font>(&url[..hs]),
                    span(&url[hs..he]).font(bold()),
                    span(&url[he..]),
                ])
                .font(Font::MONOSPACE)
                .size(style::SMALL),
            )
            .padding(10)
            .width(Length::Fill)
            .style(move |_| style::card(t)),
        );
        for m in &a.link.marks {
            let (before, strong, after) = m.sentence();
            body = body.push(
                row![
                    text("⚠").color(t.warn),
                    rich_text([
                        span::<(), Font>(before),
                        span(strong).font(bold()),
                        span(after)
                    ])
                    .size(style::SMALL),
                ]
                .spacing(8),
            );
        }
        if a.sandbox_box() {
            body = body.push(
                checkbox(self.always_sandbox)
                    .label("Always for this sandbox (stop asking for its links)")
                    .on_toggle(Msg::AlwaysSandbox)
                    .size(16)
                    .text_size(style::SMALL),
            );
        }
        if a.domain_box() {
            body = body.push(
                checkbox(self.always_domain)
                    .label(format!("Always for {} in this sandbox", a.link.host))
                    .on_toggle(Msg::AlwaysDomain)
                    .size(16)
                    .text_size(style::SMALL),
            );
        }
        if let Some(e) = &self.error {
            body = body.push(small(e.clone()).color(t.err));
        }
        let btn = |label: &'static str, kind: Kind, on: Option<Msg>| {
            button(text(label).size(style::SMALL))
                .padding([6, 14])
                .on_press_maybe(on)
                .style(move |_, status| style::button(t, kind, false, status))
        };
        let open_kind = if a.untrusted {
            Kind::DangerFill
        } else {
            Kind::Primary
        };
        body = body.push(
            row![
                Space::new().width(Length::Fill),
                btn("Cancel", Kind::Normal, Some(Msg::Answer(false))),
                btn("Open", open_kind, self.armed.then_some(Msg::Answer(true))),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
        let bg = if a.untrusted { t.err_soft } else { t.bg };
        container(body)
            .padding(18)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_| style::fill(bg, 0.0))
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::host_span;

    #[test]
    fn bold_host() {
        let cut = |u: &str| {
            let (a, b) = host_span(u);
            u[a..b].to_owned()
        };
        assert_eq!(cut("https://github.com/a"), "github.com");
        assert_eq!(cut("https://github.com@evil.com/x"), "evil.com");
        assert_eq!(cut("http://[::1]:8080/"), "[::1]");
        assert_eq!(cut("https://a.com:8443/?q=@x"), "a.com");
    }
}
