//! `cardea links` and the link relay (`cardea link-relay`, started by the daemon for each running sandbox whose
//! links may leave).

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cardea_core::links::{self, MAX_URL};
use cardea_core::model::Trust;
use cardea_proto::{LinkOutcome, Method, SchemeInfo, SchemeStatus};
use clap::Subcommand;

use crate::apps::{OnOff, save, status};

/// At most one link forwarded per this time; faster ones are dropped.
const RATE: Duration = Duration::from_secs(3);

#[derive(Subcommand)]
pub enum LinksCommand {
    /// The schemes and who opens them, and the outgoing link settings
    Show,
    /// Let the sandbox's apps send links to your browser, or not (applies at the next start)
    Outgoing { state: OnOff },
    /// Ask before opening each link (always on for an Untrusted sandbox)
    Ask { state: OnOff },
    /// Open this host's links without asking (exact name: github.com does not cover gist.github.com)
    Allow { host: String },
    /// Ask again for this host's links
    Forget { host: String },
}

pub fn run(id: String, command: LinksCommand) -> Result<(), String> {
    let (_, mut c) = crate::connect()?;
    let st = status(&mut c, &id)?;
    let mut config = st.config;
    match command {
        LinksCommand::Show => {
            let schemes: Vec<SchemeInfo> = crate::call(&mut c, Method::Links { id: id.clone() })?;
            out!("Incoming links");
            if schemes.is_empty() {
                out!("  No app of {id} declares a scheme.");
            }
            for s in &schemes {
                let who = match &s.status {
                    SchemeStatus::Active => "opens in this sandbox".to_owned(),
                    SchemeStatus::HostApp(app) => format!(
                        "handled by a host app ({app}): Cardea leaves it; choose in your desktop's default apps settings"
                    ),
                    SchemeStatus::OtherSandbox(name) => format!("handled by the sandbox {name}"),
                    SchemeStatus::Unknown => "no default handler known on this host".to_owned(),
                };
                out!("  {}:  {}  {who}", s.scheme, s.app_name);
            }
            let l = &config.links;
            let untrusted = config.trust == Trust::Untrusted;
            out!("Outgoing links");
            out!(
                "  to your browser:  {}",
                if l.outgoing { "on" } else { "off" }
            );
            if l.outgoing {
                out!(
                    "  ask first:        {}",
                    match (untrusted, l.ask) {
                        (true, _) => "always (Untrusted sandbox)",
                        (false, true) => "yes",
                        (false, false) => "no",
                    }
                );
                if !untrusted {
                    out!(
                        "  allowed hosts:    {}",
                        if l.allowed.is_empty() {
                            "none".to_owned()
                        } else {
                            l.allowed.join(", ")
                        }
                    );
                }
            }
            if st.summary.flags.restart_required {
                out!("Restart the sandbox to apply the pending changes.");
            }
            return Ok(());
        }
        LinksCommand::Outgoing { state } => config.links.outgoing = matches!(state, OnOff::On),
        LinksCommand::Ask { state } => config.links.ask = matches!(state, OnOff::On),
        LinksCommand::Allow { host } => {
            let host = links::normalize_host(&host)?;
            if config.links.allowed.contains(&host) {
                return Err(format!("{host} is already allowed"));
            }
            config.links.allowed.push(host);
        }
        LinksCommand::Forget { host } => {
            let host = links::normalize_host(&host)?;
            let before = config.links.allowed.len();
            config.links.allowed.retain(|h| *h != host);
            if config.links.allowed.len() == before {
                return Err(format!("{host} is not among the allowed hosts"));
            }
        }
    }
    save(&mut c, &id, &config)
}

/// Reads one line of at most `max` bytes; a longer one is skipped whole. `None` at the end of the file.
fn read_line(r: &mut impl BufRead, max: usize) -> std::io::Result<Option<Result<Vec<u8>, String>>> {
    let mut buf = Vec::new();
    let n = r
        .by_ref()
        .take(max as u64 + 1)
        .read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(None);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        return Ok(Some(Ok(buf)));
    }
    if buf.len() <= max {
        // The end of the file without a line break.
        return Ok(Some(Ok(buf)));
    }
    r.skip_until(b'\n')?;
    Ok(Some(Err(format!("a line longer than {max} bytes"))))
}

/// The relay: reads the sandbox's FIFO, checks each line (R3), and passes what remains to the daemon, one link at
/// a time. Its standard error is the sandbox's `state/links.log`.
pub fn relay(fifo: &Path, sandbox: &str) -> Result<(), String> {
    // Read-write: the FIFO always has a writer (this one), so no end of file when the sandbox closes it.
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(fifo)
        .map_err(|e| format!("cannot open {}: {e}", fifo.display()))?;
    let mut reader = BufReader::new(file);
    let busy = Arc::new(AtomicBool::new(false));
    let mut last: Option<Instant> = None;
    let log = |s: &str| eprintln!("{} {s}", chrono::Local::now().format("%F %T"));
    log("Link relay started");
    // A sandbox could write lines without end: at most one "Dropped" line a second, with the count of the others.
    let (mut quiet, mut last_drop) = (0u64, None::<Instant>);
    let mut dropped = |why: &str| {
        if last_drop.is_some_and(|t: Instant| t.elapsed() < Duration::from_secs(1)) {
            quiet += 1;
            return;
        }
        let more = if quiet > 0 {
            format!(" ({quiet} more dropped before)")
        } else {
            String::new()
        };
        log(&format!("Dropped {why}{more}"));
        (quiet, last_drop) = (0, Some(Instant::now()));
    };
    loop {
        let line = match read_line(&mut reader, MAX_URL).map_err(|e| e.to_string())? {
            None => {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            Some(Err(e)) => {
                dropped(&e);
                continue;
            }
            Some(Ok(bytes)) => bytes,
        };
        let Ok(line) = String::from_utf8(line) else {
            dropped("a line that is not UTF-8");
            continue;
        };
        let url = match links::outgoing(&line) {
            Ok(o) => o.url,
            Err(e) => {
                dropped(&format!("a link: {e}"));
                continue;
            }
        };
        if last.is_some_and(|t| t.elapsed() < RATE) {
            dropped("a link: less than 3 s after the previous one");
            continue;
        }
        if busy.swap(true, Ordering::AcqRel) {
            dropped("a link: another one is waiting for your answer");
            continue;
        }
        last = Some(Instant::now());
        let (busy, sandbox) = (busy.clone(), sandbox.to_owned());
        std::thread::spawn(move || {
            let result = crate::connect().and_then(|(_, mut c)| {
                crate::call::<LinkOutcome>(&mut c, Method::Link { sandbox, url })
            });
            let text = match result {
                Ok(LinkOutcome::Opened) => "Link opened".to_owned(),
                Ok(LinkOutcome::Cancelled) => "Link cancelled".to_owned(),
                Ok(LinkOutcome::Expired) => "Link dropped: no answer in time".to_owned(),
                Err(e) => format!("Link refused: {e}"),
            };
            eprintln!("{} {text}", chrono::Local::now().format("%F %T"));
            busy.store(false, Ordering::Release);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_lines_are_dropped_whole() {
        let input = format!("https://a.com/\n{}\nhttps://b.com/\nlast", "x".repeat(20));
        let mut r = BufReader::new(input.as_bytes());
        let mut next = || read_line(&mut r, 16).unwrap();
        assert_eq!(next(), Some(Ok(b"https://a.com/".to_vec())));
        assert!(matches!(next(), Some(Err(_))));
        assert_eq!(next(), Some(Ok(b"https://b.com/".to_vec())));
        assert_eq!(next(), Some(Ok(b"last".to_vec())));
        assert_eq!(next(), None);
    }
}
