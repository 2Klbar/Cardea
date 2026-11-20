//! Cardea control socket protocol: one JSON object per line.
//!
//! - Client → daemon: [`Request`] `{"id": 1, "method": "start", "params": {"id": "claude"}}`.
//! - Daemon → client: [`Response`] `{"id": 1, "result": …}` or `{"id": 1, "error": {"code", "message"}}`, and,
//!   after `subscribe` or `logs` with `follow`, [`Event`] lines `{"event": {"type": …}}`.
//! - The first request must be `hello`, which checks the protocol version.

use cardea_core::model::Icon;
use cardea_core::state::{Flags, State};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Protocol version checked during the first exchange between a client and the daemon.
pub const PROTOCOL_VERSION: u32 = 1;

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
    },
    /// Starts a task. Result: `null`; follow it with [`Event::Task`] and [`Event::TaskEnded`].
    Start {
        id: String,
    },
    Stop {
        id: String,
    },
    Restart {
        id: String,
    },
    Rebuild {
        id: String,
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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub version: u32,
    pub daemon_version: String,
    /// The daemon runs the demo engine.
    pub demo: bool,
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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileInfo {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub builtin: bool,
}

/// Result of `set_config`.
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
                id: "claude".into()
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
