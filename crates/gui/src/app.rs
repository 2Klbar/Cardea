//! Application state, messages and updates.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};

use cardea_core::apps::Detected;
use cardea_core::firewall::NetStatus;
use cardea_core::model::{
    Blocked, Config, Firewall, Folder, Icon, Mode, Package, Proto, Rule, Settings, Theme as Look,
    Trust, Zone, ZoneMode,
};
use cardea_core::packages::PackageInfo;
use cardea_core::services::Instance;
use cardea_core::state::{self, Action, State};
use cardea_proto::{Check, Disk, ErrorCode, Event, Hello, Method, ProfileInfo, Status, Summary};
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
pub const FOLDER_HOST: &str = "folder-host";
pub const FOLDER_OTHER: &str = "folder-other";
pub const PROTECT_NEW: &str = "protect-new";
pub const NET_TO: &str = "net-to";
pub const NET_PORT: &str = "net-port";
pub const NET_COMMENT: &str = "net-comment";
pub const PASSWORD: &str = "password";
pub const WIZ_NAME: &str = "wiz-name";
pub const WIZ_LOCATION: &str = "wiz-location";
pub const WIZ_IMAGE: &str = "wiz-image";
pub const WIZ_FOLDER: &str = "wiz-folder";
pub const WIZ_BUILD: &str = "wiz-build";
pub const WIZ_FIRST: &str = "wiz-first";
pub const IMPORT_PATH: &str = "import-path";
pub const EXPORT_PATH: &str = "export-path";
pub const SET_LOCATION: &str = "set-location";
pub const SET_TERMINAL: &str = "set-terminal";
pub const ADV_RAM: &str = "adv-ram";
pub const ADV_LANGS: &str = "adv-langs";
pub const ADV_DUP: &str = "adv-dup";
pub const ADV_PROFILE: &str = "adv-profile";
pub const ADV_PROFILE_NAME: &str = "adv-profile-name";
pub const ADV_EXPORT: &str = "adv-export";
pub const PKG_SOURCE: &str = "pkg-source";
pub const PKG_NAME: &str = "pkg-name";
pub const PKG_VERSION: &str = "pkg-version";
pub const REPO_NAME: &str = "repo-name";
pub const REPO_URL: &str = "repo-url";
pub const REPO_SUITE: &str = "repo-suite";
pub const REPO_COMPONENTS: &str = "repo-components";
pub const REPO_KEY: &str = "repo-key";
pub const REPO_FINGERPRINT: &str = "repo-fingerprint";
pub const IMAGE_CUSTOM: &str = "image-custom";
pub const LINK_HOST: &str = "link-host";

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
    /// A profile's detail.
    Profile(String),
    /// Reviewing a profile file before importing it.
    Import,
    Settings,
}

/// The New sandbox wizard: 1 profile, 2 base image, 3 name and location, 4 trust, 5 shared folders, 6 setup
/// script, 7 summary, 8 build.
#[derive(Clone, Debug, PartialEq)]
pub struct Wizard {
    pub step: u8,
    pub profile: Option<String>,
    /// `None`: the custom image reference field.
    pub base: Option<String>,
    pub color: usize,
    pub trust: Trust,
    pub folders: Vec<Folder>,
    /// The sandbox being built (step 8).
    pub created: Option<String>,
}

/// Icon colours offered by the wizard.
pub const COLORS: [&str; 6] = [
    "#c8643f", "#2563c9", "#2f8f7a", "#7c4dba", "#8a6d3b", "#6b7280",
];

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

#[derive(Clone, Debug, PartialEq)]
pub enum Dialog {
    Stop(String),
    Delete {
        id: String,
        keep: bool,
    },
    /// A risky setting, confirmed only once "I understand the risk" is ticked.
    Risk {
        id: String,
        risk: Risk,
        ack: bool,
    },
    /// A configuration change that turns on risky settings (`warnings`), saved once acknowledged.
    Change {
        id: String,
        config: Box<Config>,
        warnings: Vec<String>,
        ack: bool,
    },
    /// The keyring password for a start.
    Password {
        pending: Pending,
        value: String,
    },
}

/// An action waiting for the keyring password.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pending {
    Start(String),
    Restart(String),
    Rebuild(String),
    /// A rebuild without cache, the base image pulled again.
    RebuildFresh(String),
    Launch(String, String),
}

impl Pending {
    pub fn id(&self) -> &str {
        match self {
            Pending::Start(id)
            | Pending::Restart(id)
            | Pending::Rebuild(id)
            | Pending::RebuildFresh(id)
            | Pending::Launch(id, _) => id,
        }
    }

    fn method(&self, password: Option<String>) -> Method {
        let id = self.id().to_owned();
        match self {
            Pending::Start(_) => Method::Start { id, password },
            Pending::Restart(_) => Method::Restart { id, password },
            Pending::Rebuild(_) => Method::Rebuild {
                id,
                fresh: false,
                password,
            },
            Pending::RebuildFresh(_) => Method::Rebuild {
                id,
                fresh: true,
                password,
            },
            Pending::Launch(_, app) => Method::Launch {
                id,
                app: app.clone(),
                args: Vec::new(),
                url: None,
                password,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Risk {
    FirewallOff,
    /// Blocklist mode for the Host (0) or LAN (1) zone.
    Blocklist(usize),
}

/// The firewall entry being added to a zone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetDraft {
    pub zone: usize,
    pub to: String,
    pub port: String,
    pub proto: Option<Proto>,
    pub comment: String,
}

/// Where a new shared folder appears in the sandbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// `/work/<name>`.
    Default,
    /// The same path as on the host.
    Same,
    Other,
}

/// The "Add a folder" card being filled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderDraft {
    pub host: String,
    pub place: Place,
    pub other: String,
    pub ro: bool,
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
    /// Another launch asked to show the window, and a sandbox's page when not empty.
    Show(String),
    Go(Page),
    Search(String),
    Filter(Filter),
    LogFilter(String),
    /// Start, restart, rebuild (stop goes through [`Message::AskStop`]).
    Act(String, Action),
    WizardOpen(Option<String>),
    WizStep(u8),
    WizProfile(String),
    WizBase(Option<String>),
    WizColor(usize),
    WizTrust(Trust),
    WizFolderAdd,
    WizFolderRemove(usize),
    WizFolderMode(usize),
    WizCreate,
    ImportRead,
    ImportConfirm,
    ProfileExport(String),
    ProfileDelete(String),
    ProfileSave(String),
    SettingsSave(Box<Settings>),
    /// Start a sandbox at login, or not.
    Autostart(String, bool),
    RunChecks,
    OpenFolder(String),
    AdvancedSave,
    Duplicate(String),
    ExportSandbox(String),
    AskStop(String),
    AskDelete(String),
    KeepHome(bool),
    Confirm,
    Cancel,
    CopyLog(String),
    CheckAgain,
    Dismiss,
    Click,
    FolderOpen,
    FolderCancel,
    FolderHost(String),
    FolderPlace(Place),
    FolderOther(String),
    FolderRo(bool),
    FolderAdd,
    /// Remove the folder mounted there.
    FolderRemove(String),
    FolderMode(String, Mode),
    ProtectInput(String),
    ProtectAdd,
    ProtectRemove(String),
    RiskAck(bool),
    NetEnable(bool),
    NetReset,
    NetAnswer(Blocked),
    NetMode(usize, ZoneMode),
    NetOpen(usize),
    NetCancel,
    NetTo(String),
    NetPort(String),
    NetProto(Option<Proto>),
    NetComment(String),
    NetAdd,
    /// Remove the entry at this index of the zone's active list.
    NetRemove(usize, usize),
    /// Allow a blocked destination of a zone.
    NetAllow(usize, String),
    PasswordInput(String),
    /// A text field of the Packages or Image pages.
    Field(&'static str, String),
    PackagesTab(usize),
    PackageAdd,
    PackageRemove(String, String),
    /// Update the home packages: one (source, name), or all.
    PackageUpdate(Option<(String, String)>),
    RebuildFresh(String),
    /// Save the open sandbox's configuration changed this way; risky changes are confirmed first.
    Change(Box<Config>),
    Launch(String, String),
    /// Run a `cardea` command in a host terminal (terminal apps, attaching to a service).
    Terminal(Vec<String>),
    Service(String, Option<String>, bool),
    /// Adds the host typed in the Links page to the allowed ones.
    LinkAllow,
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
    /// Host paths of each sandbox's missing shared folders.
    pub missing: HashMap<String, Vec<String>>,
    pub folder_draft: Option<FolderDraft>,
    pub folder_error: Option<String>,
    pub protect_new: String,
    /// Live firewall state of the running sandboxes.
    pub net: HashMap<String, NetStatus>,
    pub net_draft: Option<NetDraft>,
    pub net_error: Option<String>,
    /// Apps found in each running sandbox's `.desktop` files.
    pub detected: HashMap<String, Vec<Detected>>,
    pub services: HashMap<String, Vec<Instance>>,
    /// The schemes of each sandbox's apps and who opens them on the host.
    pub schemes: HashMap<String, Vec<cardea_proto::SchemeInfo>>,
    pub packages: HashMap<String, Vec<PackageInfo>>,
    /// The text fields of the Packages and Image pages, by field id.
    pub fields: HashMap<&'static str, String>,
    pub wizard: Option<Wizard>,
    /// Full configurations of profiles, fetched for the wizard and the detail page.
    pub profile_configs: HashMap<String, Config>,
    /// The profile file being reviewed: its text, or why it cannot be read.
    pub import: Option<Result<(String, Config), String>>,
    pub settings: Option<Settings>,
    /// The last requirements check: desktop session, then Podman.
    pub checks: Option<Vec<Check>>,
    pub disks: HashMap<String, Disk>,
    /// The system's dark mode, used when neither `--theme` nor the settings choose.
    system_dark: bool,
    pub packages_tab: usize,
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
    pub fn new(forced: Option<bool>, sandbox: Option<String>) -> (App, Task<Message>) {
        let dark = forced.unwrap_or(false);
        let app = App {
            t: if dark { style::DARK } else { style::LIGHT },
            dark,
            forced,
            link: Link::Connecting,
            hello: None,
            sandboxes: Vec::new(),
            configs: HashMap::new(),
            missing: HashMap::new(),
            folder_draft: None,
            folder_error: None,
            protect_new: String::new(),
            net: HashMap::new(),
            net_draft: None,
            net_error: None,
            detected: HashMap::new(),
            services: HashMap::new(),
            schemes: HashMap::new(),
            packages: HashMap::new(),
            fields: HashMap::new(),
            wizard: None,
            profile_configs: HashMap::new(),
            import: None,
            settings: None,
            checks: None,
            disks: HashMap::new(),
            system_dark: false,
            packages_tab: 0,
            stats: HashMap::new(),
            logs: HashMap::new(),
            profiles: Vec::new(),
            page: sandbox.map_or(Page::All, |id| Page::Sandbox(id, Section::Overview)),
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
            Subscription::run(crate::instance::requests).map(Message::Show),
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
                self.system_dark = mode == iced::theme::Mode::Dark;
                self.apply_theme();
            }
            Message::Resized(size) => self.width = size.width,
            Message::Show(sandbox) => {
                if !sandbox.is_empty() {
                    let _ = self.go(Page::Sandbox(sandbox, Section::Overview));
                }
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
                let pending = match action {
                    Action::Start => Pending::Start(id),
                    Action::Restart => Pending::Restart(id),
                    Action::Rebuild => Pending::Rebuild(id),
                    Action::Stop => return self.update(Message::AskStop(id)),
                    Action::Delete => return self.update(Message::AskDelete(id)),
                };
                self.call(pending.method(None), Then::Unlock(pending));
            }
            Message::Launch(id, app) => {
                let pending = Pending::Launch(id, app);
                self.call(pending.method(None), Then::Unlock(pending));
            }
            Message::Terminal(argv) => {
                let mut full = vec![cardea_daemon::terminal::cardea().display().to_string()];
                full.extend(argv);
                let custom = self.settings.as_ref().and_then(|s| s.terminal.clone());
                if let Err(e) = cardea_daemon::terminal::open(custom.as_deref(), &full) {
                    self.notice = Some(e);
                }
            }
            Message::Service(service, folder, start) => {
                if let Page::Sandbox(id, _) = &self.page {
                    let id = id.clone();
                    self.call(
                        Method::Service {
                            id,
                            service,
                            folder,
                            start,
                        },
                        Then::Action,
                    );
                }
            }
            Message::Field(key, v) => {
                self.fields.insert(key, v);
                self.focus = Some(key.into());
            }
            Message::LinkAllow => {
                if let Some(c) = self.open_config() {
                    let mut c = c.clone();
                    match cardea_core::links::normalize_host(&self.value(LINK_HOST)) {
                        Ok(h) if c.links.allowed.contains(&h) => {
                            self.notice = Some(format!("{h} is already allowed"))
                        }
                        Ok(h) => {
                            c.links.allowed.push(h);
                            self.fields.remove(LINK_HOST);
                            return self.update(Message::Change(Box::new(c)));
                        }
                        Err(e) => self.notice = Some(e),
                    }
                }
            }
            Message::PackagesTab(t) => self.packages_tab = t,
            Message::PackageAdd => {
                if let Page::Sandbox(id, _) = &self.page {
                    let field = |k| {
                        self.fields
                            .get(k)
                            .map(|v: &String| v.trim().to_owned())
                            .filter(|v| !v.is_empty())
                    };
                    if let (Some(source), Some(name)) = (field(PKG_SOURCE), field(PKG_NAME)) {
                        let package = Package {
                            source,
                            name,
                            version: field(PKG_VERSION),
                            when: None,
                        };
                        let id = id.clone();
                        self.call(
                            Method::PackageAdd {
                                id: id.clone(),
                                package,
                            },
                            Then::PackageChanged(id),
                        );
                        self.fields.remove(PKG_NAME);
                        self.fields.remove(PKG_VERSION);
                    }
                }
            }
            Message::PackageRemove(source, name) => {
                if let Page::Sandbox(id, _) = &self.page {
                    let id = id.clone();
                    self.call(
                        Method::PackageRemove {
                            id: id.clone(),
                            source,
                            name,
                        },
                        Then::PackageChanged(id),
                    );
                }
            }
            Message::PackageUpdate(which) => {
                if let Page::Sandbox(id, _) = &self.page {
                    let id = id.clone();
                    let (source, name) = which.map_or((None, None), |(s, n)| (Some(s), Some(n)));
                    self.call(
                        Method::PackageUpdate {
                            id: id.clone(),
                            source,
                            name,
                        },
                        Then::PackageChanged(id),
                    );
                }
            }
            Message::RebuildFresh(id) => {
                let pending = Pending::RebuildFresh(id);
                self.call(pending.method(None), Then::Unlock(pending));
            }
            Message::PasswordInput(v) => {
                if let Some(Dialog::Password { value, .. }) = &mut self.dialog {
                    *value = v;
                }
                self.focus = Some(PASSWORD.into());
            }
            Message::Change(config) => {
                if let (Page::Sandbox(id, _), Some(old)) = (&self.page, self.open_config()) {
                    let before = cardea_core::risk::warnings(old);
                    let new: Vec<String> = cardea_core::risk::warnings(&config)
                        .into_iter()
                        .filter(|w| !before.contains(w))
                        .collect();
                    let id = id.clone();
                    if new.is_empty() {
                        self.save_config(&id, &config);
                    } else {
                        self.dialog = Some(Dialog::Change {
                            id,
                            config,
                            warnings: new,
                            ack: false,
                        });
                        self.focus = Some("dlg:cancel".into());
                    }
                }
            }
            Message::WizardOpen(profile) => {
                let profile = profile.or_else(|| self.profiles.first().map(|p| p.id.clone()));
                self.wizard = Some(Wizard {
                    step: 1,
                    profile: profile.clone(),
                    base: None,
                    color: 0,
                    trust: Trust::Work,
                    folders: Vec::new(),
                    created: None,
                });
                for k in [
                    WIZ_NAME,
                    WIZ_LOCATION,
                    WIZ_IMAGE,
                    WIZ_FOLDER,
                    WIZ_BUILD,
                    WIZ_FIRST,
                ] {
                    self.fields.remove(k);
                }
                if let Some(p) = profile {
                    self.wiz_profile(p);
                }
                return self.go(Page::New);
            }
            Message::WizStep(step) => {
                if let Some(w) = &mut self.wizard {
                    w.step = step.clamp(1, 7);
                }
                if step == 3 && !self.fields.contains_key(WIZ_NAME) {
                    let name = self
                        .wiz_config()
                        .map(|c| self.free_name(&c.name))
                        .unwrap_or_default();
                    self.fields.insert(WIZ_NAME, name);
                }
            }
            Message::WizProfile(p) => self.wiz_profile(p),
            Message::WizBase(b) => {
                if let Some(w) = &mut self.wizard {
                    w.base = b;
                }
            }
            Message::WizColor(i) => {
                if let Some(w) = &mut self.wizard {
                    w.color = i;
                }
            }
            Message::WizTrust(t) => {
                if let Some(w) = &mut self.wizard {
                    w.trust = t;
                    for f in &mut w.folders {
                        f.mode = Folder::default_mode(t);
                    }
                }
            }
            Message::WizFolderAdd => {
                let host = self.expand(&self.value(WIZ_FOLDER));
                if let Some(w) = &mut self.wizard
                    && !host.is_empty()
                {
                    let mode = Folder::default_mode(w.trust);
                    w.folders.push(Folder {
                        mount: Folder::default_mount(&host),
                        host,
                        mode,
                    });
                    self.fields.remove(WIZ_FOLDER);
                }
            }
            Message::WizFolderRemove(i) => {
                if let Some(w) = &mut self.wizard
                    && i < w.folders.len()
                {
                    w.folders.remove(i);
                }
            }
            Message::WizFolderMode(i) => {
                if let Some(f) = self.wizard.as_mut().and_then(|w| w.folders.get_mut(i)) {
                    f.mode = if f.mode == Mode::Rw {
                        Mode::Ro
                    } else {
                        Mode::Rw
                    };
                }
            }
            Message::WizCreate => match self.wiz_build() {
                Ok((config, location)) => match serde_json::to_string_pretty(&config) {
                    Ok(text) => {
                        self.logs.insert(config.id.clone(), Vec::new());
                        self.call(
                            Method::Create {
                                profile: None,
                                name: None,
                                config: Some(text),
                                location,
                            },
                            Then::Created,
                        );
                    }
                    Err(e) => self.notice = Some(e.to_string()),
                },
                Err(e) => self.notice = Some(e),
            },
            Message::ImportRead => {
                let path = self.expand(&self.value(IMPORT_PATH));
                self.import = Some(
                    std::fs::read_to_string(&path)
                        .map_err(|e| format!("cannot read {path}: {e}"))
                        .and_then(|text| {
                            cardea_core::load::config(&text, cardea_core::model::Kind::Profile)
                                .map(|c| (text, c))
                                .map_err(|e| format!("not a valid profile:\n{e}"))
                        }),
                );
            }
            Message::ImportConfirm => {
                if let Some(Ok((text, _))) = &self.import {
                    let config = text.clone();
                    self.call(
                        Method::ProfileImport {
                            config,
                            replace: false,
                        },
                        Then::ProfilesChanged,
                    );
                }
            }
            Message::ProfileExport(id) => match self.profile_configs.get(&id) {
                Some(p) => {
                    let path = self.expand(&self.value(EXPORT_PATH));
                    let text = serde_json::to_string_pretty(p).unwrap_or_default() + "\n";
                    match std::fs::write(&path, text) {
                        Ok(()) => self.notice = Some(format!("Exported to {path}")),
                        Err(e) => self.notice = Some(format!("cannot write {path}: {e}")),
                    }
                }
                None => self.call(Method::Profile { id: id.clone() }, Then::Profile(id)),
            },
            Message::ProfileDelete(id) => {
                self.call(Method::ProfileDelete { id }, Then::ProfilesChanged)
            }
            Message::ProfileSave(id) => {
                let profile = self.value(ADV_PROFILE);
                let name = self.value(ADV_PROFILE_NAME);
                let name = if name.is_empty() {
                    profile.clone()
                } else {
                    name
                };
                self.call(
                    Method::ProfileSave { id, profile, name },
                    Then::ProfilesChanged,
                );
            }
            Message::SettingsSave(settings) => {
                self.call(
                    Method::SetSettings {
                        settings: *settings,
                    },
                    Then::Settings,
                );
                self.call(Method::Settings {}, Then::Settings);
            }
            Message::Autostart(id, on) => {
                if let Some(c) = self.configs.get(&id) {
                    let mut c = c.clone();
                    c.autostart = Some(on);
                    self.save_config(&id, &c);
                }
            }
            Message::RunChecks => {
                self.checks = Some(cardea_daemon::doctor::desktop());
                self.call(Method::Doctor {}, Then::Doctor);
            }
            Message::OpenFolder(path) => {
                if let Err(e) = std::process::Command::new("xdg-open").arg(&path).spawn() {
                    self.notice = Some(format!("cannot open {path}: {e}"));
                }
            }
            Message::AdvancedSave => {
                if let Some(c) = self.open_config() {
                    let mut c = c.clone();
                    let ram = self.value(ADV_RAM);
                    c.security.ram_limit_gb = match ram.as_str() {
                        "" | "none" => None,
                        v => match v.parse::<u32>() {
                            Ok(n) if n > 0 => Some(n),
                            _ => {
                                self.notice = Some(format!("{v:?} is not a number of GB"));
                                return Task::none();
                            }
                        },
                    };
                    let langs = self.value(ADV_LANGS);
                    c.appearance.languages = langs
                        .split([',', ' '])
                        .filter(|l| !l.is_empty())
                        .map(str::to_owned)
                        .collect();
                    return self.update(Message::Change(Box::new(c)));
                }
            }
            Message::Duplicate(id) => {
                if let Some(c) = self.configs.get(&id) {
                    let mut c = c.clone();
                    let name = self.value(ADV_DUP);
                    c.name = if name.is_empty() {
                        self.free_name(&c.name)
                    } else {
                        name
                    };
                    c.id = self.free_id(&c.name);
                    c.identity.hostname = None;
                    c.home.path = None;
                    c.autostart = None;
                    match serde_json::to_string_pretty(&c) {
                        Ok(text) => self.call(
                            Method::Create {
                                profile: None,
                                name: None,
                                config: Some(text),
                                location: None,
                            },
                            Then::Action,
                        ),
                        Err(e) => self.notice = Some(e.to_string()),
                    }
                }
            }
            Message::ExportSandbox(id) => {
                if let Some(c) = self.configs.get(&id) {
                    let path = self.expand(&self.value(ADV_EXPORT));
                    let text = serde_json::to_string_pretty(c).unwrap_or_default() + "\n";
                    self.notice = Some(match std::fs::write(&path, text) {
                        Ok(()) => format!("Exported to {path}"),
                        Err(e) => format!("cannot write {path}: {e}"),
                    });
                }
            }
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
                Some(Dialog::Risk { risk, ack, .. }) if ack => self.firewall(|fw| match risk {
                    Risk::FirewallOff => fw.enabled = false,
                    Risk::Blocklist(z) => zone_mut(fw, z).mode = ZoneMode::Block,
                }),
                d @ Some(Dialog::Risk { .. }) => self.dialog = d,
                Some(Dialog::Change {
                    id,
                    config,
                    ack: true,
                    ..
                }) => self.save_config(&id, &config),
                d @ Some(Dialog::Change { .. }) => self.dialog = d,
                Some(Dialog::Password { pending, value }) => {
                    self.call(pending.method(Some(value)), Then::Unlock(pending));
                }
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
            Message::FolderOpen => {
                let ro = self
                    .open_config()
                    .is_some_and(|c| Folder::default_mode(c.trust) == Mode::Ro);
                self.folder_draft = Some(FolderDraft {
                    host: String::new(),
                    place: Place::Default,
                    other: String::new(),
                    ro,
                });
                self.folder_error = None;
                self.focus = Some(FOLDER_HOST.into());
                return operation::focus(FOLDER_HOST);
            }
            Message::FolderCancel => {
                self.folder_draft = None;
                self.folder_error = None;
            }
            Message::FolderHost(v) => self.draft(|d| d.host = v, FOLDER_HOST),
            Message::FolderPlace(p) => {
                self.draft(|d| d.place = p, "");
                if p == Place::Other {
                    self.focus = Some(FOLDER_OTHER.into());
                    return operation::focus(FOLDER_OTHER);
                }
            }
            Message::FolderOther(v) => self.draft(|d| d.other = v, FOLDER_OTHER),
            Message::FolderRo(ro) => self.draft(|d| d.ro = ro, ""),
            Message::FolderAdd => {
                if let (Some(d), Some(c)) = (self.folder_draft.clone(), self.open_config()) {
                    let host = self.expand(&d.host);
                    let mount = match d.place {
                        Place::Default => Folder::default_mount(&host),
                        Place::Same => host.clone(),
                        Place::Other => d.other.trim().to_owned(),
                    };
                    let mode = if d.ro { Mode::Ro } else { Mode::Rw };
                    let mut folders = c.folders.clone();
                    folders.push(Folder { host, mount, mode });
                    self.set_folders(folders, None);
                }
            }
            Message::FolderRemove(mount) => {
                if let Some(c) = self.open_config() {
                    let folders = c
                        .folders
                        .iter()
                        .filter(|f| f.mount != mount)
                        .cloned()
                        .collect();
                    self.set_folders(folders, None);
                }
            }
            Message::FolderMode(mount, mode) => {
                if let Some(c) = self.open_config() {
                    let mut folders = c.folders.clone();
                    for f in folders.iter_mut().filter(|f| f.mount == mount) {
                        f.mode = mode;
                    }
                    self.set_folders(folders, None);
                }
            }
            Message::ProtectInput(v) => {
                self.protect_new = v;
                self.focus = Some(PROTECT_NEW.into());
            }
            Message::ProtectAdd => {
                let name = self.protect_new.trim().to_owned();
                if let Some(c) = self.open_config()
                    && !name.is_empty()
                {
                    let mut names = c.protected_paths.names.clone();
                    if !names.contains(&name) {
                        names.push(name);
                    }
                    let folders = c.folders.clone();
                    self.protect_new.clear();
                    self.set_folders(folders, Some(names));
                }
            }
            Message::RiskAck(a) => {
                if let Some(Dialog::Risk { ack, .. } | Dialog::Change { ack, .. }) =
                    &mut self.dialog
                {
                    *ack = a;
                }
            }
            Message::NetEnable(true) => self.firewall(|fw| fw.enabled = true),
            Message::NetEnable(false) => self.risk(Risk::FirewallOff),
            Message::NetReset => {
                if let Some(c) = self.open_config() {
                    let defaults = Config::defaults(c.trust).firewall;
                    self.firewall(|fw| *fw = defaults);
                }
            }
            Message::NetAnswer(b) => self.firewall(|fw| fw.blocked = b),
            Message::NetMode(z, ZoneMode::Block) if z < 2 => self.risk(Risk::Blocklist(z)),
            Message::NetMode(z, mode) => self.firewall(|fw| zone_mut(fw, z).mode = mode),
            Message::NetOpen(zone) => {
                self.net_draft = Some(NetDraft {
                    zone,
                    to: if zone == 0 {
                        "host".into()
                    } else {
                        String::new()
                    },
                    port: String::new(),
                    proto: None,
                    comment: String::new(),
                });
                self.net_error = None;
                self.focus = Some(NET_TO.into());
                return operation::focus(NET_TO);
            }
            Message::NetCancel => {
                self.net_draft = None;
                self.net_error = None;
            }
            Message::NetTo(v) => self.net_field(|d| d.to = v, NET_TO),
            Message::NetPort(v) => self.net_field(|d| d.port = v, NET_PORT),
            Message::NetComment(v) => self.net_field(|d| d.comment = v, NET_COMMENT),
            Message::NetProto(p) => self.net_field(|d| d.proto = p, ""),
            Message::NetAdd => {
                let Some(d) = self.net_draft.clone() else {
                    return Task::none();
                };
                let port = match d.port.trim() {
                    "" | "all" => None,
                    p => match p.parse::<u16>() {
                        Ok(n) if n > 0 => Some(n),
                        _ => {
                            self.net_error = Some(format!(
                                "{p:?} is not a port: use 1 to 65535, or leave it empty"
                            ));
                            return Task::none();
                        }
                    },
                };
                let comment = Some(d.comment.trim().to_owned()).filter(|c| !c.is_empty());
                let entry = Rule {
                    to: d.to.trim().to_owned(),
                    port,
                    proto: d.proto,
                    comment,
                };
                self.firewall(|fw| {
                    let z = zone_mut(fw, d.zone);
                    match z.mode {
                        ZoneMode::Allow => z.allow.push(entry),
                        ZoneMode::Block => z.block.push(entry),
                    }
                });
            }
            Message::NetRemove(z, i) => self.firewall(|fw| {
                let zone = zone_mut(fw, z);
                let list = match zone.mode {
                    ZoneMode::Allow => &mut zone.allow,
                    ZoneMode::Block => &mut zone.block,
                };
                if i < list.len() {
                    list.remove(i);
                }
            }),
            Message::NetAllow(z, address) => self.firewall(|fw| {
                let zone = zone_mut(fw, z);
                match zone.mode {
                    ZoneMode::Allow => zone.allow.push(Rule {
                        to: address,
                        port: None,
                        proto: None,
                        comment: None,
                    }),
                    ZoneMode::Block => zone.block.retain(|r| r.to != address),
                }
            }),
            Message::ProtectRemove(name) => {
                if let Some(c) = self.open_config() {
                    let names = c
                        .protected_paths
                        .names
                        .iter()
                        .filter(|n| **n != name)
                        .cloned()
                        .collect();
                    let folders = c.folders.clone();
                    self.set_folders(folders, Some(names));
                }
            }
        }
        Task::none()
    }

    /// The configuration of the sandbox whose page is open.
    fn open_config(&self) -> Option<&Config> {
        match &self.page {
            Page::Sandbox(id, _) => self.configs.get(id),
            _ => None,
        }
    }

    /// Changes the draft of the "Add a folder" card; `field` keeps the keyboard in the field being typed in.
    fn draft(&mut self, change: impl FnOnce(&mut FolderDraft), field: &str) {
        if let Some(d) = &mut self.folder_draft {
            change(d);
        }
        if !field.is_empty() {
            self.focus = Some(field.into());
        }
    }

    /// The theme: `--theme`, else the settings, else the system's.
    fn apply_theme(&mut self) {
        let setting = match self.settings.as_ref().map(|s| s.theme) {
            Some(Look::Light) => Some(false),
            Some(Look::Dark) => Some(true),
            _ => None,
        };
        self.dark = self.forced.or(setting).unwrap_or(self.system_dark);
        self.t = if self.dark { style::DARK } else { style::LIGHT };
    }

    /// The wizard's chosen profile configuration.
    pub fn wiz_config(&self) -> Option<&Config> {
        self.profile_configs
            .get(self.wizard.as_ref()?.profile.as_ref()?)
    }

    fn wiz_profile(&mut self, p: String) {
        if !self.profile_configs.contains_key(&p) {
            self.call(Method::Profile { id: p.clone() }, Then::Profile(p.clone()));
        }
        if let Some(w) = &mut self.wizard {
            w.profile = Some(p);
            w.base = None;
        }
        self.fields.remove(WIZ_NAME);
    }

    /// A name no sandbox has: "Claude", then "Claude-2"…
    pub fn free_name(&self, base: &str) -> String {
        let taken = |n: &str| {
            self.sandboxes
                .iter()
                .any(|s| s.name.eq_ignore_ascii_case(n) || s.id == slug(n))
        };
        if !taken(base) {
            return base.to_owned();
        }
        (2..)
            .map(|i| format!("{base}-{i}"))
            .find(|n| !taken(n))
            .expect("some name is free")
    }

    fn free_id(&self, name: &str) -> String {
        let base = slug(name);
        let taken = |id: &str| self.sandboxes.iter().any(|s| s.id == id);
        if !taken(&base) {
            return base;
        }
        (2..)
            .map(|i| {
                let suffix = format!("-{i}");
                format!(
                    "{}{suffix}",
                    base[..base.len().min(32 - suffix.len())].trim_end_matches('-')
                )
            })
            .find(|id| !taken(id))
            .expect("some id is free")
    }

    /// The configuration the wizard creates, and the location chosen.
    fn wiz_build(&self) -> Result<(Config, Option<String>), String> {
        let w = self.wizard.as_ref().ok_or("no wizard")?;
        let mut c = self.wiz_config().cloned().ok_or("pick a profile first")?;
        let name = self.value(WIZ_NAME);
        if name.is_empty() {
            return Err("give the sandbox a name (step 3)".into());
        }
        c.profile = w.profile.clone();
        c.id = self.free_id(&name);
        c.name = name;
        c.base_image = match &w.base {
            Some(b) => b.clone(),
            None if self.value(WIZ_IMAGE).is_empty() => c.base_image,
            None => self.value(WIZ_IMAGE),
        };
        c.icon = Some(Icon {
            letter: c
                .name
                .chars()
                .next()
                .map(|l| l.to_uppercase().collect())
                .unwrap_or_else(|| "S".into()),
            color: COLORS[w.color % COLORS.len()].to_owned(),
        });
        if c.trust != w.trust {
            c.apply_trust(w.trust);
        }
        c.folders = w.folders.clone();
        let join = |a: &str, b: String| {
            if b.is_empty() {
                a.to_owned()
            } else if a.trim().is_empty() {
                b + "\n"
            } else {
                format!("{}\n{b}\n", a.trim_end())
            }
        };
        c.setup.build = join(&c.setup.build, self.value(WIZ_BUILD));
        c.setup.first_start = join(&c.setup.first_start, self.value(WIZ_FIRST));
        let location = Some(self.value(WIZ_LOCATION)).filter(|l| !l.is_empty());
        Ok((c, location))
    }

    fn save_config(&mut self, id: &str, config: &Config) {
        match serde_json::to_string_pretty(config) {
            Ok(text) => self.call(
                Method::SetConfig {
                    id: id.to_owned(),
                    config: text,
                },
                Then::Action,
            ),
            Err(e) => self.notice = Some(e.to_string()),
        }
    }

    /// Sends the open sandbox's firewall, changed by `change`.
    fn firewall(&mut self, change: impl FnOnce(&mut Firewall)) {
        if let (Page::Sandbox(id, _), Some(c)) = (&self.page, self.open_config()) {
            let mut fw = c.firewall.clone();
            change(&mut fw);
            let id = id.clone();
            self.call(Method::SetFirewall { id, firewall: fw }, Then::Firewall);
        }
    }

    fn risk(&mut self, risk: Risk) {
        if let Page::Sandbox(id, _) = &self.page {
            self.dialog = Some(Dialog::Risk {
                id: id.clone(),
                risk,
                ack: false,
            });
            self.focus = Some("dlg:cancel".into());
        }
    }

    fn net_field(&mut self, change: impl FnOnce(&mut NetDraft), field: &str) {
        if let Some(d) = &mut self.net_draft {
            change(d);
        }
        if !field.is_empty() {
            self.focus = Some(field.into());
        }
    }

    fn set_folders(&mut self, folders: Vec<Folder>, protected: Option<Vec<String>>) {
        if let Page::Sandbox(id, _) = &self.page {
            let id = id.clone();
            self.call(
                Method::SetFolders {
                    id,
                    folders,
                    protected,
                },
                Then::Folders,
            );
        }
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
        if page != self.page {
            self.folder_draft = None;
            self.folder_error = None;
            self.net_draft = None;
            self.net_error = None;
        }
        match &page {
            Page::Profile(id) => self.call(
                Method::Profile { id: id.clone() },
                Then::Profile(id.clone()),
            ),
            Page::Settings => {
                if let Some(st) = &self.settings {
                    let location = st.sandbox_location.clone().unwrap_or_default();
                    let terminal = st
                        .terminal
                        .as_ref()
                        .map(|t| t.join(" "))
                        .unwrap_or_default();
                    self.fields.insert(SET_LOCATION, location);
                    self.fields.insert(SET_TERMINAL, terminal);
                }
                let ids: Vec<String> = self.sandboxes.iter().map(|s| s.id.clone()).collect();
                for id in ids {
                    self.call(Method::Disk { id: id.clone() }, Then::Disk(id));
                }
            }
            Page::Sandbox(id, Section::Advanced) => {
                self.call(Method::Disk { id: id.clone() }, Then::Disk(id.clone()));
                if let Some(c) = self.configs.get(id) {
                    let ram = c
                        .security
                        .ram_limit_gb
                        .map(|g| g.to_string())
                        .unwrap_or_default();
                    let langs = c.appearance.languages.join(", ");
                    self.fields.insert(ADV_RAM, ram);
                    self.fields.insert(ADV_LANGS, langs);
                    self.fields
                        .insert(ADV_EXPORT, format!("~/{id}.sandbox.json"));
                }
            }
            _ => {}
        }
        if let Page::Sandbox(id, Section::Network) = &page {
            self.call(Method::Net { id: id.clone() }, Then::Net(id.clone()));
        }
        if let Page::Sandbox(id, Section::Links) = &page {
            self.call(Method::Links { id: id.clone() }, Then::Schemes(id.clone()));
        }
        if let Page::Sandbox(id, Section::Packages) = &page {
            self.call(
                Method::Packages { id: id.clone() },
                Then::Packages(id.clone()),
            );
        }
        if let Page::Sandbox(id, Section::Apps) = &page
            && self.summary(id).is_some_and(|s| s.state == State::Running)
        {
            self.call(
                Method::DetectApps { id: id.clone() },
                Then::Detected(id.clone()),
            );
            self.call(
                Method::Services { id: id.clone() },
                Then::Services(id.clone()),
            );
        }
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
                self.call(Method::Settings {}, Then::Settings);
                self.checks = Some(cardea_daemon::doctor::desktop());
                self.call(Method::Doctor {}, Then::Doctor);
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

    fn on_reply(&mut self, then: Then, result: Result<Value, cardea_proto::Error>) {
        let value = match result {
            Ok(v) => v,
            Err(error) => {
                let e = error.message;
                match then {
                    Then::Unlock(pending) if error.code == ErrorCode::PasswordRequired => {
                        self.dialog = Some(Dialog::Password {
                            pending,
                            value: String::new(),
                        });
                        self.focus = Some(PASSWORD.into());
                    }
                    Then::Action | Then::Unlock(_) | Then::PackageChanged(_) => {
                        self.notice = Some(e)
                    }
                    // The daemon lists each problem on its own line, after a heading.
                    Then::Folders if self.folder_draft.is_some() => {
                        self.folder_error = Some(
                            e.lines()
                                .skip(1)
                                .map(|l| l.trim().trim_start_matches("- "))
                                .collect::<Vec<_>>()
                                .join("; "),
                        )
                    }
                    Then::Folders => self.notice = Some(e),
                    Then::Firewall if self.net_draft.is_some() => {
                        self.net_error = Some(
                            e.lines()
                                .skip(1)
                                .map(|l| l.trim().trim_start_matches("- "))
                                .collect::<Vec<_>>()
                                .join("; "),
                        )
                    }
                    Then::Firewall => self.notice = Some(e),
                    _ => {}
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
                    self.missing.insert(id.clone(), s.missing_folders);
                    self.configs.insert(id, s.config);
                }
            }
            Then::Folders => {
                self.folder_draft = None;
                self.folder_error = None;
            }
            Then::Firewall => {
                self.net_draft = None;
                self.net_error = None;
            }
            Then::Net(id) => {
                if let Some(n) = parse::<NetStatus>(value) {
                    self.net.insert(id, n);
                }
            }
            Then::Detected(id) => {
                if let Some(d) = parse::<Vec<Detected>>(value) {
                    self.detected.insert(id, d);
                }
            }
            Then::Services(id) => {
                if let Some(list) = parse::<Vec<Instance>>(value) {
                    self.services.insert(id, list);
                }
            }
            Then::Schemes(id) => {
                if let Some(list) = parse(value) {
                    self.schemes.insert(id, list);
                }
            }
            Then::Unlock(_) => {}
            Then::Profile(id) => {
                if let Some(c) = parse::<Config>(value) {
                    self.profile_configs.insert(id, c);
                }
            }
            Then::Created => {
                if let (Some(s), Some(w)) = (parse::<Summary>(value), &mut self.wizard) {
                    w.created = Some(s.id);
                    w.step = 8;
                }
            }
            Then::Settings => {
                if let Some(s) = parse::<Settings>(value) {
                    self.settings = Some(s);
                    self.apply_theme();
                }
            }
            Then::Doctor => {
                if let Some(list) = parse::<Vec<Check>>(value) {
                    let mut checks = cardea_daemon::doctor::desktop();
                    checks.extend(list);
                    self.checks = Some(checks);
                }
            }
            Then::Disk(id) => {
                if let Some(d) = parse::<Disk>(value) {
                    self.disks.insert(id, d);
                }
            }
            Then::ProfilesChanged => {
                self.import = None;
                self.call(Method::Profiles {}, Then::Profiles);
                if matches!(self.page, Page::Import | Page::Profile(_)) {
                    self.page = Page::Profiles;
                }
            }
            Then::Packages(id) => {
                if let Some(list) = parse::<Vec<PackageInfo>>(value) {
                    self.packages.insert(id, list);
                }
            }
            Then::PackageChanged(id) => {
                self.call(Method::Packages { id: id.clone() }, Then::Packages(id))
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
                    self.net.remove(&id);
                    self.services.remove(&id);
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
            Event::Net { id, status } => {
                self.net.insert(id, status);
            }
            Event::Services { id, services } => {
                self.services.insert(id, services);
            }
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
        let open = match &self.dialog {
            Some(
                Dialog::Stop(id)
                | Dialog::Delete { id, .. }
                | Dialog::Risk { id, .. }
                | Dialog::Change { id, .. },
            ) => Some(id.clone()),
            Some(Dialog::Password { pending, .. }) => Some(pending.id().to_owned()),
            None => None,
        };
        if open.is_some_and(|id| self.summary(&id).is_none()) {
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

fn zone_mut(fw: &mut Firewall, z: usize) -> &mut Zone {
    match z {
        0 => &mut fw.host,
        1 => &mut fw.lan,
        _ => &mut fw.wan,
    }
}

/// "My Box" gives "my-box": the id the daemon would make from a name.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for ch in name.trim().to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.truncate(32);
    let out = out.trim_end_matches('-').to_owned();
    if out.is_empty() {
        "sandbox".into()
    } else {
        out
    }
}
