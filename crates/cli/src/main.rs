mod client;

/// `println!` that exits quietly when stdout is closed (`cardea logs -f | head`).
macro_rules! out {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    }};
}

mod apps;
mod doctor;
mod exec;
mod folders;
mod links;
mod net;
mod packages;

use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use cardea_core::load;
use cardea_core::model::{Clipboard, Config, KeyringMode, Kind, Mode, X11, Zone, ZoneMode};
use cardea_core::state::State;
use cardea_daemon::paths::Paths;
use cardea_proto::{ErrorCode, Event, Method, ProfileInfo, Saved, Status, Summary, Task};
use clap::{Parser, Subcommand, ValueEnum};

use crate::client::Client;

/// Create, run and manage isolated Podman sandboxes.
#[derive(Parser)]
#[command(name = "cardea", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List the sandboxes with their state and warnings
    List,
    /// Show a sandbox: state, capabilities, firewall, warnings
    Status { id: String },
    /// Create a sandbox from a profile, or from a configuration file
    #[command(group = clap::ArgGroup::new("from").required(true))]
    Create {
        /// Profile to create it from (see `cardea profile list`)
        #[arg(long, group = "from", requires = "name")]
        profile: Option<String>,
        /// Display name; the id is made from it
        #[arg(long)]
        name: Option<String>,
        /// A complete sandbox.json to create it from
        #[arg(long, group = "from", value_name = "FILE")]
        from_config: Option<PathBuf>,
        /// Folder that holds the new sandbox's folder (default: the location in the settings, ~/Sandboxes)
        #[arg(long, value_name = "DIR")]
        location: Option<String>,
    },
    /// Start a sandbox
    Start { id: String },
    /// Stop a sandbox (from Error: reset it to Stopped)
    Stop { id: String },
    /// Stop then start a sandbox
    Restart { id: String },
    /// Rebuild a sandbox's image (restarts it if it was running)
    Rebuild {
        id: String,
        /// Pull the base image again and use no cached step: updates the image's packages
        #[arg(long)]
        fresh: bool,
    },
    /// Packages of a sandbox, from its sources
    Packages {
        id: String,
        #[command(subcommand)]
        command: packages::PackagesCommand,
    },
    /// Package sources: the built-in ones and custom ones
    Sources {
        id: String,
        #[command(subcommand)]
        command: packages::SourcesCommand,
    },
    /// Third-party apt repositories (key fingerprint required)
    Repo {
        id: String,
        #[command(subcommand)]
        command: packages::RepoCommand,
    },
    /// Delete a sandbox and its folder
    Delete {
        id: String,
        /// Keep the sandbox folder (home and configuration) on disk
        #[arg(long)]
        keep_home: bool,
        /// Do not ask for confirmation
        #[arg(short, long)]
        yes: bool,
    },
    /// Edit a sandbox's sandbox.json in $EDITOR; it is checked when you save
    Edit { id: String },
    /// Open a shell, or run a command, in a running sandbox
    Exec {
        id: String,
        /// Run as root inside the sandbox (with a few capabilities for package maintenance)
        #[arg(long)]
        root: bool,
        /// Run this app of the sandbox (a terminal app, in this terminal); the arguments after `--` are added
        #[arg(long, conflicts_with = "root")]
        app: Option<String>,
        /// Working directory inside the sandbox
        #[arg(short, long, value_name = "DIR")]
        workdir: Option<String>,
        /// The command and its arguments, after `--` (default: your login shell)
        #[arg(last = true)]
        cmd: Vec<String>,
    },
    /// A sandbox's apps, or launch one (starting the sandbox when needed)
    App {
        id: String,
        app: Option<String>,
        /// Run the app once without any X server, to see whether it works on Wayland
        #[arg(long, requires = "app")]
        test_wayland: bool,
        /// List the apps installed in the sandbox (from their .desktop files)
        #[arg(long, conflicts_with = "app")]
        found: bool,
        /// Add one of the apps found in the sandbox
        #[arg(long, value_name = "APP", conflicts_with = "app")]
        add: Option<String>,
        /// Open this link in the app (one of the schemes it declares); used by its menu entry
        #[arg(long, requires = "app", conflicts_with = "test_wayland")]
        url: Option<String>,
        /// Arguments for the app, after `--`
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Background services: commands kept running in the sandbox
    Service {
        id: String,
        #[command(subcommand)]
        command: apps::ServiceCommand,
    },
    /// Shared folders: host folders mounted into a sandbox
    Folders {
        id: String,
        #[command(subcommand)]
        command: FoldersCommand,
    },
    /// The sandbox firewall: zones, allow and block lists, blocked connections
    Net {
        id: String,
        #[command(subcommand)]
        command: Option<net::NetCommand>,
    },
    /// Links: the schemes the sandbox's apps open, and the links its apps may send to your browser
    Links {
        id: String,
        #[command(subcommand)]
        command: Option<links::LinksCommand>,
    },
    /// Show a sandbox's log
    Logs {
        id: String,
        /// Keep showing new lines
        #[arg(short, long)]
        follow: bool,
    },
    /// Profiles: reusable recipes for sandboxes
    Profile {
        #[command(subcommand)]
        command: ProfileCommand,
    },
    /// Check a hand-written configuration file and list every problem in it
    Check {
        /// What the file is
        kind: FileKind,
        file: PathBuf,
    },
    /// Check that this host can run sandboxes: session, tray, Podman
    Doctor,
    /// Open the manager window
    Gui,
    /// Start the tray icon at login, or not
    TrayAutostart { state: apps::OnOff },
    /// Run the daemon (started automatically by the other commands when needed)
    Daemon,
    /// Filter a sandbox's Wayland connections (started by the daemon)
    #[command(hide = true)]
    WaylandFilter {
        #[arg(long)]
        listen: PathBuf,
        #[arg(long)]
        upstream: PathBuf,
        /// No clipboard between apps
        #[arg(long)]
        clipboard_off: bool,
        /// The sandbox: its windows take the name of its menu entries
        #[arg(long)]
        sandbox: Option<String>,
    },
    /// Send a link to the daemon as if a running sandbox had opened it (tests: no relay, no FIFO)
    #[command(hide = true)]
    LinkSend { sandbox: String, url: String },
    /// Pass a sandbox's links to the daemon (started by the daemon)
    #[command(hide = true)]
    LinkRelay {
        #[arg(long)]
        fifo: PathBuf,
        #[arg(long)]
        sandbox: String,
    },
}

#[derive(Subcommand)]
enum FoldersCommand {
    /// List the shared folders, the missing ones and the protected paths
    List,
    /// Share a host folder (it applies at the next start)
    Add {
        /// The folder on this computer
        host: PathBuf,
        /// Where it appears in the sandbox (default: /work/<folder name>)
        #[arg(long)]
        mount: Option<String>,
        /// Read-only (the default in an Untrusted sandbox)
        #[arg(long, conflicts_with = "rw")]
        ro: bool,
        /// Read-write, with the protected paths kept read-only (the default in a Work sandbox)
        #[arg(long)]
        rw: bool,
    },
    /// Stop sharing a folder: give its mount point, name or host path
    Remove { folder: String },
}

#[derive(Subcommand)]
enum ProfileCommand {
    /// List the profiles
    List,
    /// Show a profile in full, with every default filled in
    Show { id: String },
    /// Import a profile file: shows what it asks for, then saves it among yours
    Import {
        file: PathBuf,
        /// Replace your profile with the same id
        #[arg(long)]
        replace: bool,
        /// Do not ask for confirmation
        #[arg(short, long)]
        yes: bool,
    },
    /// Write a profile to a file (or to the standard output with `-`)
    Export { id: String, file: PathBuf },
    /// Delete one of your profiles
    Delete { id: String },
    /// Save a sandbox's configuration as one of your profiles, without what is local to it
    Save {
        /// The sandbox
        id: String,
        /// The new profile's id
        profile: String,
        /// Its display name (default: the id)
        #[arg(long)]
        name: Option<String>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum FileKind {
    /// A sandbox configuration (sandbox.json)
    Sandbox,
    Profile,
    /// Tool settings (settings.json)
    Settings,
    /// A package source
    Source,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn connect() -> Result<(Paths, Client), String> {
    let paths = Paths::from_env()?;
    let client = Client::connect(&paths)?;
    Ok((paths, client))
}

/// Calls the daemon; errors become their message.
fn call<T: serde::de::DeserializeOwned>(c: &mut Client, method: Method) -> Result<T, String> {
    c.call(method).map_err(|e| e.message)
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Command::Daemon => cardea_daemon::main(),
        Command::WaylandFilter {
            listen,
            upstream,
            clipboard_off,
            sandbox,
        } => cardea_daemon::wayland::main(
            &listen,
            &upstream,
            if clipboard_off {
                cardea_core::model::Clipboard::Off
            } else {
                cardea_core::model::Clipboard::Shared
            },
            sandbox,
        ),
        Command::LinkRelay { fifo, sandbox } => links::relay(&fifo, &sandbox),
        Command::LinkSend { sandbox, url } => {
            let (_, mut c) = connect()?;
            let outcome: cardea_proto::LinkOutcome = call(&mut c, Method::Link { sandbox, url })?;
            out!(
                "{}",
                serde_json::to_string(&outcome)
                    .unwrap_or_default()
                    .trim_matches('"')
            );
            Ok(())
        }
        Command::Links { id, command } => {
            links::run(id, command.unwrap_or(links::LinksCommand::Show))
        }
        Command::Check { kind, file } => check(kind, &file),
        Command::Doctor => doctor::run(),
        Command::Gui => {
            let gui = cardea_daemon::terminal::cardea().with_file_name("cardea-gui");
            let program = if gui.is_file() {
                gui
            } else {
                "cardea-gui".into()
            };
            std::process::Command::new(program)
                .spawn()
                .map(drop)
                .map_err(|e| format!("cannot open the manager: {e}"))
        }
        Command::TrayAutostart { state } => {
            let (_, mut c) = connect()?;
            let mut settings: cardea_core::model::Settings = call(&mut c, Method::Settings {})?;
            settings.tray_at_login = matches!(state, apps::OnOff::On);
            call::<()>(&mut c, Method::SetSettings { settings })?;
            out!(
                "The tray {} at login.",
                if matches!(state, apps::OnOff::On) {
                    "starts"
                } else {
                    "no longer starts"
                }
            );
            Ok(())
        }
        Command::Exec {
            id,
            root,
            app,
            workdir,
            cmd,
        } => {
            let code = exec::run(id, root, workdir, cmd, app, false)?;
            std::process::exit(code)
        }
        Command::Net { id, command } => net::run(id, command.unwrap_or(net::NetCommand::Show)),
        Command::Folders { id, command } => match command {
            FoldersCommand::List => folders::list(id),
            FoldersCommand::Add {
                host,
                mount,
                ro,
                rw,
            } => {
                let mode = match (ro, rw) {
                    (true, _) => Some(Mode::Ro),
                    (_, true) => Some(Mode::Rw),
                    _ => None,
                };
                folders::add(id, &host, mount, mode)
            }
            FoldersCommand::Remove { folder } => folders::remove(id, &folder),
        },
        Command::List => {
            let (_, mut c) = connect()?;
            let list: Vec<Summary> = call(&mut c, Method::List {})?;
            if list.is_empty() {
                out!("No sandbox yet. Create one with: cardea create --profile empty --name Box");
                return Ok(());
            }
            let w_id = list.iter().map(|s| s.id.len()).max().unwrap_or(2).max(2);
            let w_name = list
                .iter()
                .map(|s| s.name.chars().count())
                .max()
                .unwrap_or(4)
                .max(4);
            out!("{:<w_id$}  {:<w_name$}  STATE", "ID", "NAME");
            for s in &list {
                let mut line = format!("{:<w_id$}  {:<w_name$}  {}", s.id, s.name, state_text(s));
                if !s.warnings.is_empty() {
                    let n = s.warnings.len();
                    line += &format!("  ⚠ {n} warning{}", if n > 1 { "s" } else { "" });
                }
                out!("{line}");
            }
            Ok(())
        }
        Command::Status { id } => {
            let (_, mut c) = connect()?;
            let status: Status = call(&mut c, Method::Status { id })?;
            print_status(&status);
            Ok(())
        }
        Command::Create {
            profile,
            name,
            from_config,
            location,
        } => {
            let (_, mut c) = connect()?;
            let config = match &from_config {
                Some(f) => Some(
                    std::fs::read_to_string(f)
                        .map_err(|e| format!("cannot read {}: {e}", f.display()))?,
                ),
                None => None,
            };
            call::<()>(&mut c, Method::Subscribe {})?;
            let s: Summary = call(
                &mut c,
                Method::Create {
                    profile,
                    name,
                    config,
                    location,
                },
            )?;
            out!("Created sandbox {:?} ({}) in {}", s.name, s.id, s.folder);
            follow(&mut c, &s.id)
        }
        Command::Start { id } => start_and_wait(&id),
        Command::Stop { id } => action(&id, |_| Method::Stop { id: id.clone() }),
        Command::Restart { id } => action(&id, |password| Method::Restart {
            id: id.clone(),
            password,
        }),
        Command::Rebuild { id, fresh } => rebuild(&id, fresh),
        Command::Packages { id, command } => packages::packages(id, command),
        Command::Sources { id, command } => packages::sources(id, command),
        Command::Repo { id, command } => packages::repo(id, command),
        Command::App {
            id,
            app,
            test_wayland,
            found,
            add,
            url,
            args,
        } => {
            if found || add.is_some() {
                apps::found(id, add)
            } else {
                apps::app(id, app, test_wayland, url, args)
            }
        }
        Command::Service { id, command } => apps::service(id, command),
        Command::Delete { id, keep_home, yes } => {
            let (_, mut c) = connect()?;
            let status: Status = call(&mut c, Method::Status { id: id.clone() })?;
            let s = &status.summary;
            let question = if keep_home {
                format!(
                    "Delete sandbox {:?} ({})? Its folder {} is kept.",
                    s.name, s.id, s.folder
                )
            } else {
                format!(
                    "Delete sandbox {:?} ({}) and its folder {}, with its home?",
                    s.name, s.id, s.folder
                )
            };
            if !yes && !confirm(&question, false)? {
                return Err("cancelled (without a terminal, confirm with --yes)".into());
            }
            call::<()>(&mut c, Method::Delete { id, keep_home })?;
            out!("Deleted.");
            Ok(())
        }
        Command::Edit { id } => edit(&id),
        Command::Logs { id, follow } => {
            let (_, mut c) = connect()?;
            let lines: Vec<String> = call(&mut c, Method::Logs { id, follow })?;
            for l in lines {
                out!("{l}");
            }
            if follow {
                loop {
                    if let Event::Log { line, .. } = c.event()? {
                        out!("{line}");
                    }
                }
            }
            Ok(())
        }
        Command::Profile {
            command: ProfileCommand::List,
        } => {
            let (_, mut c) = connect()?;
            let list: Vec<ProfileInfo> = call(&mut c, Method::Profiles {})?;
            let w = list.iter().map(|p| p.id.len()).max().unwrap_or(2);
            for p in list {
                let origin = if p.builtin { "built-in" } else { "yours" };
                out!(
                    "{:<w$}  {:<9} {}",
                    p.id,
                    origin,
                    p.description.unwrap_or(p.name)
                );
            }
            Ok(())
        }
        Command::Profile {
            command: ProfileCommand::Import { file, replace, yes },
        } => {
            let text = std::fs::read_to_string(&file)
                .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
            let p = load::config(&text, Kind::Profile)
                .map_err(|e| format!("{} is not a valid profile:\n{e}", file.display()))?;
            out!("Profile {:?} ({}) asks for:", p.name, p.id);
            for item in cardea_core::profile::review(&p) {
                let mark = if item.wide { "!" } else { " " };
                let mut lines = item.detail.lines();
                out!("{mark} {:<28} {}", item.title, lines.next().unwrap_or(""));
                for l in lines {
                    out!("  {:<28} {l}", "");
                }
            }
            if !yes
                && !confirm(
                    "Import it? The items marked ! widen what its sandboxes can reach or run.",
                    false,
                )?
            {
                return Err("cancelled (without a terminal, confirm with --yes)".into());
            }
            let (_, mut c) = connect()?;
            let info: ProfileInfo = call(
                &mut c,
                Method::ProfileImport {
                    config: text,
                    replace,
                },
            )?;
            out!("Imported profile {:?} ({}).", info.name, info.id);
            Ok(())
        }
        Command::Profile {
            command: ProfileCommand::Export { id, file },
        } => {
            let (_, mut c) = connect()?;
            let p: Config = call(&mut c, Method::Profile { id })?;
            let text = serde_json::to_string_pretty(&p).expect("serializable") + "\n";
            if file.as_os_str() == "-" {
                out!("{}", text.trim_end());
            } else {
                std::fs::write(&file, text)
                    .map_err(|e| format!("cannot write {}: {e}", file.display()))?;
                out!("Wrote {}", file.display());
            }
            Ok(())
        }
        Command::Profile {
            command: ProfileCommand::Delete { id },
        } => {
            let (_, mut c) = connect()?;
            call::<()>(&mut c, Method::ProfileDelete { id })?;
            out!("Deleted.");
            Ok(())
        }
        Command::Profile {
            command: ProfileCommand::Save { id, profile, name },
        } => {
            let (_, mut c) = connect()?;
            let name = name.unwrap_or_else(|| profile.clone());
            let info: ProfileInfo = call(&mut c, Method::ProfileSave { id, profile, name })?;
            out!("Saved profile {:?} ({}).", info.name, info.id);
            Ok(())
        }
        Command::Profile {
            command: ProfileCommand::Show { id },
        } => {
            let (_, mut c) = connect()?;
            let p: Config = call(&mut c, Method::Profile { id })?;
            out!(
                "{}",
                serde_json::to_string_pretty(&p).expect("serializable")
            );
            Ok(())
        }
    }
}

/// The `terminal` setting, when set.
fn terminal_setting(c: &mut Client) -> Option<Vec<String>> {
    call::<cardea_core::model::Settings>(c, Method::Settings {})
        .ok()?
        .terminal
}

fn saved_text(saved: Saved) -> &'static str {
    match saved {
        Saved::Applied => "Saved: the change applies now.",
        Saved::RestartRequired => "Saved: restart the sandbox to apply it (cardea restart).",
        Saved::RebuildRequired => "Saved: rebuild the sandbox to apply it (cardea rebuild).",
    }
}

fn check(kind: FileKind, file: &std::path::Path) -> Result<(), String> {
    let text = std::fs::read_to_string(file)
        .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    let (label, result) = match kind {
        FileKind::Sandbox => (
            "sandbox config",
            load::config(&text, Kind::Sandbox).map(drop),
        ),
        FileKind::Profile => ("profile", load::config(&text, Kind::Profile).map(drop)),
        FileKind::Settings => ("settings file", load::settings(&text).map(drop)),
        FileKind::Source => ("source", load::source(&text).map(drop)),
    };
    match result {
        Ok(()) => {
            out!("{}: valid {label}", file.display());
            Ok(())
        }
        Err(e) => Err(format!("{} is not a valid {label}:\n{e}", file.display())),
    }
}

/// Sends a lifecycle action and follows its task.
/// Sends a lifecycle action, with the keyring password when the daemon asks for it, and follows its task.
fn action(id: &str, make: impl Fn(Option<String>) -> Method) -> Result<(), String> {
    let (_, mut c) = connect()?;
    call::<()>(&mut c, Method::Subscribe {})?;
    apps::with_password(&mut c, id, make)?;
    follow(&mut c, id)
}

fn rebuild(id: &str, fresh: bool) -> Result<(), String> {
    action(id, |password| Method::Rebuild {
        id: id.to_owned(),
        fresh,
        password,
    })
}

fn start_and_wait(id: &str) -> Result<(), String> {
    action(id, |password| Method::Start {
        id: id.to_owned(),
        password,
    })
}

/// Shows the progress of `id`'s task until it ends.
fn follow(c: &mut Client, id: &str) -> Result<(), String> {
    let tty = std::io::stdout().is_terminal();
    let mut last = String::new();
    loop {
        match c.event()? {
            Event::Task {
                id: eid,
                task: Some(t),
            } if eid == id => {
                let line = format!("{id}: {}", task_text(&t));
                if tty {
                    print!("\r\x1b[K{line}");
                    let _ = std::io::stdout().flush();
                } else if t.step_label != last {
                    out!(
                        "{id}: {} · Step {}/{} · {}",
                        t.label,
                        t.step,
                        t.steps,
                        t.step_label
                    );
                    last = t.step_label.clone();
                }
            }
            Event::TaskEnded {
                id: eid,
                label,
                error,
            } if eid == id => {
                if tty {
                    print!("\r\x1b[K");
                }
                return match error {
                    None => {
                        out!("{id}: {label} finished");
                        Ok(())
                    }
                    Some(e) => Err(format!("{id}: {label} failed: {e}")),
                };
            }
            _ => {}
        }
    }
}

fn task_text(t: &Task) -> String {
    if t.steps == 0 {
        return format!("{}…", t.label);
    }
    format!(
        "{} · Step {}/{} · {} · {} %",
        t.label, t.step, t.steps, t.step_label, t.percent
    )
}

fn state_text(s: &Summary) -> String {
    let mut text = s.state.label().to_owned();
    if s.flags.restart_required {
        text += " · restart required";
    }
    if s.flags.rebuild_required {
        text += " · rebuild required";
    }
    if let Some(t) = s.task.as_ref().filter(|t| t.steps > 0) {
        text += &format!(
            " · Step {}/{} · {} · {} %",
            t.step, t.steps, t.step_label, t.percent
        );
    }
    if let (Some(e), State::Error | State::BuildFailed) = (&s.error, s.state) {
        text += &format!(": {e}");
    }
    text
}

fn zone_text(name: &str, z: &Zone) -> String {
    match z.mode {
        ZoneMode::Allow => format!("{name} allowlist ({})", z.allow.len()),
        ZoneMode::Block => format!("{name} blocklist ({})", z.block.len()),
    }
}

fn print_status(status: &Status) {
    let (s, c) = (&status.summary, &status.config);
    out!("{} ({}) — {}", s.name, s.id, state_text(s));
    let row = |k: &str, v: String| out!("  {k:<15}{v}");
    row("Folder", s.folder.clone());
    if let Some(p) = &c.profile {
        row("Profile", p.clone());
    }
    row("Trust", format!("{:?}", c.trust).to_lowercase());
    row("Base image", c.base_image.clone());
    row(
        "Home",
        if c.home.disposable {
            "disposable (fresh at each start)".into()
        } else {
            "persistent".into()
        },
    );
    for (i, f) in c.folders.iter().enumerate() {
        let mode = format!("{:?}", f.mode).to_lowercase();
        row(
            if i == 0 { "Shared folders" } else { "" },
            format!(
                "{} → {} ({mode}){}",
                f.host,
                f.mount,
                if status.missing_folders.contains(&f.host) {
                    ", missing: skipped at start"
                } else {
                    ""
                }
            ),
        );
    }
    let caps = &c.capabilities;
    let mut on = Vec::new();
    if caps.wayland {
        on.push(match caps.clipboard {
            Clipboard::Shared => "wayland (clipboard shared)".to_owned(),
            Clipboard::Off => "wayland (clipboard off)".to_owned(),
        });
    }
    match caps.x11 {
        X11::Off => {}
        X11::Isolated => on.push("x11 (isolated)".into()),
        X11::Host => on.push("x11 (host)".into()),
    }
    if caps.gpu {
        on.push("gpu".into());
    }
    if caps.kvm {
        on.push("kvm".into());
    }
    match caps.keyring.mode {
        KeyringMode::Off => {}
        KeyringMode::Auto => on.push("keyring (auto)".into()),
        KeyringMode::Ask => on.push("keyring (ask)".into()),
    }
    row(
        "Capabilities",
        if on.is_empty() {
            "none".into()
        } else {
            on.join(", ")
        },
    );
    let fw = &c.firewall;
    row(
        "Firewall",
        if fw.enabled {
            format!(
                "on · blocked traffic {} · {} · {} · {}",
                if fw.blocked == cardea_core::model::Blocked::Reject {
                    "rejected"
                } else {
                    "dropped"
                },
                zone_text("Host", &fw.host),
                zone_text("LAN", &fw.lan),
                zone_text("WAN", &fw.wan)
            )
        } else {
            "off".into()
        },
    );
    if !c.apps.is_empty() {
        row(
            "Apps",
            c.apps
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    if !c.services.is_empty() {
        let names: Vec<_> = c
            .services
            .iter()
            .map(|sv| {
                if sv.per_folder {
                    format!("{} (per folder)", sv.name)
                } else {
                    sv.name.clone()
                }
            })
            .collect();
        row("Services", names.join(", "));
    }
    for (i, w) in s.warnings.iter().enumerate() {
        row(if i == 0 { "Warnings" } else { "" }, format!("⚠ {w}"));
    }
}

fn confirm(question: &str, default: bool) -> Result<bool, String> {
    // Without a terminal nobody can answer: no.
    if !std::io::stdin().is_terminal() {
        return Ok(false);
    }
    print!("{question} {} ", if default { "[Y/n]" } else { "[y/N]" });
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let mut answer = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut answer)
        .map_err(|e| e.to_string())?;
    Ok(match answer.trim().to_lowercase().as_str() {
        "" => default,
        a => a.starts_with('y'),
    })
}

/// `cardea edit`: edit a copy in $EDITOR, send it to the daemon, which checks it; on errors, offer to edit again.
fn edit(id: &str) -> Result<(), String> {
    let (paths, mut c) = connect()?;
    let original: String = call(&mut c, Method::GetConfig { id: id.to_owned() })?;
    let file = paths
        .runtime
        .join(format!("edit-{id}-{}.json", std::process::id()));
    std::fs::write(&file, &original)
        .map_err(|e| format!("cannot write {}: {e}", file.display()))?;
    let result = edit_loop(&mut c, id, &file, &original);
    let _ = std::fs::remove_file(&file);
    result
}

fn edit_loop(
    c: &mut Client,
    id: &str,
    file: &std::path::Path,
    original: &str,
) -> Result<(), String> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .ok()
        .filter(|e| !e.trim().is_empty())
        .unwrap_or_else(|| "nano".into());
    let mut words = editor.split_whitespace();
    let program = words.next().expect("not empty");
    let args: Vec<&str> = words.collect();
    loop {
        let status = std::process::Command::new(program)
            .args(&args)
            .arg(file)
            .status()
            .map_err(|e| format!("cannot run the editor {program:?}: {e} (set $EDITOR)"))?;
        if !status.success() {
            return Err(format!("the editor exited with {status}; nothing saved"));
        }
        let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
        if text == original {
            out!("No changes.");
            return Ok(());
        }
        match c.call::<Saved>(Method::SetConfig {
            id: id.to_owned(),
            config: text,
        }) {
            Ok(saved) => {
                out!("{}", saved_text(saved));
                return Ok(());
            }
            Err(e) if e.code == ErrorCode::InvalidConfig => {
                eprintln!("{}", e.message);
                if !confirm("Edit again?", true)? {
                    return Err("not saved".into());
                }
            }
            Err(e) => return Err(e.message),
        }
    }
}
