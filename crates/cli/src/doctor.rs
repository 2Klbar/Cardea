//! `cardea doctor`: the desktop session checks are done here, in the user's session; the Podman checks come from
//! the daemon, the only process that talks to Podman.

use cardea_proto::{Check, CheckStatus, Method};

pub fn run() -> Result<(), String> {
    let mut checks = cardea_daemon::doctor::desktop();
    let podman = crate::connect()
        .and_then(|(_, mut c)| crate::call::<Vec<Check>>(&mut c, Method::Doctor {}));
    match podman {
        Ok(list) => checks.extend(list),
        Err(e) => checks.push(Check {
            name: "Daemon".into(),
            status: CheckStatus::Fail,
            detail: e,
            hint: Some("Run `cardea daemon` to see why".into()),
        }),
    }
    let width = checks.iter().map(|c| c.name.len()).max().unwrap_or(0);
    for c in &checks {
        let status = match c.status {
            CheckStatus::Ok => "ok",
            CheckStatus::Warn => "warn",
            CheckStatus::Fail => "FAIL",
        };
        out!("{status:<5} {:<width$}  {}", c.name, c.detail);
        for line in c.hint.iter().flat_map(|h| h.lines()) {
            out!("{:<5} {:<width$}  → {line}", "", "");
        }
    }
    match checks
        .iter()
        .filter(|c| c.status == CheckStatus::Fail)
        .count()
    {
        0 => Ok(()),
        1 => Err("1 check failed".to_owned()),
        n => Err(format!("{n} checks failed")),
    }
}
