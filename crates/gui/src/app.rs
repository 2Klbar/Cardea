//! Application state, messages and updates.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};

use cardea_core::model::Config;
use cardea_core::state::{self, Action, State};
use cardea_proto::{Event, Hello, Method, ProfileInfo, Status, Summary};
use iced::futures::channel::mpsc;
use iced::keyboard::{self, Key, key::Named};
use iced::widget::operation::{self, RelativeOffset};
use iced::{Subscription, Task, Theme, window};
use serde_json::Value;

use crate::conn::{self, Call, Out, Then};
use crate::focus::{self, Group, Move, Target};
use crate::style::{self, Tokens};

/// Stats samples kept per sandbox: one minute.
pub const SAMPLES: usize = 60;
/// Log lines kept per sandbox.
const LOG_LINES: usize = 2000;
pub const MAIN_SCROLL: &str = "main";
pub const LOG_SCROLL: &str = "log";
pub const SEARCH: &str = "search";
pub const LOG_FILTER: &str = "log-filter";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Overview,
    Apps,
    Folders,
    Packages,
    Image,
    Network,
    Links,
    Security,
    Log,
    Advanced,
}

impl Section {
    pub const ALL: [Section; 10] = [
        Section::Overview,
        Section::Apps,
        Section::Folders,
        Section::Packages,
        Section::Image,
        Section::Network,
        Section::Links,
        Section::Security,
        Section::Log,
        Section::Advanced,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Section::Overview => "Overview",
            Section::Apps => "Apps",
            Section::Folders => "Shared folders",
            Section::Packages => "Packages",
            Section::Image => "Image & setup",
            Section::Network => "Network",
            Section::Links => "Links",
            Section::Security => "Security & capabilities",
            Section::Log => "Log",
            Section::Advanced => "Advanced",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Page {
    All,
    Sandbox(String, Section),
    New,
    Profiles,
    Settings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    All,
    Running,
    Stopped,
    Attention,
}

impl Filter {
    pub const ALL: [Filter; 4] = [
        Filter::All,
        Filter::Running,
        Filter::Stopped,
        Filter::Attention,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Running => "Running",
            Filter::Stopped => "Stopped",
            Filter::Attention => "Needs attention",
        }
    }

    pub fn keeps(self, s: &Summary) -> bool {
        match self {
            Filter::All => true,
            Filter::Running => s.state == State::Running,
            Filter::Stopped => s.state == State::Stopped,
            Filter::Attention => {
                matches!(s.state, State::Error | State::BuildFailed)
                    || s.flags.restart_required
                    || s.flags.rebuild_required
                    || !s.warnings.is_empty()
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dialog {
    Stop(String),
    Delete { id: String, keep: bool },
}

#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub cpu: f32,
    pub memory: u64,
    pub limit: Option<u64>,
}

pub enum Link {
    Connecting,
    Ready(mpsc::UnboundedSender<Call>),
    Lost(String),
}

#[derive(Clone, Debug)]
pub enum Message {
    Conn(Out),
    Key(keyboard::Event),
    Mode(iced::theme::Mode),
    Resized(iced::Size),
    Show,
    Go(Page),
    Search(String),
    Filter(Filter),
    LogFilter(String),
    /// Start, restart, rebuild (stop goes through [`Message::AskStop`]).
    Act(String, Action),
    Create(String),
    AskStop(String),
    AskDelete(String),
    KeepHome(bool),
    Confirm,
    Cancel,
    CopyLog(String),
    CheckAgain,
    Dismiss,
    Click,
}

pub struct App {
    pub t: Tokens,
    pub dark: bool,
    /// `--theme light|dark`: ignores the system theme.
    forced: Option<bool>,
    pub link: Link,
    pub hello: Option<Hello>,
    pub sandboxes: Vec<Summary>,
    pub configs: HashMap<String, Config>,
    pub stats: HashMap<String, VecDeque<Sample>>,
    pub logs: HashMap<String, Vec<String>>,
    pub profiles: Vec<ProfileInfo>,
    pub page: Page,
    pub search: String,
    pub filter: Filter,
    pub log_filter: String,
    pub dialog: Option<Dialog>,
    /// The last failed action, shown until dismissed.
    pub notice: Option<String>,
    pub focus: Option<String>,
    pub width: f32,
    /// Filled by the view, read by the keyboard handling.
    pub targets: RefCell<Vec<Target<Message>>>,
    /// The layer the view is registering targets for.
    pub layer: Cell<u8>,
}

impl App {
    pub fn new(forced: Option<bool>) -> (App, Task<Message>) {
        let dark = forced.unwrap_or(false);
        let app = App {
            t: if dark { style::DARK } else { style::LIGHT },
            dark,
            forced,
            link: Link::Connecting,
            hello: None,
            sandboxes: Vec::new(),
            configs: HashMap::new(),
            stats: HashMap::new(),
            logs: HashMap::new(),
            profiles: Vec::new(),
            page: Page::All,
            search: String::new(),
            filter: Filter::All,
            log_filter: String::new(),
            dialog: None,
            notice: None,
            focus: None,
            width: 1100.0,
            targets: RefCell::new(Vec::new()),
            layer: Cell::new(0),
        };
        let task = if forced.is_some() {
            Task::none()
        } else {
            iced::system::theme().map(Message::Mode)
        };
        (app, task)
    }

    pub fn title(&self) -> String {
        match &self.page {
            Page::Sandbox(id, section) => {
                let name = self.summary(id).map_or(id.as_str(), |s| s.name.as_str());
                format!("{name} — {} · Cardea", section.label())
            }
            _ => "Cardea".into(),
        }
    }

    pub fn theme(&self) -> Theme {
        self.t.theme(self.dark)
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            Subscription::run(conn::run).map(Message::Conn),
            Subscription::run(crate::instance::requests).map(|()| Message::Show),
            keyboard::listen().map(Message::Key),
            // A click hands control to the mouse: the keyboard focus ring goes away.
            iced::event::listen_with(|event, _, _| match event {
                iced::Event::Mouse(iced::mouse::Event::ButtonPressed(_)) => Some(Message::Click),
                _ => None,
            }),
            window::resize_events().map(|(_, size)| Message::Resized(size)),
        ];
        if self.forced.is_none() {
            subs.push(iced::system::theme_changes().map(Message::Mode));
        }
        Subscription::batch(subs)
    }

    pub fn summary(&self, id: &str) -> Option<&Summary> {
        self.sandboxes.iter().find(|s| s.id == id)
    }

    fn call(&mut self, method: Method, then: Then) {
        if let Link::Ready(tx) = &self.link {
            let _ = tx.unbounded_send(Call { method, then });
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Conn(out) => self.on_conn(out),
            Message::Key(event) => return self.on_key(event),
            Message::Mode(mode) => {
                if self.forced.is_none() {
                    self.dark = mode == iced::theme::Mode::Dark;
                    self.t = if self.dark { style::DARK } else { style::LIGHT };
                }
            }
            Message::Resized(size) => self.width = size.width,
            Message::Show => {
                return window::latest().and_then(|id| {
                    Task::batch([
                        window::minimize(id, false),
                        window::gain_focus(id),
                        window::request_user_attention(
                            id,
                            Some(window::UserAttention::Informational),
                        ),
                    ])
                });
            }
            Message::Go(page) => return self.go(page),
            Message::Search(s) => {
                self.search = s;
                self.focus = Some(SEARCH.into());
            }
            Message::Filter(f) => self.filter = f,
            Message::LogFilter(s) => {
                self.log_filter = s;
                self.focus = Some(LOG_FILTER.into());
            }
            Message::Act(id, action) => {
                let method = match action {
                    Action::Start => Method::Start { id },
                    Action::Stop => Method::Stop { id },
                    Action::Restart => Method::Restart { id },
                    Action::Rebuild => Method::Rebuild { id },
                    Action::Delete => Method::Delete {
                        id,
                        keep_home: true,
                    },
                };
                self.call(method, Then::Action);
            }
            Message::Create(profile) => self.call(
                Method::Create {
                    profile: Some(profile),
                    name: None,
                    config: None,
                },
                Then::Action,
            ),
            Message::AskStop(id) => {
                self.dialog = Some(Dialog::Stop(id));
                self.focus = Some("dlg:cancel".into());
            }
            Message::AskDelete(id) => {
                self.dialog = Some(Dialog::Delete { id, keep: true });
                self.focus = Some("dlg:cancel".into());
            }
            Message::KeepHome(k) => {
                if let Some(Dialog::Delete { keep, .. }) = &mut self.dialog {
                    *keep = k;
                }
            }
            Message::Confirm => match self.dialog.take() {
                Some(Dialog::Stop(id)) => self.call(Method::Stop { id }, Then::Action),
                Some(Dialog::Delete { id, keep }) => self.call(
                    Method::Delete {
                        id,
                        keep_home: keep,
                    },
                    Then::Action,
                ),
                None => {}
            },
            Message::Cancel => self.dialog = None,
            Message::CopyLog(text) => return iced::clipboard::write(text),
            Message::CheckAgain => self.call(
                Method::Hello {
                    version: cardea_proto::PROTOCOL_VERSION,
                },
                Then::Hello,
            ),
            Message::Dismiss => self.notice = None,
            Message::Click => self.focus = None,
        }
        Task::none()
    }

    fn go(&mut self, page: Page) -> Task<Message> {
        if let Page::Sandbox(id, _) = &page
            && !self.logs.contains_key(id)
        {
            self.logs.insert(id.clone(), Vec::new());
            self.call(
                Method::Logs {
                    id: id.clone(),
                    follow: false,
                },
                Then::Logs(id.clone()),
            );
        }
        let log = matches!(page, Page::Sandbox(_, Section::Log));
        self.page = page;
        if log {
            operation::snap_to_end(LOG_SCROLL)
        } else {
            operation::snap_to(MAIN_SCROLL, RelativeOffset::START)
        }
    }

    fn on_conn(&mut self, out: Out) {
        match out {
            Out::Connected(hello, tx) => {
                self.hello = Some(hello);
                self.link = Link::Ready(tx);
                self.call(Method::List {}, Then::List);
                self.call(Method::Profiles {}, Then::Profiles);
                // Log lines missed while disconnected.
                let ids: Vec<String> = self.logs.keys().cloned().collect();
                for id in ids {
                    self.call(
                        Method::Logs {
                            id: id.clone(),
                            follow: false,
                        },
                        Then::Logs(id),
                    );
                }
            }
            Out::Lost(e) => self.link = Link::Lost(e),
            Out::Event(e) => self.on_event(e),
            Out::Reply(then, result) => self.on_reply(then, result),
        }
    }

    fn on_reply(&mut self, then: Then, result: Result<Value, String>) {
        let value = match result {
            Ok(v) => v,
            Err(e) => {
                if matches!(then, Then::Action) {
                    self.notice = Some(e);
                }
                return;
            }
        };
        fn parse<T: serde::de::DeserializeOwned>(v: Value) -> Option<T> {
            serde_json::from_value(v).ok()
        }
        match then {
            Then::List => {
                if let Some(list) = parse::<Vec<Summary>>(value) {
                    self.configs
                        .retain(|id, _| list.iter().any(|s| &s.id == id));
                    self.sandboxes = list;
                    let ids: Vec<String> = self.sandboxes.iter().map(|s| s.id.clone()).collect();
                    for id in ids {
                        self.call(Method::Status { id: id.clone() }, Then::Status(id));
                    }
                    self.leave_if_gone();
                }
            }
            Then::Status(id) => {
                if let Some(s) = parse::<Status>(value) {
                    self.configs.insert(id, s.config);
                }
            }
            Then::Logs(id) => {
                if let Some(tail) = parse::<Vec<String>>(value) {
                    let buf = self.logs.entry(id).or_default();
                    // Lines that arrived as events while the tail was on its way.
                    let newer: Vec<String> = buf
                        .drain(..)
                        .filter(|l| !tail.iter().rev().take(50).any(|t| t == l))
                        .collect();
                    *buf = tail;
                    buf.extend(newer);
                }
            }
            Then::Profiles => {
                if let Some(p) = parse(value) {
                    self.profiles = p;
                }
            }
            Then::Hello => {
                if let Some(h) = parse(value) {
                    self.hello = Some(h);
                }
            }
            Then::Action | Then::Ignore => {}
        }
    }

    fn on_event(&mut self, event: Event) {
        match event {
            Event::Added { sandbox } => {
                let id = sandbox.id.clone();
                self.sandboxes.retain(|s| s.id != id);
                self.sandboxes.push(sandbox);
                self.call(Method::Status { id: id.clone() }, Then::Status(id));
            }
            Event::Removed { id } => {
                self.sandboxes.retain(|s| s.id != id);
                self.configs.remove(&id);
                self.stats.remove(&id);
                self.logs.remove(&id);
                self.leave_if_gone();
            }
            Event::Changed { sandbox } => {
                let id = sandbox.id.clone();
                if sandbox.state != State::Running {
                    self.stats.remove(&id);
                }
                if let Some(s) = self.sandboxes.iter_mut().find(|s| s.id == id) {
                    *s = sandbox;
                }
                self.call(Method::Status { id: id.clone() }, Then::Status(id));
            }
            Event::Task { id, task } => {
                if let Some(s) = self.sandboxes.iter_mut().find(|s| s.id == id) {
                    s.task = task;
                }
            }
            Event::TaskEnded { .. } => {}
            Event::Log { id, line } => {
                if let Some(buf) = self.logs.get_mut(&id) {
                    buf.push(line);
                    if buf.len() > LOG_LINES {
                        buf.drain(..buf.len() - LOG_LINES);
                    }
                }
            }
            Event::Stats {
                id,
                cpu,
                memory,
                memory_limit,
            } => {
                let q = self.stats.entry(id).or_default();
                q.push_back(Sample {
                    cpu,
                    memory,
                    limit: memory_limit,
                });
                if q.len() > SAMPLES {
                    q.pop_front();
                }
            }
        }
    }

    /// Back to All sandboxes when the open sandbox is gone.
    fn leave_if_gone(&mut self) {
        if let Page::Sandbox(id, _) = &self.page
            && self.summary(id).is_none()
        {
            self.page = Page::All;
        }
        if let Some(Dialog::Stop(id) | Dialog::Delete { id, .. }) = &self.dialog
            && self.summary(id).is_none()
        {
            self.dialog = None;
        }
    }

    /// Whether `action` is offered for this sandbox now.
    pub fn allowed(s: &Summary, action: Action) -> bool {
        s.task.is_none() && state::check(s.state, action).is_ok()
    }

    fn on_key(&mut self, event: keyboard::Event) -> Task<Message> {
        let keyboard::Event::KeyPressed { key, modifiers, .. } = event else {
            return Task::none();
        };
        let ctrl = modifiers.command();
        let mv = match key.as_ref() {
            Key::Named(Named::Tab) if modifiers.shift() => Some(Move::Prev),
            Key::Named(Named::Tab) => Some(Move::Next),
            Key::Named(Named::ArrowDown | Named::ArrowRight) => Some(Move::Down),
            Key::Named(Named::ArrowUp | Named::ArrowLeft) => Some(Move::Up),
            Key::Named(Named::F6) => Some(Move::Region),
            _ => None,
        };
        if let Some(mv) = mv {
            return self.move_focus(mv);
        }
        match key.as_ref() {
            Key::Named(Named::Enter | Named::Space) => {
                let target = self.focused();
                match target {
                    Some((Some(msg), _)) => return self.update(msg),
                    Some((None, Some(input))) => return operation::focus(input),
                    _ => {}
                }
            }
            Key::Named(Named::Escape) => {
                if self.dialog.is_some() {
                    self.dialog = None;
                } else if self.notice.is_some() {
                    self.notice = None;
                } else if matches!(self.page, Page::Sandbox(..)) {
                    return self.go(Page::All);
                }
            }
            Key::Character("f") if ctrl && self.dialog.is_none() => {
                let input = match self.page {
                    Page::All => SEARCH,
                    Page::Sandbox(_, Section::Log) => LOG_FILTER,
                    _ => return Task::none(),
                };
                self.focus = Some(input.into());
                return operation::focus(input);
            }
            Key::Character("n") if ctrl && self.dialog.is_none() => return self.go(Page::New),
            Key::Character("l") if ctrl && self.dialog.is_none() => {
                if let Page::Sandbox(id, _) = &self.page {
                    return self.go(Page::Sandbox(id.clone(), Section::Log));
                }
            }
            Key::Character("q") if ctrl => return iced::exit(),
            _ => {}
        }
        Task::none()
    }

    /// The focused target's action and text field.
    fn focused(&self) -> Option<(Option<Message>, Option<&'static str>)> {
        let key = self.focus.as_deref()?;
        let targets = self.targets.borrow();
        let reachable = focus::reachable(&targets);
        let t = reachable.iter().find(|t| t.key == key)?;
        Some((t.on.clone(), t.input))
    }

    fn move_focus(&mut self, mv: Move) -> Task<Message> {
        let targets = self.targets.borrow();
        let Some(key) = focus::step(&targets, self.focus.as_deref(), mv) else {
            return Task::none();
        };
        let reachable = focus::reachable(&targets);
        let t = reachable
            .iter()
            .find(|t| t.key == key)
            .expect("step returns a target");
        // Text fields get the keyboard; anything else takes it from them (no widget has this id).
        let give = operation::focus(t.input.unwrap_or("none"));
        // shortcut: the page scrolls in proportion to the target's rank, not to its real position; good
        // enough for these pages, use the widget bounds if a long irregular page comes.
        let main: Vec<&&Target<Message>> = reachable
            .iter()
            .filter(|t| t.group == Group::Main && t.layer == 0)
            .collect();
        let scroll = match main.iter().position(|m| m.key == key) {
            Some(i) if main.len() > 1 => operation::snap_to(
                MAIN_SCROLL,
                RelativeOffset {
                    x: 0.0,
                    y: i as f32 / (main.len() - 1) as f32,
                },
            ),
            _ => Task::none(),
        };
        drop(targets);
        self.focus = Some(key);
        Task::batch([give, scroll])
    }

    /// Registers a target while the view is built; returns whether it has the focus.
    pub fn target(
        &self,
        key: &str,
        group: Group,
        on: Option<Message>,
        input: Option<&'static str>,
    ) -> bool {
        self.targets.borrow_mut().push(Target {
            key: key.to_owned(),
            group,
            layer: self.layer.get(),
            on,
            input,
        });
        self.focus.as_deref() == Some(key)
    }
}
