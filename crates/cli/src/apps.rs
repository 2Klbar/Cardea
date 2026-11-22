//! `cardea app` and `cardea service`.

use std::io::IsTerminal;

use cardea_core::apps::Detected;
use cardea_core::model::{Autostart, Config, Service, Toolkit};
use cardea_core::services::{self, Instance, InstanceState};
use cardea_core::state::State;
use cardea_proto::{ErrorCode, Method, Saved, Status};
use clap::{Subcommand, ValueEnum};

use crate::client::Client;

pub fn status(c: &mut Client, id: &str) -> Result<Status, String> {
    crate::call(c, Method::Status { id: id.to_owned() })
}

/// Saves a changed configuration through the daemon, which checks it.
pub fn save(c: &mut Client, id: &str, config: &Config) -> Result<(), String> {
    let text = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    let saved: Saved = crate::call(
        c,
        Method::SetConfig {
            id: id.to_owned(),
            config: text,
        },
    )?;
    out!("{}", crate::saved_text(saved));
    Ok(())
}

/// The keyring password, read from the terminal without echo.
pub fn password(id: &str) -> Result<String, String> {
    use rustix::termios::{LocalModes, OptionalActions, tcgetattr, tcsetattr};
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        return Err(format!(
            "the keyring of {id} asks for its password: start it from a terminal or the manager"
        ));
    }
    eprint!("Keyring password of {id}: ");
    let old = tcgetattr(&stdin).map_err(|e| e.to_string())?;
    let mut quiet = old.clone();
    quiet.local_modes.remove(LocalModes::ECHO);
    tcsetattr(&stdin, OptionalActions::Now, &quiet).map_err(|e| e.to_string())?;
    let mut line = String::new();
    let read = stdin.read_line(&mut line);
    let _ = tcsetattr(&stdin, OptionalActions::Now, &old);
    eprintln!();
    read.map_err(|e| e.to_string())?;
    Ok(line.trim_end_matches(['\n', '\r']).to_owned())
}

/// Sends a call that may need the keyring password, asking for it when the daemon says so.
pub fn with_password(
    c: &mut Client,
    id: &str,
    make: impl Fn(Option<String>) -> Method,
) -> Result<(), String> {
    match c.call::<()>(make(None)) {
        Err(e) if e.code == ErrorCode::PasswordRequired => c
            .call::<()>(make(Some(password(id)?)))
            .map_err(|e| e.message),
        r => r.map_err(|e| e.message),
    }
}

/// `cardea app <id>`: the apps; `cardea app <id> <app>`: launch one.
pub fn app(
    id: String,
    app: Option<String>,
    test_wayland: bool,
    url: Option<String>,
    args: Vec<String>,
) -> Result<(), String> {
    let (_, mut c) = crate::connect()?;
    let st = status(&mut c, &id)?;
    let Some(app_id) = app else {
        if st.config.apps.is_empty() {
            out!("No app yet. The apps installed in the sandbox: `cardea app {id} --found`.");
        }
        for a in &st.config.apps {
            let mut marks = Vec::new();
            if a.pinned {
                marks.push("pinned");
            }
            if a.desktop_entry {
                marks.push("menu");
            }
            out!(
                "{:<16} {:<24} {:<9} {}",
                a.id,
                a.name,
                format!("{:?}", a.toolkit).to_lowercase(),
                marks.join(", ")
            );
        }
        return Ok(());
    };
    let a = st
        .config
        .apps
        .iter()
        .find(|a| a.id == app_id)
        .ok_or_else(|| format!("{id} has no app {app_id:?} (see `cardea app {id}`)"))?
        .clone();
    if a.toolkit != Toolkit::Terminal && !test_wayland {
        return with_password(&mut c, &id, |password| Method::Launch {
            id: id.clone(),
            app: app_id.clone(),
            args: args.clone(),
            url: url.clone(),
            password,
        });
    }
    if url.is_some() {
        return Err(format!("{} opens no links", a.name));
    }
    let tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    if !tty {
        // From a menu entry: a terminal of the desktop runs this same command.
        let mut argv = vec![
            cardea_daemon::terminal::cardea().display().to_string(),
            "app".into(),
            id,
            app_id,
        ];
        if test_wayland {
            argv.push("--test-wayland".into());
        }
        argv.push("--".into());
        argv.extend(args);
        return cardea_daemon::terminal::open(crate::terminal_setting(&mut c).as_deref(), &argv);
    }
    if st.summary.state != State::Running {
        crate::start_and_wait(&id)?;
    }
    let code = crate::exec::run(
        id.clone(),
        false,
        None,
        args,
        Some(app_id.clone()),
        test_wayland,
    )?;
    if test_wayland {
        let toolkits = [
            ("qt", Toolkit::Qt),
            ("gtk", Toolkit::Gtk),
            ("electron", Toolkit::Electron),
            ("other", Toolkit::Other),
        ];
        if crate::confirm(&format!("Did a window of {} appear?", a.name), false)? {
            out!(
                "Which toolkit is it made with? qt, gtk, electron or other (other keeps the Wayland variables in its env):"
            );
            let mut line = String::new();
            std::io::stdin()
                .read_line(&mut line)
                .map_err(|e| e.to_string())?;
            let Some((_, toolkit)) = toolkits.iter().find(|(n, _)| *n == line.trim()) else {
                return Err("no toolkit saved".into());
            };
            let (_, mut c) = crate::connect()?;
            let mut config = status(&mut c, &id)?.config;
            let saved = config
                .apps
                .iter_mut()
                .find(|x| x.id == app_id)
                .ok_or("the app is gone")?;
            saved.toolkit = *toolkit;
            saved
                .wants
                .retain(|w| *w != cardea_core::model::Capability::X11);
            if *toolkit == Toolkit::Other {
                for v in cardea_core::apps::test_wayland(saved, &[])
                    .env
                    .iter()
                    .filter(|v| !v.starts_with("DISPLAY="))
                {
                    let (k, val) = v.split_once('=').expect("NAME=value");
                    saved.env.insert(k.into(), val.into());
                }
            }
            save(&mut c, &id, &config)?;
        }
    }
    std::process::exit(code)
}

/// `cardea app <id> --found`: the apps of the sandbox's `.desktop` files, and adding one.
pub fn found(id: String, add: Option<String>) -> Result<(), String> {
    let (_, mut c) = crate::connect()?;
    let list: Vec<Detected> = crate::call(&mut c, Method::DetectApps { id: id.clone() })?;
    let Some(add) = add else {
        for d in &list {
            out!("{:<16} {:<24} {}", d.id, d.name, d.exec.join(" "));
        }
        return Ok(());
    };
    let d = list
        .iter()
        .find(|d| d.id == add)
        .ok_or_else(|| format!("no app {add:?} found in {id}"))?;
    let mut config = status(&mut c, &id)?.config;
    if config.apps.iter().any(|a| a.id == d.id) {
        return Err(format!("{id} already has an app {:?}", d.id));
    }
    config.apps.push(d.app());
    save(&mut c, &id, &config)
}

#[derive(Clone, Copy, ValueEnum)]
pub enum OnOff {
    On,
    Off,
}

#[derive(Subcommand)]
pub enum ServiceCommand {
    /// The services and their instances, with their state
    List,
    /// Declare a service: a command kept running in the background
    Add {
        service: String,
        /// Display name (default: the id)
        #[arg(long)]
        name: Option<String>,
        /// One instance per shared folder, running in it; `{folder}` in the command becomes its name
        #[arg(long)]
        per_folder: bool,
        /// The command and its arguments, after `--`
        #[arg(last = true, required = true)]
        exec: Vec<String>,
    },
    /// Remove a service from the configuration
    Remove { service: String },
    /// Start an instance (again)
    Start {
        service: String,
        folder: Option<String>,
    },
    /// Stop an instance
    Stop {
        service: String,
        folder: Option<String>,
    },
    /// Open an instance in this terminal; detach with Ctrl+B then D
    Attach {
        service: String,
        folder: Option<String>,
    },
    /// Start an instance with the sandbox, or not
    Autostart {
        service: String,
        state: OnOff,
        folder: Option<String>,
    },
}

pub fn service(id: String, command: ServiceCommand) -> Result<(), String> {
    let (_, mut c) = crate::connect()?;
    let st = status(&mut c, &id)?;
    let mut config = st.config;
    let running = st.summary.state == State::Running;
    match command {
        ServiceCommand::List => {
            let list: Vec<Instance> = if running {
                crate::call(&mut c, Method::Services { id: id.clone() })?
            } else {
                services::instances(&config, "")
            };
            if list.is_empty() {
                out!("No service instance.");
            }
            for i in list {
                let state = match i.state {
                    InstanceState::Running => "running".to_owned(),
                    InstanceState::Exited(code) => format!("EXITED (code {code})"),
                    InstanceState::Stopped => "stopped".to_owned(),
                };
                let folder = i.folder.map(|f| format!(" [{f}]")).unwrap_or_default();
                let auto = if i.autostart { "  autostart" } else { "" };
                out!("{}{folder}  {state}{auto}", i.service);
            }
            Ok(())
        }
        ServiceCommand::Add {
            service,
            name,
            per_folder,
            exec,
        } => {
            config.services.push(Service {
                name: name.unwrap_or_else(|| service.clone()),
                id: service,
                exec,
                env: Default::default(),
                per_folder,
                autostart: None,
            });
            save(&mut c, &id, &config)
        }
        ServiceCommand::Remove { service } => {
            let before = config.services.len();
            config.services.retain(|s| s.id != service);
            if config.services.len() == before {
                return Err(format!("{id} has no service {service:?}"));
            }
            save(&mut c, &id, &config)
        }
        ServiceCommand::Start { .. } | ServiceCommand::Stop { .. } if !running => {
            Err(format!("{id} is not running"))
        }
        ServiceCommand::Start { service, folder } => crate::call::<()>(
            &mut c,
            Method::Service {
                id,
                service,
                folder,
                start: true,
            },
        ),
        ServiceCommand::Stop { service, folder } => crate::call::<()>(
            &mut c,
            Method::Service {
                id,
                service,
                folder,
                start: false,
            },
        ),
        ServiceCommand::Attach { service, folder } => {
            if !std::io::stdin().is_terminal() {
                let mut argv = vec![
                    cardea_daemon::terminal::cardea().display().to_string(),
                    "service".into(),
                    id,
                    "attach".into(),
                    service,
                ];
                argv.extend(folder);
                return cardea_daemon::terminal::open(
                    crate::terminal_setting(&mut c).as_deref(),
                    &argv,
                );
            }
            let cmd = services::attach_command(&service, folder.as_deref());
            let code = crate::exec::run(id, false, None, cmd, None, false)?;
            std::process::exit(code)
        }
        ServiceCommand::Autostart {
            service,
            state,
            folder,
        } => {
            let s = config
                .services
                .iter_mut()
                .find(|s| s.id == service)
                .ok_or_else(|| format!("{id} has no service {service:?}"))?;
            let on = matches!(state, OnOff::On);
            s.autostart = match (s.per_folder, folder) {
                (false, None) => Some(Autostart::Single(on)),
                (true, Some(f)) => {
                    let mut names = match s.autostart.take() {
                        Some(Autostart::Folders(n)) => n,
                        _ => Vec::new(),
                    };
                    names.retain(|n| *n != f);
                    if on {
                        names.push(f);
                    }
                    Some(Autostart::Folders(names))
                }
                (true, None) => return Err(format!("{service} runs per folder: name the folder")),
                (false, Some(_)) => {
                    return Err(format!("{service} has a single instance: name no folder"));
                }
            };
            save(&mut c, &id, &config)
        }
    }
}
