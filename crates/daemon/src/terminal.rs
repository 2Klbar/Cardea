//! Opening a host terminal that runs a `cardea` command (terminal apps, attaching to a service), for clients that
//! have none: the manager, a menu entry.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Terminals tried in order, with the argument that comes before the command (`None`: the command follows at once).
const TERMINALS: [(&str, Option<&str>); 6] = [
    ("xdg-terminal-exec", None),
    ("konsole", Some("-e")),
    ("kgx", Some("--")),
    ("gnome-terminal", Some("--")),
    ("x-terminal-emulator", Some("-e")),
    ("xterm", Some("-e")),
];

/// The `cardea` command line program: next to the running binary, else from `PATH`.
pub fn cardea() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join("cardea")))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from("cardea"))
}

fn in_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|d| Path::new(&d).join(program).is_file())
    })
}

/// Opens a terminal running `argv`: `custom` (the `terminal` setting, an argv prefix) when set, else the first
/// terminal found. It is not waited for.
pub fn open(custom: Option<&[String]>, argv: &[String]) -> Result<(), String> {
    if let Some((program, args)) = custom.and_then(|c| c.split_first()) {
        return Command::new(program)
            .args(args)
            .args(argv)
            .spawn()
            .map(drop)
            .map_err(|e| format!("cannot open {program} (the terminal setting): {e}"));
    }
    let (program, before) = TERMINALS
        .iter()
        .find(|(p, _)| in_path(p))
        .ok_or("no terminal found: install xdg-terminal-exec, Konsole or GNOME Console")?;
    let mut cmd = Command::new(program);
    cmd.args(before.iter()).args(argv);
    cmd.spawn()
        .map(drop)
        .map_err(|e| format!("cannot open {program}: {e}"))
}
