//! Cardea daemon: the only process that talks to the engine. It owns the sandboxes' files and states, runs
//! tasks and serves the control socket.

pub mod daemon;
pub mod doctor;
pub mod menu;
pub mod paths;
pub mod server;
pub mod store;
pub mod terminal;
pub mod wayland;

use std::os::unix::fs::MetadataExt;
use std::sync::Arc;

use cardea_core::image::Ids;
use cardea_engine::Engine;
use cardea_engine::podman_engine::PodmanEngine;

/// `cardea daemon`: runs until stopped.
pub fn main() -> Result<(), String> {
    // Started by a client, outside the unit: hand over to the user unit when it is installed, so one daemon runs,
    // owned by the session. The cgroup tells, not INVOCATION_ID: an autostarted tray runs in a systemd unit of its
    // own, and its children inherit that variable.
    let in_unit = std::fs::read_to_string("/proc/self/cgroup").is_ok_and(|c| in_cardea_unit(&c));
    if !in_unit && start_unit() {
        return Ok(());
    }
    let paths = paths::Paths::from_env()?;
    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let listener = match server::bind(&paths.runtime) {
            // The unit must not restart in a loop behind a daemon that already serves (RestartPreventExitStatus).
            Err(e) if in_unit && e == server::ALREADY_RUNNING => {
                eprintln!("error: {e}");
                std::process::exit(ALREADY_RUNNING_EXIT)
            }
            r => r?,
        };
        let engine: Arc<dyn Engine> = {
            let me = std::fs::metadata("/proc/self").map_err(|e| e.to_string())?;
            Arc::new(PodmanEngine::new(
                &paths.podman,
                &paths.runtime,
                Ids {
                    uid: me.uid(),
                    gid: me.gid(),
                },
            ))
        };
        let daemon = daemon::Daemon::open(paths.clone(), engine)?;
        eprintln!(
            "cardea daemon {} listening on {}",
            cardea_core::VERSION,
            paths.socket().display(),
        );
        server::run(daemon, listener).await
    })
}

/// Exit status of the unit's daemon when another daemon already serves; the unit does not restart on it.
pub const ALREADY_RUNNING_EXIT: i32 = 3;

/// Whether `/proc/self/cgroup` places this process in `cardea.service`.
fn in_cardea_unit(cgroup: &str) -> bool {
    cgroup
        .lines()
        .any(|l| l.rsplit('/').next() == Some("cardea.service"))
}

/// `systemctl --user start cardea.service`; false when there is no such unit or no user manager.
fn start_unit() -> bool {
    std::process::Command::new("systemctl")
        .args(["--user", "--quiet", "start", "cardea.service"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::in_cardea_unit;

    #[test]
    fn unit_from_cgroup() {
        assert!(in_cardea_unit(
            "0::/user.slice/user-1000.slice/user@1000.service/app.slice/cardea.service\n"
        ));
        // The tray autostarted by the session: its own unit, even with INVOCATION_ID inherited.
        assert!(!in_cardea_unit(
            "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-cardea\\x2dtray@autostart.service\n"
        ));
        assert!(!in_cardea_unit(
            "0::/user.slice/user-1000.slice/session-2.scope\n"
        ));
    }
}
