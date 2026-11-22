//! Background services: each instance runs in a tmux session of the app container (socket `-L cardea`), with
//! `remain-on-exit` so its last output stays when it ends. Commands are argument arrays: no shell (R9).

use serde::{Deserialize, Serialize};

use crate::model::{Autostart, Config, Service};

/// The tmux socket name used for services.
pub const SOCKET: &str = "cardea";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instance {
    pub service: String,
    /// The shared folder's name, for a per-folder service.
    #[serde(default)]
    pub folder: Option<String>,
    pub state: InstanceState,
    /// Started when the sandbox starts.
    #[serde(default)]
    pub autostart: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "code")]
pub enum InstanceState {
    Running,
    /// The command ended; its pane stays with its last output.
    Exited(i32),
    Stopped,
}

/// The tmux session of an instance; tmux turns `.` and `:` into `_`, so only safe characters are used.
pub fn session(service: &str, folder: Option<&str>) -> String {
    let safe = |s: &str| {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>()
    };
    match folder {
        Some(f) => format!("svc-{}-{}", safe(service), safe(f)),
        None => format!("svc-{}", safe(service)),
    }
}

/// Every instance the configuration declares: (service, folder), per-folder services once per shared folder.
pub fn declared(c: &Config) -> Vec<(&Service, Option<&str>)> {
    let mut out = Vec::new();
    for s in &c.services {
        if s.per_folder {
            out.extend(c.folders.iter().map(|f| (s, Some(f.name()))));
        } else {
            out.push((s, None));
        }
    }
    out
}

pub fn autostarts(s: &Service, folder: Option<&str>) -> bool {
    match (&s.autostart, folder) {
        (Some(Autostart::Single(on)), None) => *on,
        (Some(Autostart::Folders(names)), Some(f)) => names.iter().any(|n| n == f),
        _ => false,
    }
}

/// The tmux command that starts an instance in the background.
pub fn start_command(c: &Config, s: &Service, folder: Option<&str>) -> Vec<String> {
    let name = session(&s.id, folder);
    let dir = match folder {
        Some(f) => c
            .folders
            .iter()
            .find(|x| x.name() == f)
            .map_or(c.identity.home_mount.clone(), |x| x.mount.clone()),
        None => c.identity.home_mount.clone(),
    };
    let mut cmd: Vec<String> = [
        "tmux",
        "-L",
        SOCKET,
        "new-session",
        "-d",
        "-s",
        &name,
        "-c",
        &dir,
    ]
    .map(String::from)
    .to_vec();
    for (k, v) in &s.env {
        cmd.push("-e".into());
        cmd.push(format!("{k}={v}"));
    }
    cmd.extend(s.exec.iter().map(|a| match folder {
        Some(f) if a == "{folder}" => f.to_owned(),
        _ => a.clone(),
    }));
    // In the same tmux command list, before the pane can end.
    cmd.extend([";", "set-option", "-t", &name, "remain-on-exit", "on"].map(String::from));
    cmd
}

pub fn stop_command(service: &str, folder: Option<&str>) -> Vec<String> {
    [
        "tmux",
        "-L",
        SOCKET,
        "kill-session",
        "-t",
        &session(service, folder),
    ]
    .map(String::from)
    .to_vec()
}

pub fn attach_command(service: &str, folder: Option<&str>) -> Vec<String> {
    [
        "tmux",
        "-L",
        SOCKET,
        "attach-session",
        "-t",
        &session(service, folder),
    ]
    .map(String::from)
    .to_vec()
}

/// Lists the panes: `session dead status` per line.
pub const LIST_COMMAND: [&str; 7] = [
    "tmux",
    "-L",
    SOCKET,
    "list-panes",
    "-a",
    "-F",
    "#{session_name} #{pane_dead} #{pane_dead_status}",
];

/// The instances with their state, from the output of [`LIST_COMMAND`].
pub fn instances(c: &Config, panes: &str) -> Vec<Instance> {
    declared(c)
        .into_iter()
        .map(|(s, folder)| {
            let name = session(&s.id, folder);
            let state = panes
                .lines()
                .filter_map(|l| {
                    let mut f = l.split_whitespace();
                    (f.next()? == name).then(|| {
                        (
                            f.next().unwrap_or("0"),
                            f.next().and_then(|c| c.parse().ok()),
                        )
                    })
                })
                .next()
                .map_or(InstanceState::Stopped, |(dead, code)| {
                    if dead == "1" {
                        InstanceState::Exited(code.unwrap_or(-1))
                    } else {
                        InstanceState::Running
                    }
                });
            Instance {
                service: s.id.clone(),
                folder: folder.map(str::to_owned),
                state,
                autostart: autostarts(s, folder),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin;
    use crate::model::{Folder, Mode};

    fn config() -> Config {
        let mut c = builtin::example_claude();
        c.folders = vec![
            Folder {
                host: "/p/site".into(),
                mount: "/work/site".into(),
                mode: Mode::Rw,
            },
            Folder {
                host: "/p/my.notes".into(),
                mount: "/work/my.notes".into(),
                mode: Mode::Ro,
            },
        ];
        c.services[0].autostart = Some(Autostart::Folders(vec!["site".into()]));
        c
    }

    #[test]
    fn per_folder_instances_and_commands() {
        let c = config();
        let d: Vec<_> = declared(&c)
            .into_iter()
            .map(|(s, f)| (s.id.as_str(), f))
            .collect();
        assert_eq!(
            d,
            [
                ("remote-control", Some("site")),
                ("remote-control", Some("my.notes"))
            ]
        );
        let s = &c.services[0];
        assert!(autostarts(s, Some("site")) && !autostarts(s, Some("my.notes")));
        assert_eq!(
            session("remote-control", Some("my.notes")),
            "svc-remote-control-my_notes"
        );
        let cmd = start_command(&c, s, Some("site"));
        assert_eq!(
            cmd,
            [
                "tmux",
                "-L",
                "cardea",
                "new-session",
                "-d",
                "-s",
                "svc-remote-control-site",
                "-c",
                "/work/site",
                "claude",
                "remote-control",
                "--name",
                "site",
                ";",
                "set-option",
                "-t",
                "svc-remote-control-site",
                "remain-on-exit",
                "on"
            ]
        );
    }

    #[test]
    fn states_from_tmux() {
        let c = config();
        let got = instances(&c, "svc-remote-control-site 1 3\nother 0 \n");
        assert_eq!(got[0].state, InstanceState::Exited(3));
        assert!(got[0].autostart);
        assert_eq!(got[1].state, InstanceState::Stopped);
        assert_eq!(
            instances(&c, "svc-remote-control-my_notes 0 \n")[1].state,
            InstanceState::Running
        );
    }
}
