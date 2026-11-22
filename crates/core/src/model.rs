//! Data model: sandbox configuration (`sandbox.json`), profiles, tool settings and package sources.
//!
//! A sandbox and a profile share one type, [`Config`]: a profile is a config without the local fields
//! (`profile`, `folders`, `home.path`, `identity.hostname`, `autostart`, services' `autostart`, IP addresses in
//! firewall entries). [`crate::validate`] enforces the difference with [`Kind`].

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};

/// Version of the `sandbox.json`, profile and settings formats.
pub const SCHEMA: u32 = 1;

/// Base image used when a config does not name one.
pub const DEFAULT_BASE_IMAGE: &str = "docker.io/library/debian:trixie-slim";

/// What a [`Config`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Sandbox,
    Profile,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Sandbox => "sandbox",
            Kind::Profile => "profile",
        }
    }
}

/// A sandbox configuration or a profile.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema: u32,
    /// Sandbox id, or profile id for a profile.
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// `None`: derived from the name by the clients.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<Icon>,
    /// Sandbox only: the profile it was created from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    pub base_image: String,
    pub trust: Trust,
    pub identity: Identity,
    pub home: Home,
    /// Sandbox only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<Folder>,
    pub protected_paths: ProtectedPaths,
    pub capabilities: Capabilities,
    pub security: Security,
    pub firewall: Firewall,
    pub sources: Vec<Source>,
    pub apt_repos: Vec<AptRepo>,
    pub packages: Vec<Package>,
    pub setup: Setup,
    pub apps: Vec<App>,
    pub services: Vec<Service>,
    /// Links: incoming ones reach the apps that declare their scheme, outgoing ones may leave for the host browser.
    #[serde(default, skip_serializing_if = "Links::is_default")]
    pub links: Links,
    pub appearance: Appearance,
    /// Sandbox only: start at login.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autostart: Option<bool>,
}

impl Config {
    /// A config with every setting at its default for `trust`. `id` and `name` are empty.
    pub fn defaults(trust: Trust) -> Config {
        let mut config = Config {
            schema: SCHEMA,
            id: String::new(),
            name: String::new(),
            description: None,
            icon: None,
            profile: None,
            base_image: DEFAULT_BASE_IMAGE.to_owned(),
            trust,
            identity: Identity::default(),
            home: Home::default(),
            folders: Vec::new(),
            protected_paths: ProtectedPaths::default(),
            capabilities: Capabilities::default(),
            security: Security::default(),
            firewall: Firewall::default(),
            sources: Vec::new(),
            apt_repos: Vec::new(),
            packages: Vec::new(),
            setup: Setup::default(),
            apps: Vec::new(),
            services: Vec::new(),
            links: Links::default(),
            appearance: Appearance::default(),
            autostart: None,
        };
        config.apply_trust(trust);
        config
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Icon {
    /// One letter or digit.
    pub letter: String,
    /// `#rrggbb`.
    pub color: String,
}

/// Trust level: a preset over a group of settings (see [`crate::trust`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trust {
    Work,
    Untrusted,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub user: String,
    pub home_mount: String,
    /// Sandbox only. `None`: the sandbox id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
}

impl Default for Identity {
    fn default() -> Self {
        Identity {
            user: "sandbox".into(),
            home_mount: "/home/sandbox".into(),
            hostname: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Home {
    /// Sandbox only. `None`: `<sandbox folder>/home`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Wiped at each start.
    pub disposable: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Folder {
    /// Absolute path on the host.
    pub host: String,
    /// Absolute path inside the sandbox.
    pub mount: String,
    pub mode: Mode,
}

impl Folder {
    /// The folder's name: last component of its mount point (used by per-folder services).
    pub fn name(&self) -> &str {
        self.mount.rsplit('/').next().unwrap_or_default()
    }

    /// The mount point proposed for a host folder: `/work/<its name>`.
    pub fn default_mount(host: &str) -> String {
        format!(
            "/work/{}",
            host.trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or_default()
        )
    }

    /// The access proposed for a new folder: read-only in an Untrusted sandbox (R6).
    pub fn default_mode(trust: Trust) -> Mode {
        match trust {
            Trust::Work => Mode::Rw,
            Trust::Untrusted => Mode::Ro,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Ro,
    Rw,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtectedPaths {
    /// Relative paths re-mounted read-only inside read-write shared folders.
    pub names: Vec<String>,
    /// How deep they are looked for under each shared folder.
    pub depth: u8,
}

impl Default for ProtectedPaths {
    fn default() -> Self {
        let names = [
            ".git/hooks",
            ".git/config",
            ".obsidian",
            ".vscode",
            ".envrc",
            ".husky",
            ".cargo/config.toml",
            ".devcontainer",
        ];
        ProtectedPaths {
            names: names.map(String::from).to_vec(),
            depth: 5,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub wayland: bool,
    pub x11: X11,
    pub gpu: bool,
    pub kvm: bool,
    pub keyring: Keyring,
    /// What the sandbox's apps get of the desktop clipboard (needs `wayland`).
    pub clipboard: Clipboard,
}

impl Default for Capabilities {
    fn default() -> Self {
        Capabilities {
            wayland: true,
            x11: X11::Off,
            gpu: false,
            kvm: false,
            keyring: Keyring::default(),
            clipboard: Clipboard::Shared,
        }
    }
}

impl Capabilities {
    pub fn is_on(&self, cap: Capability) -> bool {
        match cap {
            Capability::Wayland => self.wayland,
            Capability::X11 => self.x11 != X11::Off,
            Capability::Gpu => self.gpu,
            Capability::Kvm => self.kvm,
            Capability::Keyring => self.keyring.mode != KeyringMode::Off,
        }
    }
}

/// A capability, as named in `wants` and `when`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Wayland,
    X11,
    Gpu,
    Kvm,
    Keyring,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum X11 {
    Off,
    /// Private Xwayland inside the sandbox.
    Isolated,
    /// The host's X server (risky).
    Host,
}

/// The desktop clipboard, as the Wayland filter gives it. Background access (clipboard managers) is never given.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Clipboard {
    /// Copy and paste with your other apps, while a sandbox window has the focus.
    Shared,
    /// No clipboard between apps (nor drag and drop).
    Off,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keyring {
    pub mode: KeyringMode,
    /// Disposable home only: keep the keyring across starts.
    pub keep: bool,
}

impl Default for Keyring {
    fn default() -> Self {
        Keyring {
            mode: KeyringMode::Auto,
            keep: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyringMode {
    Off,
    Auto,
    Ask,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Security {
    pub seccomp: bool,
    /// Isolated UID (`--userns=auto`) instead of the user's own (`keep-id`).
    pub isolated_uid: bool,
    pub ram_limit_gb: Option<u32>,
}

impl Default for Security {
    fn default() -> Self {
        Security {
            seccomp: true,
            isolated_uid: false,
            ram_limit_gb: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Firewall {
    pub enabled: bool,
    pub blocked: Blocked,
    pub host: Zone,
    pub lan: Zone,
    pub wan: Zone,
}

impl Default for Firewall {
    /// No gateway in LAN: the router's admin page and UPnP are a target, and the Internet does not need it.
    fn default() -> Self {
        Firewall {
            enabled: true,
            blocked: Blocked::Reject,
            host: Zone::new(ZoneMode::Allow, Vec::new()),
            lan: Zone::new(ZoneMode::Allow, Vec::new()),
            wan: Zone::new(ZoneMode::Block, Vec::new()),
        }
    }
}

/// What blocked traffic gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Blocked {
    Reject,
    Drop,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Zone {
    pub mode: ZoneMode,
    /// Used in allowlist mode.
    pub allow: Vec<Rule>,
    /// Used in blocklist mode.
    pub block: Vec<Rule>,
}

impl Zone {
    fn new(mode: ZoneMode, allow: Vec<Rule>) -> Zone {
        Zone {
            mode,
            allow,
            block: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZoneMode {
    Allow,
    Block,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// A reference (`host`, `gateway`, `dns`) or an IP address / CIDR (sandbox only).
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// `None`: TCP and UDP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proto: Option<Proto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Proto {
    Tcp,
    Udp,
}

/// A package manager, as data. Built-in sources use the same format.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub id: String,
    pub name: String,
    pub scope: Scope,
    /// Installs the manager itself, once. Empty: nothing to do.
    #[serde(default)]
    pub setup: String,
    /// Shell templates run inside the sandbox as `sh -c '<template>' sh <name> <version>`.
    pub install: String,
    pub remove: String,
    pub update: String,
    /// Prints `name version` lines.
    pub list: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// As root at build time.
    Image,
    /// As the sandbox user in the running sandbox.
    Home,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AptRepo {
    pub name: String,
    pub url: String,
    pub suite: String,
    pub components: Vec<String>,
    pub key_url: String,
    pub fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    pub source: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<When>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct When {
    pub capability: Capability,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Setup {
    /// Shell script run as root at build time.
    pub build: String,
    /// Shell script run as the user at the first start.
    pub first_start: String,
    pub on_failure: OnFailure,
}

impl Default for Setup {
    fn default() -> Self {
        Setup {
            build: String::new(),
            first_start: String::new(),
            on_failure: OnFailure::Stop,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnFailure {
    Stop,
    Continue,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct App {
    pub id: String,
    pub name: String,
    pub exec: Vec<String>,
    pub toolkit: Toolkit,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wants: Vec<Capability>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub desktop_entry: bool,
    /// Icon name or absolute path inside the sandbox, for the host menu entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Window class or Wayland app id of its windows, so the taskbar groups them under the menu entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wm_class: Option<String>,
    /// URL schemes it opens (`claude`): a link of one of them clicked on the host reaches this app.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schemes: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Toolkit {
    Qt,
    Gtk,
    Electron,
    Terminal,
    X11,
    Other,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Service {
    pub id: String,
    pub name: String,
    pub exec: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// One instance per shared folder; `{folder}` in `exec` becomes the folder's name.
    #[serde(default)]
    pub per_folder: bool,
    /// Sandbox only: `true`/`false` for a single instance, folder names for a per-folder service.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autostart: Option<Autostart>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Autostart {
    Single(bool),
    Folders(Vec<String>),
}

impl<'de> Deserialize<'de> for Autostart {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde_json::Value;
        let wrong = || serde::de::Error::custom("expected true, false or a list of folder names");
        match Value::deserialize(d)? {
            Value::Bool(b) => Ok(Autostart::Single(b)),
            Value::Array(items) => items
                .into_iter()
                .map(|v| match v {
                    Value::String(s) => Ok(s),
                    _ => Err(wrong()),
                })
                .collect::<Result<_, _>>()
                .map(Autostart::Folders),
            _ => Err(wrong()),
        }
    }
}

/// Outgoing links: from the sandbox's apps to the host's default browser, through the link relay.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Links {
    /// Links may leave the sandbox (applies at the next start).
    #[serde(default)]
    pub outgoing: bool,
    /// Ask before opening each link (always asked for an Untrusted sandbox).
    #[serde(default = "yes")]
    pub ask: bool,
    /// Hosts opened without asking (exact names, never their subdomains); ignored for an Untrusted sandbox.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed: Vec<String>,
}

fn yes() -> bool {
    true
}

impl Default for Links {
    fn default() -> Self {
        Links {
            outgoing: false,
            ask: true,
            allowed: Vec::new(),
        }
    }
}

impl Links {
    fn is_default(&self) -> bool {
        *self == Links::default()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Appearance {
    pub theme: Theme,
    /// Language codes such as `fr` or `en_US`, first is the main one.
    pub languages: Vec<String>,
}

impl Default for Appearance {
    fn default() -> Self {
        Appearance {
            theme: Theme::System,
            languages: vec!["en".into()],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    System,
    Light,
    Dark,
}

/// Tool settings (`~/.config/cardea/settings.json`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub schema: u32,
    /// Where new sandboxes go. `None`: `~/Sandboxes`.
    pub sandbox_location: Option<String>,
    pub tray_at_login: bool,
    /// Theme of the manager and the tray.
    pub theme: Theme,
    /// Host terminal command (argv prefix, the command to run is appended). `None`: detected.
    pub terminal: Option<Vec<String>>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            schema: SCHEMA,
            sandbox_location: None,
            tray_at_login: true,
            theme: Theme::System,
            terminal: None,
        }
    }
}
