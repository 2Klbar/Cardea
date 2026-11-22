//! Cardea control socket protocol: one JSON object per line.
//!
//! - Client → daemon: [`Request`] `{"id": 1, "method": "start", "params": {"id": "claude"}}`.
//! - Daemon → client: [`Response`] `{"id": 1, "result": …}` or `{"id": 1, "error": {"code", "message"}}`, and,
//!   after `subscribe` or `logs` with `follow`, [`Event`] lines `{"event": {"type": …}}`.
//! - The first request must be `hello`, which checks the protocol version.

use cardea_core::firewall::NetStatus;
use cardea_core::model::{Firewall, Folder, Icon, Package, Settings};
use cardea_core::services::Instance;
use cardea_core::state::{Flags, State};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Protocol version checked during the first exchange between a client and the daemon.
pub const PROTOCOL_VERSION: u32 = 3;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    #[serde(flatten)]
    pub method: Method,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Method {
    /// Result: [`Hello`].
    Hello {
        version: u32,
    },
    /// Forward every [`Event`] to this connection. Result: `null`.
    Subscribe {},
    /// Result: `Vec<`[`Summary`]`>`.
    List {},
    /// Result: [`Status`].
    Status {
        id: String,
    },
    /// Result: `Vec<`[`ProfileInfo`]`>`.
    Profiles {},
    /// Result: the profile config, every default filled in.
    Profile {
        id: String,
    },
    /// From a profile (`profile` and `name`) or from a whole config (`config`, JSON text). Starts the build.
    /// Result: [`Summary`].
    Create {
        #[serde(default)]
        profile: Option<String>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        config: Option<String>,
        /// The folder that holds the sandbox's folder (`~/` allowed); default: the settings' location.
        #[serde(default)]
        location: Option<String>,
    },
    /// Saves a profile file (JSON text) in the user's profiles; `replace` overwrites one with the same id.
    /// Result: [`ProfileInfo`].
    ProfileImport {
        config: String,
        #[serde(default)]
        replace: bool,
    },
    /// Deletes a user profile. Result: `null`.
    ProfileDelete {
        id: String,
    },
    /// Saves a sandbox's configuration as a user profile, without what is local to it. Result: [`ProfileInfo`].
    ProfileSave {
        id: String,
        profile: String,
        name: String,
    },
    /// Result: the tool settings.
    Settings {},
    /// Validates and saves the tool settings. Result: `null`.
    SetSettings {
        settings: Settings,
    },
    /// Disk use of a sandbox's folder and home. Result: [`Disk`].
    Disk {
        id: String,
    },
    /// Starts a task. Result: `null`; follow it with [`Event::Task`] and [`Event::TaskEnded`].
    Start {
        id: String,
        /// The keyring password, for a sandbox whose keyring is `ask` (see [`ErrorCode::PasswordRequired`]).
        #[serde(default)]
        password: Option<String>,
    },
    Stop {
        id: String,
    },
    Restart {
        id: String,
        /// The keyring password, for a sandbox whose keyring is `ask` (see [`ErrorCode::PasswordRequired`]).
        #[serde(default)]
        password: Option<String>,
    },
    /// A running sandbox is restarted after the build. `fresh`: pull the base image again and use no cached layer,
    /// which updates the image's packages.
    Rebuild {
        id: String,
        #[serde(default)]
        fresh: bool,
        /// The keyring password, for a sandbox whose keyring is `ask` (see [`ErrorCode::PasswordRequired`]).
        #[serde(default)]
        password: Option<String>,
    },
    /// Result: `null`.
    Delete {
        id: String,
        #[serde(default)]
        keep_home: bool,
    },
    /// Result: the `sandbox.json` text.
    GetConfig {
        id: String,
    },
    /// Validates and saves a new `sandbox.json` text. Result: [`Saved`].
    SetConfig {
        id: String,
        config: String,
    },
    /// Result: `Vec<String>`, the last log lines. With `follow`, [`Event::Log`] lines for this sandbox follow,
    /// without gap or duplicate.
    Logs {
        id: String,
        #[serde(default)]
        follow: bool,
    },
    /// Checks Podman on the host. Result: `Vec<`[`Check`]`>`.
    Doctor {},
    /// Replaces the shared folders and, when given, the protected path names. Result: [`Saved`].
    SetFolders {
        id: String,
        folders: Vec<Folder>,
        #[serde(default)]
        protected: Option<Vec<String>>,
    },
    /// Replaces the firewall settings; a running sandbox gets them at once. Result: [`Saved`].
    SetFirewall {
        id: String,
        firewall: Firewall,
    },
    /// The live firewall state of a running sandbox. Result: `NetStatus` (empty when stopped).
    Net {
        id: String,
    },
    /// Launches an app in the background, starting the sandbox first when it is stopped. Terminal apps are run by
    /// the client through `exec` with `app`. Result: `null`.
    Launch {
        id: String,
        app: String,
        #[serde(default)]
        args: Vec<String>,
        /// A link of one of the app's schemes (R4): checked, then given as the app's last argument.
        #[serde(default)]
        url: Option<String>,
        /// The keyring password, for a sandbox whose keyring is `ask` (see [`ErrorCode::PasswordRequired`]).
        #[serde(default)]
        password: Option<String>,
    },
    /// The schemes the sandbox's apps declare and who opens them on the host. Result: `Vec<`[`SchemeInfo`]`>`.
    Links {
        id: String,
    },
    /// A link a running sandbox wants to open in the host browser (sent by its link relay, R3). Answered once
    /// decided: opened, refused, or asked in the link window and answered there. Result: [`LinkOutcome`].
    Link {
        sandbox: String,
        url: String,
    },
    /// A link waiting for the owner's answer, read by the link window. Result: [`LinkAsk`].
    LinkRequest {
        id: u64,
    },
    /// The owner's answer from the link window. Result: `null`.
    LinkAnswer {
        id: u64,
        open: bool,
        /// Stop asking for this sandbox's links (not offered for an Untrusted sandbox).
        #[serde(default)]
        always_sandbox: bool,
        /// Open this host's links of this sandbox without asking (not offered for a suspicious link).
        #[serde(default)]
        always_domain: bool,
    },
    /// The packages with the versions installed in the running sandbox. Result: `Vec<PackageInfo>`.
    Packages {
        id: String,
    },
    /// Adds a package; a home package is installed at once in a running sandbox (an image package needs a
    /// rebuild). Result: [`Saved`].
    PackageAdd {
        id: String,
        package: Package,
    },
    /// Removes a package; a home package is uninstalled at once in a running sandbox. Result: [`Saved`].
    PackageRemove {
        id: String,
        source: String,
        name: String,
    },
    /// Updates the home packages of a running sandbox: all, those of one source, or one. Image packages are
    /// updated by a fresh rebuild. Result: `null`.
    PackageUpdate {
        id: String,
        #[serde(default)]
        source: Option<String>,
        #[serde(default)]
        name: Option<String>,
    },
    /// The apps of a running sandbox's `.desktop` files. Result: `Vec<Detected>`.
    DetectApps {
        id: String,
    },
    /// The service instances with their state. Result: `Vec<Instance>`.
    Services {
        id: String,
    },
    /// Starts (again) or stops a service instance. Result: `null`.
    Service {
        id: String,
        service: String,
        #[serde(default)]
        folder: Option<String>,
        start: bool,
    },
    /// Runs a command (default: the login shell) in a running sandbox. Result: [`ExecStarted`]; the response
    /// line carries the command's connection as a file descriptor (`SCM_RIGHTS`). Without `tty` its output is
    /// multiplexed: 8-byte frame headers, stream 1 = stdout, 2 = stderr.
    Exec {
        id: String,
        #[serde(default)]
        cmd: Option<Vec<String>>,
        /// Runs this app (with its toolkit's environment) instead of `cmd`.
        #[serde(default)]
        app: Option<String>,
        /// With `app`: once, without X server, every toolkit told to use Wayland.
        #[serde(default)]
        test_wayland: bool,
        #[serde(default)]
        root: bool,
        #[serde(default)]
        workdir: Option<String>,
        #[serde(default)]
        tty: bool,
        #[serde(default)]
        rows: u16,
        #[serde(default)]
        cols: u16,
        #[serde(default)]
        term: Option<String>,
    },
    /// Waits for a command to end. Result: its exit code.
    ExecWait {
        exec: String,
    },
    /// The caller's terminal changed size. Result: `null`.
    ExecResize {
        exec: String,
        rows: u16,
        cols: u16,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Response {
    Ok { id: u64, result: Value },
    Err { id: Option<u64>, error: Error },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The line is not a valid request.
    BadRequest,
    /// No `hello` yet, or another protocol version.
    Version,
    /// Another user's connection.
    Forbidden,
    NotFound,
    AlreadyExists,
    /// The action is not allowed in the current state, or a task is running.
    InvalidState,
    /// The configuration is not valid; the message lists every problem.
    InvalidConfig,
    /// The sandbox's keyring asks for its password: send it again with `password`.
    PasswordRequired,
    /// No engine can do this (Podman engine not available).
    Unavailable,
    Internal,
}

/// A line sent by the daemon.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Incoming {
    Event { event: Event },
    Response(Response),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Added {
        sandbox: Summary,
    },
    Removed {
        id: String,
    },
    /// State, flags, error or config changed.
    Changed {
        sandbox: Summary,
    },
    /// Task progress; `task: null` when it ends.
    Task {
        id: String,
        task: Option<Task>,
    },
    /// A task ended: `error` is `None` on success.
    TaskEnded {
        id: String,
        label: String,
        error: Option<String>,
    },
    Log {
        id: String,
        line: String,
    },
    /// The firewall state of a running sandbox changed (references, counters, blocked destinations).
    Net {
        id: String,
        status: NetStatus,
    },
    /// The service instances of a running sandbox changed state.
    Services {
        id: String,
        services: Vec<Instance>,
    },
    /// Resource use of a running sandbox, about once a second.
    Stats {
        id: String,
        /// Percent of one CPU core: 250 is two and a half cores.
        cpu: f32,
        /// Bytes.
        memory: u64,
        /// Bytes; `None` when there is no limit.
        memory_limit: Option<u64>,
    },
}

/// One line of `cardea doctor`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    pub status: CheckStatus,
    pub detail: String,
    /// What to do about it, when the check does not pass.
    #[serde(default)]
    pub hint: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub version: u32,
    pub daemon_version: String,
    /// Why no sandbox can start (Podman missing…); `None` when the engine works.
    #[serde(default)]
    pub engine_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub id: String,
    pub name: String,
    pub icon: Option<Icon>,
    pub state: State,
    pub flags: Flags,
    /// Why the sandbox is in Error or Build failed.
    pub error: Option<String>,
    pub task: Option<Task>,
    /// Risky settings that are on.
    pub warnings: Vec<String>,
    /// The sandbox folder.
    pub folder: String,
}

/// A long task: "Step 3/6 · other packages", 45 %.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    /// What the task does: "build", "start"…
    pub label: String,
    pub step: u32,
    pub steps: u32,
    pub step_label: String,
    pub percent: u8,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub summary: Summary,
    pub config: cardea_core::model::Config,
    /// Host paths of the shared folders that do not exist (skipped at start).
    #[serde(default)]
    pub missing_folders: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecStarted {
    pub exec: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProfileInfo {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub builtin: bool,
    #[serde(default)]
    pub icon: Option<Icon>,
    #[serde(default)]
    pub base_image: String,
    #[serde(default)]
    pub packages: usize,
    #[serde(default)]
    pub apps: usize,
    /// Names of the sandboxes made from it.
    #[serde(default)]
    pub used_by: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Disk {
    /// Bytes in the sandbox folder (its home included when it is inside).
    pub folder: u64,
    pub home: u64,
}

/// A scheme an app of the sandbox declares, and who opens it on the host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemeInfo {
    pub scheme: String,
    pub app: String,
    pub app_name: String,
    pub status: SchemeStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "by", rename_all = "snake_case")]
pub enum SchemeStatus {
    /// Its links open in this sandbox's app.
    Active,
    /// A host app handles it (its menu entry's file name): Cardea leaves it alone.
    HostApp(String),
    /// Another sandbox declared it first (its name).
    OtherSandbox(String),
    /// No default handler is known (no `xdg-mime` on this host, or not set yet).
    Unknown,
}

/// What became of a link sent by a sandbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkOutcome {
    Opened,
    Cancelled,
    /// Not answered in time.
    Expired,
}

/// A link waiting for the owner's answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkAsk {
    pub id: u64,
    pub sandbox: String,
    pub sandbox_name: String,
    pub link: cardea_core::links::Outgoing,
    /// The window is red, always asks, offers no "Always" box.
    pub untrusted: bool,
}

impl LinkAsk {
    /// "Always for this sandbox" is offered.
    pub fn sandbox_box(&self) -> bool {
        !self.untrusted
    }

    /// "Always for <domain> in this sandbox" is offered.
    pub fn domain_box(&self) -> bool {
        !self.untrusted && self.link.marks.is_empty()
    }
}

/// Result of `set_config`, `set_folders` and `set_firewall`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Saved {
    Applied,
    RestartRequired,
    RebuildRequired,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_format() {
        let r: Request =
            serde_json::from_str(r#"{"id":1,"method":"start","params":{"id":"claude"}}"#).unwrap();
        assert_eq!(
            r.method,
            Method::Start {
                id: "claude".into(),
                password: None
            }
        );
        let r: Request = serde_json::from_str(r#"{"id":2,"method":"list","params":{}}"#).unwrap();
        assert_eq!(r.method, Method::List {});
        let ok = serde_json::to_string(&Response::Ok {
            id: 1,
            result: Value::Null,
        })
        .unwrap();
        assert_eq!(ok, r#"{"id":1,"result":null}"#);
        let line = r#"{"event":{"type":"removed","id":"x"}}"#;
        assert_eq!(
            serde_json::from_str::<Incoming>(line).unwrap(),
            Incoming::Event {
                event: Event::Removed { id: "x".into() }
            }
        );
        let line = r#"{"id":3,"error":{"code":"not_found","message":"no"}}"#;
        assert!(matches!(
            serde_json::from_str::<Incoming>(line).unwrap(),
            Incoming::Response(Response::Err { .. })
        ));
    }
}
