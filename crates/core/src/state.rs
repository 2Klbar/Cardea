//! Sandbox lifecycle: states, flags and which actions each state allows.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Building,
    BuildFailed,
    Stopped,
    Starting,
    Running,
    Stopping,
    Rebuilding,
    Error,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            State::Building => "building",
            State::BuildFailed => "build failed",
            State::Stopped => "stopped",
            State::Starting => "starting",
            State::Running => "running",
            State::Stopping => "stopping",
            State::Rebuilding => "rebuilding",
            State::Error => "error",
        }
    }

    /// A task is moving the sandbox out of this state.
    pub fn is_transient(self) -> bool {
        matches!(
            self,
            State::Building | State::Starting | State::Stopping | State::Rebuilding
        )
    }
}

/// Flags shown on top of Stopped or Running.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Flags {
    pub restart_required: bool,
    pub rebuild_required: bool,
}

/// What a user can ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
    Restart,
    Rebuild,
    Delete,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::Start => "start",
            Action::Stop => "stop",
            Action::Restart => "restart",
            Action::Rebuild => "rebuild",
            Action::Delete => "delete",
        }
    }
}

/// Whether `action` is allowed in `state`; the error says why not and what to do.
pub fn check(state: State, action: Action) -> Result<(), String> {
    use Action::*;
    use State::*;
    let allowed = match action {
        // From Error, start is a retry.
        Start => matches!(state, Stopped | Error),
        // From Error, stop resets the sandbox to Stopped.
        Stop => matches!(state, Running | Error),
        Restart => matches!(state, Running),
        Rebuild => matches!(state, Stopped | Running | BuildFailed),
        Delete => matches!(state, Stopped | BuildFailed | Error),
    };
    if allowed {
        return Ok(());
    }
    let hint = match (action, state) {
        (_, s) if s.is_transient() => "wait for the current task to finish",
        (Start, Running) => "it is already running",
        (Start, BuildFailed) => "rebuild it first",
        (Stop | Restart, Stopped) => "it is not running",
        (Restart, _) => "start it instead",
        (Delete, Running) => "stop it first",
        _ => "not possible now",
    };
    Err(format!(
        "cannot {} a sandbox that is {}: {hint}",
        action.label(),
        state.label()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_rules() {
        assert!(check(State::Stopped, Action::Start).is_ok());
        assert!(check(State::Error, Action::Start).is_ok());
        assert!(check(State::Error, Action::Stop).is_ok());
        assert!(check(State::BuildFailed, Action::Rebuild).is_ok());
        assert_eq!(
            check(State::Running, Action::Delete).unwrap_err(),
            "cannot delete a sandbox that is running: stop it first"
        );
        assert!(
            check(State::Building, Action::Stop)
                .unwrap_err()
                .contains("wait")
        );
        assert!(check(State::Running, Action::Start).is_err());
    }
}
