//! Links (security R3, R4): incoming links for the apps that declare their scheme, and links sent by a sandbox's
//! relay, opened in the host browser at once or after the owner's answer in the link window.

use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use cardea_core::links::{self, Outgoing};
use cardea_core::model::{App, Kind, Trust};
use cardea_core::state::State;
use cardea_core::{apps, validate};
use cardea_proto::{ErrorCode, LinkAsk, LinkOutcome, SchemeInfo, SchemeStatus};
use serde_json::Value;
use tokio::sync::oneshot;

use super::{Daemon, Error, Reply, fail, ok};
use crate::menu;

/// A link not answered within this time is dropped.
const ANSWER_TIME: Duration = Duration::from_secs(120);

/// A link waiting for the owner's answer in its window.
pub(super) struct Waiting {
    ask: LinkAsk,
    tx: Option<oneshot::Sender<Answer>>,
}

struct Answer {
    open: bool,
    always_sandbox: bool,
    always_domain: bool,
}

impl Daemon {
    fn waiting(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Waiting>> {
        self.links.lock().expect("links lock")
    }

    /// The schemes of a sandbox's apps, and who opens each on the host.
    pub(super) fn schemes(&self, id: &str) -> Reply {
        let config = self.with(id, |e| Ok(e.config.clone()))?;
        let names: HashMap<String, String> = self
            .lock()
            .sandboxes
            .iter()
            .map(|(k, e)| (k.clone(), e.config.name.clone()))
            .collect();
        let mut list = Vec::new();
        for a in &config.apps {
            for s in &a.schemes {
                let ours = apps::menu_file(id, &a.id);
                let status = match menu::default_handler(s) {
                    None => SchemeStatus::Unknown,
                    Some(h) if h == ours => SchemeStatus::Active,
                    Some(h) => match menu::owner(&self.paths, &h) {
                        Some(other) => {
                            SchemeStatus::OtherSandbox(names.get(&other).cloned().unwrap_or(other))
                        }
                        None => SchemeStatus::HostApp(h),
                    },
                };
                list.push(SchemeInfo {
                    scheme: s.clone(),
                    app: a.id.clone(),
                    app_name: a.name.clone(),
                    status,
                });
            }
        }
        ok(list)
    }

    /// A link a running sandbox's relay sent (checked again here, R5): opened, refused, or asked.
    pub(super) async fn link(self: &Arc<Self>, sandbox: &str, url: &str) -> Reply {
        let o = links::outgoing(url).map_err(|e| {
            self.log(sandbox, &format!("Link refused: {e}"));
            fail(ErrorCode::BadRequest, e)
        })?;
        let (config, state) = self.with(sandbox, |e| Ok((e.config.clone(), e.state)))?;
        if state != State::Running || !config.links.outgoing {
            self.log(
                sandbox,
                &format!(
                    "Link to {} refused: links cannot leave this sandbox",
                    o.host
                ),
            );
            return Err(fail(
                ErrorCode::InvalidState,
                "links cannot leave this sandbox",
            ));
        }
        let untrusted = config.trust == Trust::Untrusted;
        let quiet = !untrusted
            && o.marks.is_empty()
            && (!config.links.ask || config.links.allowed.contains(&o.host));
        if quiet {
            self.open_link(sandbox, &o);
            return ok(LinkOutcome::Opened);
        }
        let id = self.next_link.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        let ask = LinkAsk {
            id,
            sandbox: sandbox.to_owned(),
            sandbox_name: config.name.clone(),
            link: o.clone(),
            untrusted,
        };
        let (sandbox_box, domain_box) = (ask.sandbox_box(), ask.domain_box());
        self.waiting().insert(id, Waiting { ask, tx: Some(tx) });
        if let Err(e) = self.window(id, sandbox) {
            self.waiting().remove(&id);
            self.log(
                sandbox,
                &format!(
                    "Link to {} refused: the link window cannot open: {e}",
                    o.host
                ),
            );
            return Err(fail(ErrorCode::Unavailable, e));
        }
        self.log(
            sandbox,
            &format!("Link to {}: asking in the link window", o.host),
        );
        let answer = tokio::time::timeout(ANSWER_TIME, rx).await;
        self.waiting().remove(&id);
        let answer = match answer {
            Err(_) => {
                self.log(
                    sandbox,
                    &format!("Link to {} dropped: no answer within 2 minutes", o.host),
                );
                return ok(LinkOutcome::Expired);
            }
            Ok(Ok(a)) if a.open => a,
            // Cancelled, or the window closed without an answer.
            Ok(_) => {
                self.log(sandbox, &format!("Link to {} cancelled", o.host));
                return ok(LinkOutcome::Cancelled);
            }
        };
        let always_sandbox = answer.always_sandbox && sandbox_box;
        let always_domain = answer.always_domain && domain_box;
        if always_sandbox || always_domain {
            self.remember(sandbox, &o.host, always_sandbox, always_domain);
        }
        self.open_link(sandbox, &o);
        ok(LinkOutcome::Opened)
    }

    /// "Always" boxes: stop asking for this sandbox, or allow this host in it.
    fn remember(&self, sandbox: &str, host: &str, always_sandbox: bool, always_domain: bool) {
        let Ok(mut new) = self.with(sandbox, |e| Ok(e.config.clone())) else {
            return;
        };
        if always_sandbox {
            new.links.ask = false;
        }
        if always_domain && !new.links.allowed.iter().any(|h| h == host) {
            new.links.allowed.push(host.to_owned());
        }
        let saved = validate::config(&new, Kind::Sandbox)
            .map_err(|e| e.to_string())
            .and_then(|()| self.save(sandbox, new).map_err(|e| e.message));
        if let Err(e) = saved {
            self.log(sandbox, &format!("The link setting was not saved: {e}"));
        }
    }

    pub(super) fn link_request(&self, id: u64) -> Reply {
        match self.waiting().get(&id) {
            Some(w) => ok(&w.ask),
            None => Err(gone(id)),
        }
    }

    pub(super) fn link_answer(
        &self,
        id: u64,
        open: bool,
        always_sandbox: bool,
        always_domain: bool,
    ) -> Reply {
        let tx = self.waiting().get_mut(&id).and_then(|w| w.tx.take());
        match tx {
            Some(tx) => {
                let _ = tx.send(Answer {
                    open,
                    always_sandbox,
                    always_domain,
                });
                Ok(Value::Null)
            }
            None => Err(gone(id)),
        }
    }

    /// Opens the link window for a waiting link; when it ends without an answer, the link is cancelled.
    fn window(self: &Arc<Self>, id: u64, sandbox: &str) -> Result<(), String> {
        let gui = crate::terminal::cardea().with_file_name("cardea-gui");
        let program = if gui.is_file() {
            gui
        } else {
            "cardea-gui".into()
        };
        let mut child = Command::new(program)
            .arg("--link")
            .arg(id.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;
        let daemon = self.clone();
        let sandbox = sandbox.to_owned();
        std::thread::spawn(move || {
            let status = child.wait();
            let unanswered = daemon.waiting().get_mut(&id).and_then(|w| w.tx.take());
            // A failure exit (not a signal): it could not show.
            if unanswered.is_some() && status.is_ok_and(|s| s.code().is_some_and(|c| c != 0)) {
                daemon.log(
                    &sandbox,
                    "The link window could not open (no display in this session?)",
                );
            }
        });
        Ok(())
    }

    /// Hands a checked link to the host's default browser, as one argument.
    fn open_link(&self, sandbox: &str, o: &Outgoing) {
        match Command::new("xdg-open")
            .arg(&o.url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                std::thread::spawn(move || child.wait());
                self.log(
                    sandbox,
                    &format!("Opened a link to {} in your browser", o.host),
                );
            }
            Err(e) => self.log(
                sandbox,
                &format!("Link to {} not opened: xdg-open: {e}", o.host),
            ),
        }
    }
}

/// The incoming link for `app`, checked (R4): it becomes the app's last argument.
pub(super) fn incoming(app: &App, url: &str) -> Result<String, Error> {
    if app.schemes.is_empty() {
        return Err(fail(
            ErrorCode::BadRequest,
            format!("{} opens no links", app.name),
        ));
    }
    links::incoming(url, &app.schemes)
        .map(str::to_owned)
        .map_err(|e| fail(ErrorCode::BadRequest, e))
}

fn gone(id: u64) -> Error {
    fail(
        ErrorCode::NotFound,
        format!("link {id} is no longer waiting for an answer"),
    )
}
