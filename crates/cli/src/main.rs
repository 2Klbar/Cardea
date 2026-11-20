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

use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use cardea_core::load;
use cardea_core::model::{Config, KeyringMode, Kind, X11, Zone, ZoneMode};
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
    },
    /// Start a sandbox
    Start { id: String },
    /// Stop a sandbox (from Error: reset it to Stopped)
    Stop { id: String },
    /// Stop then start a sandbox
    Restart { id: String },
    /// Rebuild a sandbox's image (restarts it if it was running)
    Rebuild { id: String },
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
    /// Run the daemon (started automatically by the other commands when needed)
    Daemon {
        /// Use the demo engine: simulated sandboxes, nothing touches Podman or your real sandboxes
        #[arg(long)]
        demo: bool,
    },
}

#[derive(Subcommand)]
enum ProfileCommand {
    /// List the profiles
    List,
    /// Show a profile in full, with every default filled in
    Show { id: String },
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
        Command::Daemon { demo } => cardea_daemon::main(demo),
        Command::Check { kind, file } => check(kind, &file),
        Command::List => {
            let (_, mut c) = connect()?;
            let list: Vec<Summary> = call(&mut c, Method::List {})?;
            if c.hello.demo {
                out!("(demo daemon: simulated sandboxes)");
            }
            if list.is_empty() {
                out!(
                    "No sandbox yet. Create one with: cardea create --profile claude --name Claude"
                );
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
                },
            )?;
            out!("Created sandbox {:?} ({}) in {}", s.name, s.id, s.folder);
            follow(&mut c, &s.id)
        }
        Command::Start { id } => action(Method::Start { id: id.clone() }, &id),
        Command::Stop { id } => action(Method::Stop { id: id.clone() }, &id),
        Command::Restart { id } => action(Method::Restart { id: id.clone() }, &id),
        Command::Rebuild { id } => action(Method::Rebuild { id: id.clone() }, &id),
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
fn action(method: Method, id: &str) -> Result<(), String> {
    let (_, mut c) = connect()?;
    call::<()>(&mut c, Method::Subscribe {})?;
    call::<()>(&mut c, method)?;
    follow(&mut c, id)
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
            format!("{} → {} ({mode})", f.host, f.mount),
        );
    }
    let caps = &c.capabilities;
    let mut on = Vec::new();
    if caps.wayland {
        on.push("wayland".to_owned());
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
                out!(
                    "{}",
                    match saved {
                        Saved::Applied => "Saved: the change applies now.",
                        Saved::RestartRequired =>
                            "Saved: restart the sandbox to apply it (cardea restart).",
                        Saved::RebuildRequired =>
                            "Saved: rebuild the sandbox to apply it (cardea rebuild).",
                    }
                );
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
