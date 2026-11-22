//! What the tray shows, from what the daemon reports: the icon state, the tooltip, the menu, the notifications.
//! Pure, so it is tested without a session bus.

use std::collections::HashMap;

use cardea_core::model::{Config, Toolkit};
use cardea_core::services::{Instance, InstanceState};
use cardea_core::state::{self, Action, State};
use cardea_proto::{Event, Summary};

/// What the tray knows.
#[derive(Clone, Debug, Default)]
pub struct World {
    pub connected: bool,
    pub sandboxes: Vec<Summary>,
    pub configs: HashMap<String, Config>,
    pub services: HashMap<String, Vec<Instance>>,
}

/// The icon: one per state, each with its badge (`assets/logo/tray`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    Idle,
    Running,
    Busy,
    Error,
}

pub fn look(w: &World) -> Look {
    let s = &w.sandboxes;
    if !w.connected
        || s.iter().any(|s| {
            matches!(s.state, State::Error | State::BuildFailed)
                || s.warnings.iter().any(|w| w.starts_with("service "))
        })
    {
        Look::Error
    } else if s.iter().any(|s| s.task.is_some()) {
        Look::Busy
    } else if s.iter().any(|s| s.state == State::Running) {
        Look::Running
    } else {
        Look::Idle
    }
}

fn state_text(s: &Summary) -> String {
    match (&s.task, s.state) {
        (Some(t), _) if t.percent > 0 => format!("{} · {} %", t.label, t.percent),
        (Some(t), _) => t.label.clone(),
        (None, State::Running) => "● running".into(),
        (None, State::Stopped) => "○ stopped".into(),
        (None, State::Error) => "✕ error".into(),
        (None, State::BuildFailed) => "✕ build failed".into(),
        (None, other) => format!("{other:?}").to_lowercase(),
    }
}

/// Title and text of the tooltip: what runs.
pub fn tooltip(w: &World) -> (String, String) {
    if !w.connected {
        return ("Cardea".into(), "The Cardea daemon is not running".into());
    }
    let busy: Vec<String> = w
        .sandboxes
        .iter()
        .filter(|s| s.state != State::Stopped || s.task.is_some())
        .map(|s| format!("{}: {}", s.name, state_text(s)))
        .collect();
    let text = if busy.is_empty() {
        "No sandbox running".into()
    } else {
        busy.join("\n")
    };
    ("Cardea".into(), text)
}

/// What a menu entry or a notification button does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    Manager,
    Details(String),
    Launch(String, String),
    /// A `cardea` command in a host terminal.
    Terminal(Vec<String>),
    Start(String),
    Stop(String),
    Service {
        id: String,
        service: String,
        folder: Option<String>,
        start: bool,
    },
    Quit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    /// `None`: a disabled line.
    Do(String, Option<Act>),
    Sub(String, Vec<Item>),
    Separator,
}

/// The menu: one submenu per sandbox, or a flat list when there is only one; then the manager and Quit.
pub fn menu(w: &World) -> Vec<Item> {
    let mut items = Vec::new();
    if !w.connected {
        items.push(Item::Do("The Cardea daemon is not running".into(), None));
    } else if w.sandboxes.is_empty() {
        items.push(Item::Do("No sandbox yet".into(), None));
    } else if let [s] = &w.sandboxes[..] {
        items.push(Item::Do(format!("{} — {}", s.name, state_text(s)), None));
        items.extend(sandbox_items(w, s));
    } else {
        for s in &w.sandboxes {
            items.push(Item::Sub(
                format!("{} — {}", s.name, state_text(s)),
                sandbox_items(w, s),
            ));
        }
    }
    items.push(Item::Separator);
    items.push(Item::Do("Open manager".into(), Some(Act::Manager)));
    items.push(Item::Do("Quit tray".into(), Some(Act::Quit)));
    items
}

fn sandbox_items(w: &World, s: &Summary) -> Vec<Item> {
    let id = s.id.clone();
    let running = s.state == State::Running;
    let mut items: Vec<Item> = s
        .warnings
        .iter()
        .map(|w| Item::Do(format!("! {w}"), None))
        .collect();
    if let Some(c) = w.configs.get(&id) {
        for a in c.apps.iter().filter(|a| a.pinned) {
            let act = if a.toolkit == Toolkit::Terminal {
                Act::Terminal(vec!["app".into(), id.clone(), a.id.clone()])
            } else {
                Act::Launch(id.clone(), a.id.clone())
            };
            items.push(Item::Do(a.name.clone(), s.task.is_none().then_some(act)));
        }
    }
    for i in w
        .services
        .get(&id)
        .filter(|_| running)
        .into_iter()
        .flatten()
    {
        let name = w
            .configs
            .get(&id)
            .and_then(|c| c.services.iter().find(|x| x.id == i.service))
            .map_or(i.service.clone(), |x| x.name.clone());
        let label = match &i.folder {
            Some(f) => format!("{name} · {f}"),
            None => name,
        };
        let (state, alive) = match i.state {
            InstanceState::Running => ("running".to_owned(), true),
            InstanceState::Exited(code) => (format!("exited ({code})"), true),
            InstanceState::Stopped => ("stopped".to_owned(), false),
        };
        let svc = |start| Act::Service {
            id: id.clone(),
            service: i.service.clone(),
            folder: i.folder.clone(),
            start,
        };
        let mut attach = vec![
            "service".into(),
            id.clone(),
            "attach".into(),
            i.service.clone(),
        ];
        attach.extend(i.folder.clone());
        items.push(Item::Sub(
            format!("{label} — {state}"),
            vec![
                Item::Do(
                    if alive { "Restart" } else { "Start" }.into(),
                    Some(svc(true)),
                ),
                Item::Do("Stop".into(), alive.then(|| svc(false))),
                Item::Do("Attach".into(), alive.then_some(Act::Terminal(attach))),
            ],
        ));
    }
    if !items.is_empty() {
        items.push(Item::Separator);
    }
    items.push(Item::Do(
        "Open details".into(),
        Some(Act::Details(id.clone())),
    ));
    items.push(Item::Do(
        "Open a terminal".into(),
        running.then(|| Act::Terminal(vec!["exec".into(), id.clone()])),
    ));
    let allowed = |a| s.task.is_none() && state::check(s.state, a).is_ok();
    if running {
        items.push(Item::Do(
            "Stop".into(),
            allowed(Action::Stop).then(|| Act::Stop(id.clone())),
        ));
    } else {
        items.push(Item::Do(
            "Start".into(),
            allowed(Action::Start).then(|| Act::Start(id.clone())),
        ));
    }
    items
}

/// A desktop notification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub title: String,
    pub body: String,
    pub actions: Vec<(String, Act)>,
}

/// The notifications an event raises, judged against the state before it.
pub fn notes(before: &World, event: &Event) -> Vec<Note> {
    let name = |id: &str| {
        before
            .sandboxes
            .iter()
            .find(|s| s.id == id)
            .map_or(id.to_owned(), |s| s.name.clone())
    };
    match event {
        Event::TaskEnded {
            id,
            label,
            error: Some(e),
        } => vec![Note {
            title: format!("{}: {label} failed", name(id)),
            body: e.clone(),
            actions: vec![("Details".into(), Act::Details(id.clone()))],
        }],
        Event::Changed { sandbox } => {
            let was = before.sandboxes.iter().find(|s| s.id == sandbox.id);
            // An error with no task running: the sandbox or its firewall stopped on its own.
            let unexpected = sandbox.state == State::Error
                && sandbox.task.is_none()
                && was.is_some_and(|w| w.state == State::Running && w.task.is_none());
            if unexpected {
                vec![Note {
                    title: format!("{} stopped", sandbox.name),
                    body: sandbox.error.clone().unwrap_or_default(),
                    actions: vec![
                        ("Restart".into(), Act::Start(sandbox.id.clone())),
                        ("Details".into(), Act::Details(sandbox.id.clone())),
                    ],
                }]
            } else {
                Vec::new()
            }
        }
        Event::Services { id, services } => {
            let old = before.services.get(id);
            services
                .iter()
                .filter_map(|i| {
                    let InstanceState::Exited(code) = i.state else {
                        return None;
                    };
                    let was_running = old.into_iter().flatten().any(|o| {
                        o.service == i.service
                            && o.folder == i.folder
                            && o.state == InstanceState::Running
                    });
                    was_running.then(|| {
                        let mut attach = vec![
                            "service".into(),
                            id.clone(),
                            "attach".into(),
                            i.service.clone(),
                        ];
                        attach.extend(i.folder.clone());
                        let label = i
                            .folder
                            .as_ref()
                            .map_or(i.service.clone(), |f| format!("{} · {f}", i.service));
                        Note {
                            title: format!("{}: {label} stopped", name(id)),
                            body: format!(
                                "The service exited with code {code}; its last output is kept."
                            ),
                            actions: vec![
                                ("Attach".into(), Act::Terminal(attach)),
                                (
                                    "Restart".into(),
                                    Act::Service {
                                        id: id.clone(),
                                        service: i.service.clone(),
                                        folder: i.folder.clone(),
                                        start: true,
                                    },
                                ),
                            ],
                        }
                    })
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

/// The notification when the keyring asks for its password: the manager has the password dialog.
pub fn password_note(w: &World, id: &str) -> Note {
    let name = w
        .sandboxes
        .iter()
        .find(|s| s.id == id)
        .map_or(id, |s| s.name.as_str());
    Note {
        title: format!("{name}: keyring password needed"),
        body: "Start it from the manager to type its keyring password.".into(),
        actions: vec![("Open".into(), Act::Details(id.to_owned()))],
    }
}

/// Applies an event; returns the sandboxes whose configuration should be read again.
pub fn apply(w: &mut World, event: Event) -> Option<String> {
    match event {
        Event::Added { sandbox } | Event::Changed { sandbox } => {
            let id = sandbox.id.clone();
            if sandbox.state != State::Running {
                w.services.remove(&id);
            }
            match w.sandboxes.iter_mut().find(|s| s.id == id) {
                Some(s) => *s = sandbox,
                None => w.sandboxes.push(sandbox),
            }
            Some(id)
        }
        Event::Removed { id } => {
            w.sandboxes.retain(|s| s.id != id);
            w.configs.remove(&id);
            w.services.remove(&id);
            None
        }
        Event::Task { id, task } => {
            if let Some(s) = w.sandboxes.iter_mut().find(|s| s.id == id) {
                s.task = task;
            }
            None
        }
        Event::Services { id, services } => {
            w.services.insert(id, services);
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cardea_core::builtin;
    use cardea_core::state::Flags;
    use cardea_proto::Task;

    fn summary(id: &str, state: State) -> Summary {
        Summary {
            id: id.into(),
            name: id.to_uppercase(),
            icon: None,
            state,
            flags: Flags::default(),
            error: None,
            task: None,
            warnings: Vec::new(),
            folder: String::new(),
        }
    }

    fn world(list: Vec<Summary>) -> World {
        World {
            connected: true,
            sandboxes: list,
            ..World::default()
        }
    }

    #[test]
    fn icon_states() {
        assert_eq!(look(&World::default()), Look::Error);
        assert_eq!(look(&world(vec![summary("a", State::Stopped)])), Look::Idle);
        assert_eq!(
            look(&world(vec![
                summary("a", State::Stopped),
                summary("b", State::Running)
            ])),
            Look::Running
        );
        let mut busy = summary("c", State::Building);
        busy.task = Some(Task {
            label: "build".into(),
            step: 1,
            steps: 3,
            step_label: "image".into(),
            percent: 40,
        });
        assert_eq!(
            look(&world(vec![summary("b", State::Running), busy])),
            Look::Busy
        );
        let mut crashed = summary("d", State::Running);
        crashed
            .warnings
            .push("service svc-x exited (code 3)".into());
        assert_eq!(look(&world(vec![crashed])), Look::Error);
        let (_, text) = tooltip(&world(vec![
            summary("a", State::Stopped),
            summary("b", State::Running),
        ]));
        assert_eq!(text, "B: ● running");
    }

    #[test]
    fn single_sandbox_menu_is_flat_and_multi_has_submenus() {
        let mut w = world(vec![summary("claude", State::Running)]);
        let mut c = builtin::example_claude();
        c.id = "claude".into();
        w.configs.insert("claude".into(), c);
        let m = menu(&w);
        assert_eq!(m[0], Item::Do("CLAUDE — ● running".into(), None));
        assert!(m.contains(&Item::Do(
            "Claude".into(),
            Some(Act::Launch("claude".into(), "claude-desktop".into()))
        )));
        let code = Act::Terminal(vec!["app".into(), "claude".into(), "claude-code".into()]);
        assert!(m.contains(&Item::Do("Claude Code".into(), Some(code))));
        assert!(m.contains(&Item::Do("Stop".into(), Some(Act::Stop("claude".into())))));
        assert_eq!(
            m.last(),
            Some(&Item::Do("Quit tray".into(), Some(Act::Quit)))
        );
        w.sandboxes.push(summary("lab", State::Stopped));
        let m = menu(&w);
        assert!(matches!(&m[0], Item::Sub(l, _) if l == "CLAUDE — ● running"));
        let Item::Sub(_, lab) = &m[1] else {
            panic!("{m:?}")
        };
        assert!(lab.contains(&Item::Do("Start".into(), Some(Act::Start("lab".into())))));
        assert!(lab.contains(&Item::Do("Open a terminal".into(), None)));
    }

    #[test]
    fn warnings_and_services_in_the_menu() {
        let mut s = summary("claude", State::Running);
        s.warnings.push("firewall off: …".into());
        let mut w = world(vec![s]);
        w.services.insert(
            "claude".into(),
            vec![Instance {
                service: "rc".into(),
                folder: Some("site".into()),
                state: InstanceState::Exited(1),
                autostart: false,
            }],
        );
        let m = menu(&w);
        assert_eq!(m[1], Item::Do("! firewall off: …".into(), None));
        let Item::Sub(label, sub) = &m[2] else {
            panic!("{m:?}")
        };
        assert_eq!(label, "rc · site — exited (1)");
        assert_eq!(
            sub[0],
            Item::Do(
                "Restart".into(),
                Some(Act::Service {
                    id: "claude".into(),
                    service: "rc".into(),
                    folder: Some("site".into()),
                    start: true
                })
            )
        );
    }

    #[test]
    fn notifications() {
        let mut w = world(vec![summary("claude", State::Running)]);
        let running = Instance {
            service: "rc".into(),
            folder: None,
            state: InstanceState::Running,
            autostart: true,
        };
        w.services.insert("claude".into(), vec![running.clone()]);
        let exited = Event::Services {
            id: "claude".into(),
            services: vec![Instance {
                state: InstanceState::Exited(3),
                ..running.clone()
            }],
        };
        let n = notes(&w, &exited);
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].title, "CLAUDE: rc stopped");
        assert_eq!(
            n[0].actions[0].1,
            Act::Terminal(vec![
                "service".into(),
                "claude".into(),
                "attach".into(),
                "rc".into()
            ])
        );
        // Already exited before: no second notification.
        apply(&mut w, exited.clone());
        assert!(notes(&w, &exited).is_empty());

        let mut err = summary("claude", State::Error);
        err.error = Some("the sandbox stopped unexpectedly".into());
        let w = world(vec![summary("claude", State::Running)]);
        assert_eq!(
            notes(
                &w,
                &Event::Changed {
                    sandbox: err.clone()
                }
            )[0]
            .title,
            "CLAUDE stopped"
        );
        // An error at the end of a start is the task's own: TaskEnded says it.
        let mut starting = summary("claude", State::Starting);
        starting.task = Some(Task {
            label: "start".into(),
            step: 0,
            steps: 0,
            step_label: String::new(),
            percent: 0,
        });
        assert!(notes(&world(vec![starting]), &Event::Changed { sandbox: err }).is_empty());
        let failed = Event::TaskEnded {
            id: "claude".into(),
            label: "start".into(),
            error: Some("no".into()),
        };
        assert_eq!(notes(&w, &failed)[0].title, "CLAUDE: start failed");
    }
}
