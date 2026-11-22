//! The daemon's state: the sandboxes, their lifecycle and tasks, and the handling of every request.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use cardea_core::builtin::Manager;
use cardea_core::firewall::NetStatus;
use cardea_core::impact::{Impact, impact};
use cardea_core::model::{Config, Firewall, Folder, KeyringMode, Kind, Toolkit};
use cardea_core::model::{OnFailure, Package, Scope};
use cardea_core::services::{Instance, InstanceState};
use cardea_core::state::{self, Action, Flags, State};
use cardea_core::{apps, packages};
use cardea_core::{builtin, load, risk, validate};
use cardea_engine::{Engine, Exec, Progress, Report, Sandbox, Started, Watch};
use cardea_proto::{
    Disk, Error, ErrorCode, Event, ExecStarted, Method, ProfileInfo, Saved, Status, Summary, Task,
};
use serde_json::Value;
use tokio::sync::broadcast;

use crate::menu;
use crate::paths::Paths;
use crate::store::{self, Registered, Registry};

/// How many log lines `logs` returns before following.
mod links;

const LOG_TAIL: usize = 200;

pub type Reply = Result<Value, Error>;

/// Events a connection asked to receive.
pub enum Sub {
    All(broadcast::Receiver<Event>),
    Logs(String, broadcast::Receiver<Event>),
}

pub fn fail(code: ErrorCode, message: impl Into<String>) -> Error {
    Error {
        code,
        message: message.into(),
    }
}

fn internal(e: impl std::fmt::Display) -> Error {
    fail(ErrorCode::Internal, e.to_string())
}

fn internal_msg(message: String) -> Error {
    fail(ErrorCode::Internal, message)
}

fn ok<T: serde::Serialize>(t: T) -> Reply {
    Ok(serde_json::to_value(t).expect("protocol types serialize"))
}

struct Entry {
    config: Config,
    folder: PathBuf,
    state: State,
    flags: Flags,
    error: Option<String>,
    task: Option<Task>,
    /// The firewall state last read, while running.
    net: Option<NetStatus>,
    /// The keyring password of the next start, until the task takes it; never stored.
    password: Option<String>,
    /// Service instances last read, while running.
    services: Vec<Instance>,
    /// Services that ended on their own since the start, shown as warnings.
    service_warnings: Vec<String>,
}

struct Inner {
    sandboxes: BTreeMap<String, Entry>,
    registry: Registry,
}

/// A long task the daemon runs for a sandbox.
#[derive(Clone, Copy, Debug)]
enum Job {
    Build,
    Start,
    Stop,
    Restart,
    Rebuild { was_running: bool, fresh: bool },
}

impl Job {
    fn label(self) -> &'static str {
        match self {
            Job::Build => "build",
            Job::Start => "start",
            Job::Stop => "stop",
            Job::Restart => "restart",
            Job::Rebuild { .. } => "rebuild",
        }
    }
}

pub struct Daemon {
    paths: Paths,
    engine: Arc<dyn Engine>,
    inner: Mutex<Inner>,
    events: broadcast::Sender<Event>,
    /// Held while a log line is written and sent, so `logs --follow` sees no gap and no duplicate.
    log_lock: Mutex<()>,
    /// Held from reading a sandbox's config to applying its rules, so refreshes started one after the other
    /// cannot finish out of order and leave older rules in place.
    firewall_lock: tokio::sync::Mutex<()>,
    /// Links waiting for the owner's answer, by request id.
    links: Mutex<std::collections::HashMap<u64, links::Waiting>>,
    next_link: std::sync::atomic::AtomicU64,
}

impl Daemon {
    /// Loads the registered sandboxes (needs a Tokio runtime).
    pub fn open(paths: Paths, engine: Arc<dyn Engine>) -> Result<Arc<Daemon>, String> {
        let registry = store::load_registry(&paths)?;
        if let Ok(s) = store::load_settings(&paths)
            && let Err(e) = menu::tray_autostart(&paths, s.tray_at_login)
        {
            eprintln!("cardea daemon: tray login entry: {e}");
        }
        let mut sandboxes = BTreeMap::new();
        for r in &registry.sandboxes {
            let entry = match store::load_config(&r.folder) {
                Ok(config) => Entry::new(config, r.folder.clone(), State::Stopped),
                Err(e) => {
                    let mut config = Config::defaults(cardea_core::model::Trust::Work);
                    config.id = r.id.clone();
                    config.name = r.id.clone();
                    let mut entry = Entry::new(config, r.folder.clone(), State::Error);
                    entry.error = Some(e);
                    entry
                }
            };
            sandboxes.insert(r.id.clone(), entry);
        }
        let (events, _) = broadcast::channel(1024);
        let daemon = Arc::new(Daemon {
            paths,
            engine,
            inner: Mutex::new(Inner {
                sandboxes,
                registry,
            }),
            events,
            log_lock: Mutex::new(()),
            firewall_lock: tokio::sync::Mutex::new(()),
            links: Mutex::new(std::collections::HashMap::new()),
            next_link: std::sync::atomic::AtomicU64::new(1),
        });
        tokio::spawn(daemon.clone().watch_engine());
        tokio::spawn(daemon.clone().firewall_loop());
        Ok(daemon)
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    /// Follows the real engine: reads which sandboxes run, then their stats and exits; again after each loss of
    /// the engine's streams.
    async fn watch_engine(self: Arc<Self>) {
        let mut first = true;
        loop {
            match self.engine.running().await {
                Ok(running) => {
                    self.sync_running(&running);
                    if std::mem::take(&mut first) {
                        self.start_at_login();
                    }
                }
                Err(e) => eprintln!("cardea daemon: {e}"),
            }
            let daemon = self.clone();
            if let Err(e) = self
                .engine
                .watch(Arc::new(move |w| daemon.on_watch(w)))
                .await
            {
                eprintln!("cardea daemon: {e}");
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }

    /// Every 5 s, the firewall of each idle running sandbox follows the host's network (R7).
    async fn firewall_loop(self: Arc<Self>) {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            let ids: Vec<String> = self
                .lock()
                .sandboxes
                .values()
                .filter(|e| e.state == State::Running && e.task.is_none())
                .map(|e| e.config.id.clone())
                .collect();
            for id in ids {
                self.refresh_firewall(&id).await;
                self.refresh_services(&id).await;
            }
        }
    }

    /// Applies a firewall change to a running sandbox now.
    /// After a configuration change: the firewall of a running sandbox, and the menu entries with any new icon.
    fn kick_firewall(self: &Arc<Self>, id: &str) {
        let (daemon, id) = (self.clone(), id.to_owned());
        tokio::spawn(async move {
            daemon.refresh_firewall(&id).await;
            let running = daemon.running(&id).is_ok();
            daemon.sync_menu(&id, running).await;
        });
    }

    async fn refresh_firewall(&self, id: &str) {
        let _serial = self.firewall_lock.lock().await;
        let Some((config, folder)) = self
            .with(id, |e| {
                Ok((e.state == State::Running && e.task.is_none())
                    .then(|| (e.config.clone(), e.folder.clone())))
            })
            .ok()
            .flatten()
        else {
            return;
        };
        let home = home_of(&self.paths, &config, &folder);
        let sandbox = Sandbox {
            config: &config,
            folder: &folder,
            home: &home,
            password: None,
            fresh: false,
        };
        let result = self.engine.firewall(&sandbox).await;
        let mut inner = self.lock();
        let Some(e) = inner.sandboxes.get_mut(id) else {
            return;
        };
        if e.state != State::Running || e.task.is_some() {
            return;
        }
        match result {
            Ok(None) => {}
            Ok(Some(status)) => {
                let old = e.net.replace(status.clone()).unwrap_or_default();
                drop(inner);
                for b in status.blocked.iter().filter(|b| {
                    !old.blocked
                        .iter()
                        .any(|o| o.zone == b.zone && o.address == b.address)
                }) {
                    self.log(
                        id,
                        &format!(
                            "Firewall blocked a connection to {} ({} zone)",
                            b.address, b.zone
                        ),
                    );
                }
                if old != status {
                    self.emit(Event::Net {
                        id: id.to_owned(),
                        status,
                    });
                }
            }
            // Fail closed: the namespace keeps its last rules, the sandbox shows the error.
            Err(message) => {
                let message = format!("the firewall could not be updated: {message}");
                e.state = State::Error;
                e.error = Some(message.clone());
                self.changed(&inner, id);
                drop(inner);
                self.log(id, &capitalize(&message));
            }
        }
    }

    /// When the daemon starts (at login): the stopped sandboxes set to start automatically. A keyring that asks for
    /// its password waits for a start from the manager.
    fn start_at_login(self: &Arc<Self>) {
        let ids: Vec<(String, bool)> = self
            .lock()
            .sandboxes
            .values()
            .filter(|e| e.config.autostart == Some(true) && e.state == State::Stopped)
            .map(|e| {
                (
                    e.config.id.clone(),
                    e.config.capabilities.keyring.mode == KeyringMode::Ask,
                )
            })
            .collect();
        for (id, ask) in ids {
            if ask {
                self.log(
                    &id,
                    "Not started at login: its keyring asks for its password",
                );
            } else if let Err(e) = self.begin(&id, Action::Start, Job::Start, None) {
                self.log(&id, &format!("Not started at login: {}", e.message));
            }
        }
    }

    /// Idle sandboxes take the state the engine reports.
    fn sync_running(&self, running: &[String]) {
        let mut inner = self.lock();
        let ids: Vec<String> = inner.sandboxes.keys().cloned().collect();
        for id in ids {
            let e = inner.sandboxes.get_mut(&id).expect("listed above");
            let runs = running.contains(&id);
            let state = match e.state {
                State::Stopped if runs && e.task.is_none() => State::Running,
                State::Running if !runs && e.task.is_none() => State::Stopped,
                _ => continue,
            };
            e.state = state;
            self.changed(&inner, &id);
        }
    }

    fn on_watch(&self, w: Watch) {
        match w {
            Watch::Stats { id, cpu, memory } => {
                let limit = match self.lock().sandboxes.get(&id) {
                    Some(e) if e.state == State::Running => e.config.security.ram_limit_gb,
                    _ => return,
                };
                self.emit(Event::Stats {
                    id,
                    cpu,
                    memory,
                    memory_limit: limit.map(|gb| u64::from(gb) << 30),
                });
            }
            Watch::Exited { id, code, sidecar } => {
                let code = code
                    .map(|c| format!(" (exit code {c})"))
                    .unwrap_or_default();
                let message = if sidecar {
                    format!(
                        "the firewall sidecar stopped unexpectedly{code}; the sandbox keeps its last rules, stop it"
                    )
                } else {
                    format!("the sandbox stopped unexpectedly{code}")
                };
                {
                    let mut inner = self.lock();
                    let Some(e) = inner.sandboxes.get_mut(&id) else {
                        return;
                    };
                    // An exit during a task is the task's own doing.
                    if e.state != State::Running || e.task.is_some() {
                        return;
                    }
                    e.state = State::Error;
                    e.error = Some(message.clone());
                    self.changed(&inner, &id);
                }
                self.log(&id, &capitalize(&message));
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().expect("daemon state lock")
    }

    fn emit(&self, event: Event) {
        // No subscriber is fine.
        let _ = self.events.send(event);
    }

    fn log(&self, id: &str, text: &str) {
        let _guard = self.log_lock.lock().expect("log lock");
        let line = store::append_log(&self.paths, id, text);
        self.emit(Event::Log {
            id: id.to_owned(),
            line,
        });
    }

    fn summary(&self, e: &Entry) -> Summary {
        Summary {
            id: e.config.id.clone(),
            name: e.config.name.clone(),
            icon: e.config.icon.clone(),
            state: e.state,
            flags: e.flags,
            error: e.error.clone(),
            task: e.task.clone(),
            warnings: risk::warnings(&e.config)
                .into_iter()
                .chain(e.service_warnings.iter().cloned())
                .collect(),
            folder: self.paths.display(&e.folder),
        }
    }

    fn changed(&self, inner: &Inner, id: &str) {
        if let Some(e) = inner.sandboxes.get(id) {
            self.emit(Event::Changed {
                sandbox: self.summary(e),
            });
        }
    }

    pub fn hello(&self) -> cardea_proto::Hello {
        cardea_proto::Hello {
            version: cardea_proto::PROTOCOL_VERSION,
            daemon_version: cardea_core::VERSION.to_owned(),
            engine_error: self.engine.problem(),
        }
    }

    /// Handles one request (`hello` is handled by the connection).
    pub async fn handle(self: &Arc<Self>, method: Method) -> (Reply, Option<Sub>) {
        let reply = match method {
            Method::Hello { .. } => ok(self.hello()),
            Method::Subscribe {} => {
                return (Ok(Value::Null), Some(Sub::All(self.events.subscribe())));
            }
            Method::Logs { id, follow } => return self.logs(&id, follow),
            Method::List {} => {
                // In registry order: creation order.
                let inner = self.lock();
                let list: Vec<_> = inner
                    .registry
                    .sandboxes
                    .iter()
                    .filter_map(|r| inner.sandboxes.get(&r.id))
                    .map(|e| self.summary(e))
                    .collect();
                ok(list)
            }
            Method::Status { id } => self.with(&id, |e| {
                ok(Status {
                    summary: self.summary(e),
                    config: e.config.clone(),
                    missing_folders: e
                        .config
                        .folders
                        .iter()
                        .filter(|f| !Path::new(&f.host).is_dir())
                        .map(|f| f.host.clone())
                        .collect(),
                })
            }),
            Method::Profiles {} => ok(self.profiles()),
            Method::Profile { id } => match self.profile(&id) {
                Some(p) => ok(p),
                None => Err(fail(ErrorCode::NotFound, format!("no profile {id:?}"))),
            },
            Method::Create {
                profile,
                name,
                config,
                location,
            } => self.create(profile, name, config, location),
            Method::ProfileImport { config, replace } => self.profile_import(&config, replace),
            Method::ProfileDelete { id } => {
                if builtin::profile(&id).is_some() {
                    Err(fail(
                        ErrorCode::InvalidState,
                        "built-in profiles cannot be deleted",
                    ))
                } else {
                    match fs::remove_file(store::user_profile_file(&self.paths, &id)) {
                        Ok(()) => Ok(Value::Null),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(fail(
                            ErrorCode::NotFound,
                            format!("no profile {id:?} of yours"),
                        )),
                        Err(e) => Err(internal(e)),
                    }
                }
            }
            Method::ProfileSave { id, profile, name } => self.profile_save(&id, &profile, &name),
            Method::Settings {} => store::load_settings(&self.paths)
                .map_err(internal_msg)
                .and_then(ok),
            Method::SetSettings { settings } => self.set_settings(&settings),
            Method::Disk { id } => self.disk(&id).await,
            Method::Start { id, password } => self
                .begin(&id, Action::Start, Job::Start, password)
                .map(|()| Value::Null),
            Method::Stop { id } => self
                .begin(&id, Action::Stop, Job::Stop, None)
                .map(|()| Value::Null),
            Method::Restart { id, password } => self
                .begin(&id, Action::Restart, Job::Restart, password)
                .map(|()| Value::Null),
            Method::Rebuild {
                id,
                fresh,
                password,
            } => {
                let was_running = self
                    .with(&id, |e| Ok(e.state == State::Running))
                    .unwrap_or(false);
                self.begin(
                    &id,
                    Action::Rebuild,
                    Job::Rebuild { was_running, fresh },
                    password,
                )
                .map(|()| Value::Null)
            }
            Method::Packages { id } => self.packages(&id).await,
            Method::PackageAdd { id, package } => self.package_add(&id, package).await,
            Method::PackageRemove { id, source, name } => {
                self.package_remove(&id, &source, &name).await
            }
            Method::PackageUpdate { id, source, name } => {
                self.package_update(&id, source, name).await
            }
            Method::Launch {
                id,
                app,
                args,
                url,
                password,
            } => self.launch(&id, &app, args, url, password).await,
            Method::Links { id } => self.schemes(&id),
            Method::Link { sandbox, url } => self.link(&sandbox, &url).await,
            Method::LinkRequest { id } => self.link_request(id),
            Method::LinkAnswer {
                id,
                open,
                always_sandbox,
                always_domain,
            } => self.link_answer(id, open, always_sandbox, always_domain),
            Method::DetectApps { id } => self.detect_apps(&id).await,
            Method::Services { id } => self.services(&id).await,
            Method::Service {
                id,
                service,
                folder,
                start,
            } => self.service(&id, &service, folder.as_deref(), start).await,
            Method::Delete { id, keep_home } => self.delete(&id, keep_home).await,
            Method::GetConfig { id } => self.with(&id, |e| {
                fs::read_to_string(store::config_file(&e.folder))
                    .map(Value::String)
                    .map_err(internal)
            }),
            Method::SetConfig { id, config } => {
                let saved = self.set_config(&id, &config);
                if saved.is_ok() {
                    self.kick_firewall(&id);
                }
                saved
            }
            Method::SetFirewall { id, firewall } => {
                let saved = self.set_firewall(&id, firewall);
                if saved.is_ok() {
                    self.kick_firewall(&id);
                }
                saved
            }
            Method::Net { id } => self.with(&id, |e| {
                ok(e.net
                    .clone()
                    .filter(|_| e.state == State::Running)
                    .unwrap_or_default())
            }),
            Method::Doctor {} => ok(crate::doctor::podman(&self.paths.podman).await),
            Method::SetFolders {
                id,
                folders,
                protected,
            } => self.set_folders(&id, folders, protected),
            // Answered by the connection, which hands over the file descriptor.
            Method::Exec { .. } => {
                Err(fail(ErrorCode::BadRequest, "exec needs its own connection"))
            }
            Method::ExecWait { exec } => self
                .engine
                .exec_wait(&exec)
                .await
                .map_err(internal_msg)
                .map(Value::from),
            Method::ExecResize { exec, rows, cols } => {
                // A resize can arrive before the command starts or after it ends: not worth an error.
                let _ = self.engine.exec_resize(&exec, rows, cols).await;
                Ok(Value::Null)
            }
        };
        (reply, None)
    }

    /// Starts a command in a running sandbox; the server hands its connection over to the client.
    pub async fn exec(
        &self,
        id: &str,
        mut exec: Exec,
        app: Option<String>,
        test_wayland: bool,
    ) -> Result<(ExecStarted, Started), Error> {
        if let Some(app) = app {
            let config = self.with(id, |e| Ok(e.config.clone()))?;
            let a = config
                .apps
                .iter()
                .find(|a| a.id == app)
                .ok_or_else(|| fail(ErrorCode::NotFound, format!("{id} has no app {app:?}")))?;
            let args = exec.cmd.take().unwrap_or_default();
            let l = if test_wayland {
                apps::test_wayland(a, &args)
            } else {
                apps::launch(&config, a, &args)
            };
            exec.cmd = Some(l.argv);
            exec.env = l.env;
        }
        exec.term = exec.term.filter(|t| {
            t.len() <= 64
                && t.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-._+".contains(c))
        });
        self.with(id, |e| match e.state {
            State::Running => Ok(()),
            _ => Err(fail(
                ErrorCode::InvalidState,
                format!("{id} is not running"),
            )),
        })?;
        let started = self.engine.exec(id, exec).await.map_err(internal_msg)?;
        Ok((
            ExecStarted {
                exec: started.exec.clone(),
            },
            started,
        ))
    }

    /// The configuration of a running sandbox.
    fn running(&self, id: &str) -> Result<Config, Error> {
        self.with(id, |e| match e.state {
            State::Running => Ok(e.config.clone()),
            _ => Err(fail(
                ErrorCode::InvalidState,
                format!("{id} is not running"),
            )),
        })
    }

    async fn detect_apps(&self, id: &str) -> Reply {
        self.running(id)?;
        self.engine
            .detect_apps(id)
            .await
            .map_err(internal_msg)
            .and_then(ok)
    }

    async fn services(&self, id: &str) -> Reply {
        let config = self.running(id)?;
        self.engine
            .services(&config)
            .await
            .map_err(internal_msg)
            .and_then(ok)
    }

    async fn service(&self, id: &str, service: &str, folder: Option<&str>, start: bool) -> Reply {
        let config = self.running(id)?;
        self.engine
            .service(&config, service, folder, start)
            .await
            .map_err(internal_msg)?;
        let name = cardea_core::services::session(service, folder);
        self.log(
            id,
            &format!(
                "Service {name} {}",
                if start { "started" } else { "stopped" }
            ),
        );
        self.clear_service_warning(id, &name);
        self.refresh_services(id).await;
        Ok(Value::Null)
    }

    /// Starts an app; a stopped sandbox is started first, and the app follows once it runs.
    async fn launch(
        self: &Arc<Self>,
        id: &str,
        app: &str,
        mut args: Vec<String>,
        url: Option<String>,
        password: Option<String>,
    ) -> Reply {
        let (config, state) = self.with(id, |e| Ok((e.config.clone(), e.state)))?;
        let a = config
            .apps
            .iter()
            .find(|a| a.id == app)
            .ok_or_else(|| fail(ErrorCode::NotFound, format!("{id} has no app {app:?}")))?;
        // Last, after any free argument: never read as an option (R4).
        if let Some(url) = url {
            args.push(links::incoming(a, &url)?);
        }
        if a.toolkit == Toolkit::Terminal {
            return Err(fail(
                ErrorCode::BadRequest,
                format!(
                    "{} runs in a terminal: `cardea app {id} {app}` opens it there",
                    a.name
                ),
            ));
        }
        if matches!(state, State::Stopped | State::Error) {
            self.begin(id, Action::Start, Job::Start, password)?;
            // shortcut: waits by polling the state; fine for a start of a few seconds, an event wait would do too.
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                let (state, error) = self.with(id, |e| Ok((e.state, e.error.clone())))?;
                match state {
                    State::Starting => continue,
                    State::Running => break,
                    _ => {
                        return Err(fail(
                            ErrorCode::InvalidState,
                            error.unwrap_or_else(|| format!("{id} did not start")),
                        ));
                    }
                }
            }
        } else if state != State::Running {
            return Err(fail(
                ErrorCode::InvalidState,
                format!("{id} is busy, try again when its task ends"),
            ));
        }
        self.engine
            .launch(id, apps::launch(&config, a, &args))
            .await
            .map_err(internal_msg)?;
        self.log(id, &format!("Launched {}", a.name));
        Ok(Value::Null)
    }

    /// Reads the service instances of a running sandbox; an instance that ended on its own becomes a warning.
    async fn refresh_services(&self, id: &str) {
        let Ok(config) = self.running(id) else { return };
        if config.services.is_empty() {
            return;
        }
        let Ok(list) = self.engine.services(&config).await else {
            return;
        };
        let mut ended = Vec::new();
        {
            let mut inner = self.lock();
            let Some(e) = inner.sandboxes.get_mut(id) else {
                return;
            };
            if e.services == list {
                return;
            }
            for i in &list {
                let was_running = e.services.iter().any(|o| {
                    o.service == i.service
                        && o.folder == i.folder
                        && o.state == InstanceState::Running
                });
                if let (InstanceState::Exited(code), true) = (i.state, was_running) {
                    let name = cardea_core::services::session(&i.service, i.folder.as_deref());
                    let warning = format!("service {name} exited (code {code})");
                    e.service_warnings
                        .retain(|w| !w.starts_with(&format!("service {name} ")));
                    e.service_warnings.push(warning.clone());
                    ended.push(warning);
                }
            }
            e.services = list.clone();
            self.changed(&inner, id);
        }
        for w in ended {
            self.log(id, &capitalize(&w));
        }
        self.emit(Event::Services {
            id: id.to_owned(),
            services: list,
        });
    }

    fn clear_service_warning(&self, id: &str, session: &str) {
        let mut inner = self.lock();
        if let Some(e) = inner.sandboxes.get_mut(id) {
            e.service_warnings
                .retain(|w| !w.starts_with(&format!("service {session} ")));
            self.changed(&inner, id);
        }
    }

    /// Writes the host menu entries of a sandbox's apps; with `icons`, first copies out the icons it lacks (the
    /// sandbox must run).
    async fn sync_menu(&self, id: &str, icons: bool) {
        let Ok(mut config) = self.with(id, |e| Ok(e.config.clone())) else {
            return;
        };
        if icons {
            let lacking = |a: &cardea_core::model::App| {
                a.desktop_entry && (a.icon.is_none() || a.wm_class.is_none())
            };
            if config.apps.iter().any(lacking) {
                match self.engine.detect_apps(id).await {
                    // Saved, so the menu entry keeps them while the sandbox is stopped.
                    Ok(found) => {
                        let before = config.clone();
                        config
                            .apps
                            .iter_mut()
                            .for_each(|a| cardea_core::apps::complete(a, &found));
                        if config != before {
                            let saved = cardea_core::validate::config(&config, Kind::Sandbox)
                                .map_err(|e| e.to_string())
                                .and_then(|()| {
                                    self.save(id, config.clone()).map_err(|e| e.message)
                                });
                            if let Err(e) = saved {
                                self.log(id, &format!("The apps' icons were not saved: {e}"));
                            }
                        }
                    }
                    Err(e) => self.log(id, &format!("The apps' desktop files were not read: {e}")),
                }
            }
            for a in config.apps.iter().filter(|a| a.desktop_entry) {
                let Some(name) = &a.icon else { continue };
                let target = menu::icon_path(&self.paths, id, &a.id);
                if target.exists() {
                    continue;
                }
                match self.engine.icon(id, name).await {
                    Ok(Some((path, bytes))) => {
                        if let Err(e) = menu::save_icon(&target, &path, &bytes) {
                            self.log(id, &format!("Icon of {} not used: {e}", a.name));
                        }
                    }
                    Ok(None) => {}
                    Err(e) => self.log(id, &format!("Icon of {} not read: {e}", a.name)),
                }
            }
        }
        if let Err(e) = menu::sync(&self.paths, &config, true) {
            self.log(
                id,
                &format!("The desktop menu entries could not be written: {e}"),
            );
        }
    }

    fn with<T>(&self, id: &str, f: impl FnOnce(&Entry) -> Result<T, Error>) -> Result<T, Error> {
        let inner = self.lock();
        match inner.sandboxes.get(id) {
            Some(e) => f(e),
            None => Err(not_found(id)),
        }
    }

    fn logs(&self, id: &str, follow: bool) -> (Reply, Option<Sub>) {
        if let Err(e) = self.with(id, |_| Ok(())) {
            return (Err(e), None);
        }
        let _guard = self.log_lock.lock().expect("log lock");
        let lines = store::tail_log(&self.paths, id, LOG_TAIL);
        let sub = follow.then(|| Sub::Logs(id.to_owned(), self.events.subscribe()));
        (ok(lines), sub)
    }

    fn profiles(&self) -> Vec<ProfileInfo> {
        let used: Vec<(Option<String>, String)> = {
            let inner = self.lock();
            inner
                .sandboxes
                .values()
                .map(|e| (e.config.profile.clone(), e.config.name.clone()))
                .collect()
        };
        let info = |p: &Config, builtin| ProfileInfo {
            id: p.id.clone(),
            name: p.name.clone(),
            description: p.description.clone(),
            builtin,
            icon: p.icon.clone(),
            base_image: p.base_image.clone(),
            packages: p.packages.len(),
            apps: p.apps.len(),
            used_by: used
                .iter()
                .filter(|(pid, _)| pid.as_deref() == Some(p.id.as_str()))
                .map(|(_, n)| n.clone())
                .collect(),
        };
        let mut list: Vec<_> = builtin::profile_ids()
            .filter_map(builtin::profile)
            .map(|p| info(&p, true))
            .collect();
        for p in self.user_profiles() {
            list.push(info(&p, false));
        }
        list
    }

    fn profile_save(&self, id: &str, profile: &str, name: &str) -> Reply {
        let config = self.with(id, |e| Ok(e.config.clone()))?;
        let p = cardea_core::profile::from_sandbox(&config, profile, name.trim());
        self.save_profile(&p, false)
    }

    fn set_settings(&self, settings: &cardea_core::model::Settings) -> Reply {
        validate::settings(settings).map_err(|e| {
            fail(
                ErrorCode::InvalidConfig,
                format!("the settings are not valid:\n{e}"),
            )
        })?;
        store::save_settings(&self.paths, settings).map_err(internal)?;
        menu::tray_autostart(&self.paths, settings.tray_at_login).map_err(internal)?;
        Ok(Value::Null)
    }

    async fn disk(&self, id: &str) -> Reply {
        let (folder, home) = self.with(id, |e| {
            Ok((e.folder.clone(), home_of(&self.paths, &e.config, &e.folder)))
        })?;
        let disk = tokio::task::spawn_blocking(move || Disk {
            folder: store::disk_use(&folder),
            home: store::disk_use(&home),
        })
        .await
        .map_err(internal)?;
        ok(disk)
    }

    /// Validates a profile file (R10: no host path, IP address or local setting) and saves it.
    fn profile_import(&self, text: &str, replace: bool) -> Reply {
        let p = load::config(text, Kind::Profile).map_err(|e| {
            fail(
                ErrorCode::InvalidConfig,
                format!("the profile is not valid:\n{e}"),
            )
        })?;
        self.save_profile(&p, replace)
    }

    fn save_profile(&self, p: &Config, replace: bool) -> Reply {
        validate::config(p, Kind::Profile).map_err(|e| {
            fail(
                ErrorCode::InvalidConfig,
                format!("the profile is not valid:\n{e}"),
            )
        })?;
        if builtin::profile(&p.id).is_some() {
            return Err(fail(
                ErrorCode::AlreadyExists,
                format!("{:?} is a built-in profile: give yours another id", p.id),
            ));
        }
        if !replace && store::user_profile_file(&self.paths, &p.id).exists() {
            return Err(fail(
                ErrorCode::AlreadyExists,
                format!("you already have a profile {:?}", p.id),
            ));
        }
        store::save_user_profile(&self.paths, p).map_err(internal)?;
        let info = self.profiles().into_iter().find(|i| i.id == p.id);
        info.map_or_else(
            || {
                Err(fail(
                    ErrorCode::Internal,
                    "the profile was saved but cannot be read",
                ))
            },
            ok,
        )
    }

    /// User profiles, without those reusing a built-in id.
    fn user_profiles(&self) -> Vec<Config> {
        let (profiles, errors) = store::load_user_profiles(&self.paths);
        for e in errors {
            eprintln!("cardea daemon: {e}");
        }
        profiles
            .into_iter()
            .filter(|p| builtin::profile(&p.id).is_none())
            .collect()
    }

    fn profile(&self, id: &str) -> Option<Config> {
        builtin::profile(id).or_else(|| self.user_profiles().into_iter().find(|p| p.id == id))
    }

    fn create(
        self: &Arc<Self>,
        profile: Option<String>,
        name: Option<String>,
        text: Option<String>,
        location: Option<String>,
    ) -> Reply {
        let config = match (profile, text) {
            (Some(pid), None) => {
                let name = name.ok_or_else(|| fail(ErrorCode::BadRequest, "a name is needed"))?;
                let mut c = self
                    .profile(&pid)
                    .ok_or_else(|| fail(ErrorCode::NotFound, format!("no profile {pid:?}")))?;
                c.id = self.free_id(&slug(&name));
                c.name = name.trim().to_owned();
                c.profile = Some(pid);
                c
            }
            (None, Some(text)) => load::config(&text, Kind::Sandbox).map_err(|e| {
                fail(
                    ErrorCode::InvalidConfig,
                    format!("the configuration is not valid:\n{e}"),
                )
            })?,
            _ => {
                return Err(fail(
                    ErrorCode::BadRequest,
                    "give either a profile and a name, or a configuration",
                ));
            }
        };
        validate::config(&config, Kind::Sandbox).map_err(|e| {
            fail(
                ErrorCode::InvalidConfig,
                format!("the configuration is not valid:\n{e}"),
            )
        })?;
        if self.lock().sandboxes.contains_key(&config.id) {
            return Err(fail(
                ErrorCode::AlreadyExists,
                format!("a sandbox {:?} already exists", config.id),
            ));
        }
        let settings = store::load_settings(&self.paths).map_err(internal)?;
        let location = location
            .filter(|l| !l.trim().is_empty())
            .or(settings.sandbox_location)
            .map_or_else(
                || self.paths.sandboxes.clone(),
                |l| self.paths.expand(l.trim()),
            );
        if !location.is_absolute() {
            return Err(fail(
                ErrorCode::BadRequest,
                format!("{} is not an absolute folder", location.display()),
            ));
        }
        let folder = location.join(&config.id);
        if folder.exists() && fs::read_dir(&folder).map_err(internal)?.next().is_some() {
            return Err(fail(
                ErrorCode::AlreadyExists,
                format!(
                    "the folder {} already exists and is not empty",
                    self.paths.display(&folder)
                ),
            ));
        }
        let id = config.id.clone();
        let summary = self.register(&config, &folder)?;
        self.emit(Event::Added {
            sandbox: summary.clone(),
        });
        self.log(
            &id,
            &format!(
                "Created from {}",
                config
                    .profile
                    .as_deref()
                    .map_or("a configuration file".into(), |p| format!("profile {p}"))
            ),
        );
        self.spawn(id, Job::Build, true);
        ok(summary)
    }

    /// Creates the sandbox folder and files and registers it, in Building state.
    fn register(&self, config: &Config, folder: &Path) -> Result<Summary, Error> {
        let mut inner = self.lock();
        let home = home_of(&self.paths, config, folder);
        if let Some(other) = inner
            .sandboxes
            .values()
            .find(|e| home_of(&self.paths, &e.config, &e.folder) == home)
        {
            return Err(fail(
                ErrorCode::AlreadyExists,
                format!(
                    "the home {} is already used by sandbox {:?}",
                    self.paths.display(&home),
                    other.config.id
                ),
            ));
        }
        for dir in [folder.join("home"), folder.join("state")] {
            fs::create_dir_all(dir).map_err(internal)?;
        }
        store::save_config(folder, config).map_err(internal)?;
        if let Err(e) = menu::sync(&self.paths, config, true) {
            eprintln!("cardea daemon: menu entries of {}: {e}", config.id);
        }
        inner.registry.schema = 1;
        inner.registry.sandboxes.push(Registered {
            id: config.id.clone(),
            folder: folder.to_owned(),
        });
        store::save_registry(&self.paths, &inner.registry).map_err(internal)?;
        let entry = Entry::new(config.clone(), folder.to_owned(), State::Building);
        let summary = self.summary(&entry);
        inner.sandboxes.insert(config.id.clone(), entry);
        Ok(summary)
    }

    fn free_id(&self, base: &str) -> String {
        let inner = self.lock();
        let taken =
            |id: &str| inner.sandboxes.contains_key(id) || self.paths.sandboxes.join(id).exists();
        if !taken(base) {
            return base.to_owned();
        }
        (2..)
            .map(|n| {
                let suffix = format!("-{n}");
                format!(
                    "{}{suffix}",
                    base[..base.len().min(32 - suffix.len())].trim_end_matches('-')
                )
            })
            .find(|id| !taken(id))
            .expect("some id is free")
    }

    /// Checks that `action` is allowed, marks the task as running and starts it.
    fn begin(
        self: &Arc<Self>,
        id: &str,
        action: Action,
        job: Job,
        password: Option<String>,
    ) -> Result<(), Error> {
        {
            let mut inner = self.lock();
            let e = inner.sandboxes.get_mut(id).ok_or_else(|| not_found(id))?;
            let starts = matches!(
                job,
                Job::Start
                    | Job::Restart
                    | Job::Rebuild {
                        was_running: true,
                        ..
                    }
            );
            if starts
                && e.config.capabilities.keyring.mode == KeyringMode::Ask
                && password.is_none()
            {
                return Err(fail(
                    ErrorCode::PasswordRequired,
                    format!("the keyring of {id} asks for its password at each start"),
                ));
            }
            e.password = password;
            if let Some(t) = &e.task {
                return Err(fail(
                    ErrorCode::InvalidState,
                    format!("{} is busy: {} in progress", id, t.label),
                ));
            }
            state::check(e.state, action).map_err(|m| fail(ErrorCode::InvalidState, m))?;
        }
        self.spawn(id.to_owned(), job, false);
        Ok(())
    }

    fn set_state(&self, id: &str, state: State) {
        let mut inner = self.lock();
        if let Some(e) = inner.sandboxes.get_mut(id) {
            e.state = state;
        }
        self.changed(&inner, id);
    }

    fn progress(self: &Arc<Self>, id: &str) -> Progress {
        let (daemon, id) = (self.clone(), id.to_owned());
        Progress::new(move |report| {
            if let Report::Log(line) = &report {
                return daemon.log(&id, line);
            }
            let mut inner = daemon.lock();
            let Some(task) = inner.sandboxes.get_mut(&id).and_then(|e| e.task.as_mut()) else {
                return;
            };
            match report {
                Report::Step { step, steps, label } => {
                    (task.step, task.steps, task.step_label) = (step, steps, label);
                }
                Report::Percent(p) => task.percent = p,
                Report::Log(_) => unreachable!(),
            }
            let task = Some(task.clone());
            daemon.emit(Event::Task {
                id: id.clone(),
                task,
            });
        })
    }

    /// Runs `job` in the background. `seeded`: the state is already set (create).
    fn spawn(self: &Arc<Self>, id: String, job: Job, seeded: bool) {
        let first = match job {
            Job::Build => State::Building,
            Job::Start => State::Starting,
            Job::Stop | Job::Restart => State::Stopping,
            Job::Rebuild { .. } => State::Rebuilding,
        };
        let (config, folder, password) = {
            let mut inner = self.lock();
            let Some(e) = inner.sandboxes.get_mut(&id) else {
                return;
            };
            if !seeded {
                // Retrying a failed build is a build, not a rebuild.
                e.state = if e.state == State::BuildFailed {
                    State::Building
                } else {
                    first
                };
            }
            e.error = None;
            e.task = Some(Task {
                label: job.label().into(),
                step: 0,
                steps: 0,
                step_label: String::new(),
                percent: 0,
            });
            let config = (e.config.clone(), e.folder.clone(), e.password.take());
            self.changed(&inner, &id);
            config
        };
        self.log(&id, &format!("{} started", capitalize(job.label())));
        let daemon = self.clone();
        tokio::spawn(async move {
            let home = home_of(&daemon.paths, &config, &folder);
            let sandbox = Sandbox {
                config: &config,
                folder: &folder,
                home: &home,
                password: password.as_deref(),
                fresh: matches!(job, Job::Rebuild { fresh: true, .. }),
            };
            let result = daemon.run(&id, job, &sandbox).await;
            let running = result == Ok(State::Running);
            daemon.finish(&id, job, result);
            if running {
                daemon.refresh_services(&id).await;
                daemon.sync_menu(&id, true).await;
            }
        });
    }

    /// The work of a job; returns the final state, or the failure state and its message.
    async fn run(
        self: &Arc<Self>,
        id: &str,
        job: Job,
        config: &Sandbox<'_>,
    ) -> Result<State, (State, String)> {
        let engine = &self.engine;
        let p = self.progress(id);
        match job {
            Job::Build => engine
                .build(config, p)
                .await
                .map(|()| State::Stopped)
                .map_err(|e| (State::BuildFailed, e)),
            Job::Start => self.start_all(config, p).await,
            Job::Stop => engine
                .stop(id, p)
                .await
                .map(|()| State::Stopped)
                .map_err(|e| (State::Error, e)),
            Job::Restart => {
                engine
                    .stop(id, p.clone())
                    .await
                    .map_err(|e| (State::Error, e))?;
                self.set_state(id, State::Starting);
                self.start_all(config, p).await
            }
            Job::Rebuild { was_running, .. } => {
                engine
                    .build(config, p.clone())
                    .await
                    .map_err(|e| (State::BuildFailed, e))?;
                if !was_running {
                    return Ok(State::Stopped);
                }
                engine
                    .stop(id, p.clone())
                    .await
                    .map_err(|e| (State::Error, e))?;
                self.set_state(id, State::Starting);
                self.start_all(config, p).await
            }
        }
    }

    /// The engine's start, then the first start: the home packages it lacks and the first-start script.
    async fn start_all(&self, sb: &Sandbox<'_>, p: Progress) -> Result<State, (State, String)> {
        self.engine
            .start(sb, p.clone())
            .await
            .map_err(|e| (State::Error, e))?;
        if let Err(e) = self.first_start(sb, &p).await {
            let _ = self.engine.stop(&sb.config.id, p).await;
            return Err((State::Error, e));
        }
        Ok(State::Running)
    }

    async fn first_start(&self, sb: &Sandbox<'_>, p: &Progress) -> Result<(), String> {
        let c = sb.config;
        let id = c.id.as_str();
        let stop = c.setup.on_failure == OnFailure::Stop;
        let failed = |what: String, out: &str| {
            for line in tail(out) {
                p.log(format!("  {line}"));
            }
            if stop {
                Err(what)
            } else {
                p.log(format!(
                    "Warning: {what}; going on (setup.on_failure is \"continue\")"
                ));
                Ok(())
            }
        };
        let home = packages::of_scope(c, Scope::Home);
        if !home.is_empty() {
            let manager = manager_of(sb.folder);
            let sources = packages::sources(c, manager);
            let installed = self.installed(id, c, manager, Some(Scope::Home)).await;
            let mut set_up: Vec<&str> = Vec::new();
            for pkg in home.into_iter().filter(|pk| {
                !installed
                    .iter()
                    .any(|(s, n, _)| *s == pk.source && *n == pk.name)
            }) {
                let source = sources
                    .iter()
                    .find(|s| s.id == pkg.source)
                    .ok_or(format!("no source {:?}", pkg.source))?;
                if !set_up.contains(&source.id.as_str()) {
                    set_up.push(&source.id);
                    if !source.setup.trim().is_empty() {
                        p.log(format!("Setting up {}", source.name));
                        let (code, out) = self
                            .engine
                            .run(
                                id,
                                vec!["sh".into(), "-c".into(), source.setup.clone()],
                                false,
                            )
                            .await?;
                        if code != 0 {
                            failed(
                                format!("the setup of {} failed (exit {code})", source.name),
                                &out,
                            )?;
                        }
                    }
                }
                p.log(format!("Installing {} ({})", pkg.name, pkg.source));
                let cmd = packages::command(&source.install, &pkg.name, pkg.version.as_deref());
                let (code, out) = self.engine.run(id, cmd, false).await?;
                if code != 0 {
                    failed(
                        format!(
                            "{} package {} did not install (exit {code})",
                            pkg.source, pkg.name
                        ),
                        &out,
                    )?;
                }
            }
        }
        // A disposable home starts empty each time, so its script runs each time.
        let marker = sb.folder.join("state/first-start-done");
        if !c.setup.first_start.trim().is_empty() && (c.home.disposable || !marker.exists()) {
            p.log("Running the first-start script");
            let cmd = vec!["sh".into(), "-ec".into(), c.setup.first_start.clone()];
            let (code, out) = self.engine.run(id, cmd, false).await?;
            if code != 0 {
                failed(format!("the first-start script failed (exit {code})"), &out)?;
            }
            // Only once a script ran: one added later still runs at the next start.
            let _ = fs::write(&marker, "");
        }
        Ok(())
    }

    /// (source, name, version) of what the sources used by the wanted packages list in the running sandbox.
    async fn installed(
        &self,
        id: &str,
        c: &Config,
        manager: Option<Manager>,
        scope: Option<Scope>,
    ) -> Vec<(String, String, String)> {
        let mut used: Vec<&str> = Vec::new();
        for pk in packages::wanted(c) {
            if scope.is_none_or(|s| packages::scope(c, &pk.source) == Some(s))
                && !used.contains(&pk.source.as_str())
            {
                used.push(&pk.source);
            }
        }
        let mut out = Vec::new();
        for source in packages::sources(c, manager)
            .into_iter()
            .filter(|s| used.contains(&s.id.as_str()))
        {
            let cmd = vec!["sh".into(), "-c".into(), source.list.clone()];
            if let Ok((0, text)) = self.engine.run(id, cmd, false).await {
                out.extend(
                    packages::parse_list(&text)
                        .into_iter()
                        .map(|(n, v)| (source.id.clone(), n, v)),
                );
            }
        }
        out
    }

    async fn packages(&self, id: &str) -> Reply {
        let (c, folder, state) =
            self.with(id, |e| Ok((e.config.clone(), e.folder.clone(), e.state)))?;
        let installed = if state == State::Running {
            self.installed(id, &c, manager_of(&folder), None).await
        } else {
            Vec::new()
        };
        ok(packages::infos(&c, &installed))
    }

    /// Runs a home package command of `source` in the running sandbox, its setup first for an install.
    async fn package_run(
        &self,
        id: &str,
        c: &Config,
        folder: &Path,
        pkg: &Package,
        op: &str,
    ) -> Result<(), Error> {
        let sources = packages::sources(c, manager_of(folder));
        let source = sources.iter().find(|s| s.id == pkg.source).ok_or_else(|| {
            fail(
                ErrorCode::InvalidConfig,
                format!("no source {:?}", pkg.source),
            )
        })?;
        let mut steps = Vec::new();
        if op == "install" && !source.setup.trim().is_empty() {
            steps.push(vec!["sh".to_owned(), "-c".to_owned(), source.setup.clone()]);
        }
        let template = match op {
            "install" => &source.install,
            "remove" => &source.remove,
            _ => &source.update,
        };
        steps.push(packages::command(
            template,
            &pkg.name,
            pkg.version.as_deref(),
        ));
        for cmd in steps {
            let (code, out) = self
                .engine
                .run(id, cmd, false)
                .await
                .map_err(internal_msg)?;
            if code != 0 {
                let detail = tail(&out).join("\n");
                return Err(fail(
                    ErrorCode::Internal,
                    format!(
                        "{} {op} of {} failed (exit {code}):\n{detail}",
                        pkg.source, pkg.name
                    ),
                ));
            }
        }
        self.log(
            id,
            &format!("{} package {}: {op} done", pkg.source, pkg.name),
        );
        Ok(())
    }

    async fn package_add(&self, id: &str, pkg: Package) -> Reply {
        let (mut c, folder, state) =
            self.with(id, |e| Ok((e.config.clone(), e.folder.clone(), e.state)))?;
        if c.packages
            .iter()
            .any(|p| p.source == pkg.source && p.name == pkg.name)
        {
            return Err(fail(
                ErrorCode::AlreadyExists,
                format!("{id} already has the {} package {}", pkg.source, pkg.name),
            ));
        }
        c.packages.push(pkg.clone());
        validate::config(&c, Kind::Sandbox).map_err(|e| {
            fail(
                ErrorCode::InvalidConfig,
                format!("the configuration is not valid:\n{e}"),
            )
        })?;
        let now = state == State::Running
            && packages::scope(&c, &pkg.source) == Some(Scope::Home)
            && packages::wanted(&c).any(|p| *p == pkg);
        if now {
            self.package_run(id, &c, &folder, &pkg, "install").await?;
        }
        self.save(id, c)
    }

    async fn package_remove(&self, id: &str, source: &str, name: &str) -> Reply {
        let (mut c, folder, state) =
            self.with(id, |e| Ok((e.config.clone(), e.folder.clone(), e.state)))?;
        let pkg = c
            .packages
            .iter()
            .find(|p| p.source == source && p.name == name)
            .cloned()
            .ok_or_else(|| {
                fail(
                    ErrorCode::NotFound,
                    format!("{id} has no {source} package {name}"),
                )
            })?;
        if state == State::Running && packages::scope(&c, source) == Some(Scope::Home) {
            self.package_run(id, &c, &folder, &pkg, "remove").await?;
        }
        c.packages
            .retain(|p| !(p.source == source && p.name == name));
        self.save(id, c)
    }

    async fn package_update(
        &self,
        id: &str,
        source: Option<String>,
        name: Option<String>,
    ) -> Reply {
        let c = self.running(id)?;
        let folder = self.with(id, |e| Ok(e.folder.clone()))?;
        let chosen: Vec<Package> = packages::of_scope(&c, Scope::Home)
            .into_iter()
            .filter(|p| {
                source.as_ref().is_none_or(|s| *s == p.source)
                    && name.as_ref().is_none_or(|n| *n == p.name)
            })
            .cloned()
            .collect();
        if chosen.is_empty() {
            return Err(fail(
                ErrorCode::NotFound,
                "no home package matches: image packages are updated by a fresh rebuild",
            ));
        }
        for pkg in &chosen {
            self.package_run(id, &c, &folder, pkg, "update").await?;
        }
        Ok(Value::Null)
    }

    fn finish(&self, id: &str, job: Job, result: Result<State, (State, String)>) {
        let error = result.as_ref().err().map(|(_, m)| m.clone());
        {
            let mut inner = self.lock();
            let Some(e) = inner.sandboxes.get_mut(id) else {
                return;
            };
            match &result {
                Ok(state) => {
                    e.state = *state;
                    e.error = None;
                    if *state != State::Running {
                        e.services.clear();
                        e.service_warnings.clear();
                    }
                    if matches!(job, Job::Build | Job::Rebuild { .. }) {
                        e.flags.rebuild_required = false;
                    }
                    if matches!(*state, State::Stopped | State::Running)
                        && !matches!(job, Job::Build)
                    {
                        e.flags.restart_required = false;
                    }
                }
                Err((state, message)) => {
                    e.state = *state;
                    e.error = Some(message.clone());
                }
            }
            e.task = None;
            self.emit(Event::Task {
                id: id.to_owned(),
                task: None,
            });
            self.changed(&inner, id);
        }
        let label = job.label();
        match &error {
            None => self.log(id, &format!("{} finished", capitalize(label))),
            Some(m) => self.log(id, &format!("{} failed: {m}", capitalize(label))),
        }
        self.emit(Event::TaskEnded {
            id: id.to_owned(),
            label: label.to_owned(),
            error,
        });
    }

    async fn delete(&self, id: &str, keep_home: bool) -> Reply {
        {
            let mut inner = self.lock();
            let e = inner.sandboxes.get_mut(id).ok_or_else(|| not_found(id))?;
            if let Some(t) = &e.task {
                return Err(fail(
                    ErrorCode::InvalidState,
                    format!("{id} is busy: {} in progress", t.label),
                ));
            }
            state::check(e.state, Action::Delete).map_err(|m| fail(ErrorCode::InvalidState, m))?;
            // Blocks other actions while the engine removes the sandbox.
            e.task = Some(Task {
                label: "delete".into(),
                step: 1,
                steps: 1,
                step_label: "delete".into(),
                percent: 0,
            });
        }
        if let Err(m) = self.engine.remove(id).await {
            let mut inner = self.lock();
            if let Some(e) = inner.sandboxes.get_mut(id) {
                e.task = None;
            }
            return Err(fail(ErrorCode::Internal, m));
        }
        let mut inner = self.lock();
        let entry = inner
            .sandboxes
            .remove(id)
            .expect("entry is kept while deleting");
        inner.registry.sandboxes.retain(|r| r.id != id);
        store::save_registry(&self.paths, &inner.registry).map_err(internal)?;
        drop(inner);
        // Only a folder that is a sandbox folder is removed.
        if !keep_home && store::config_file(&entry.folder).exists() {
            fs::remove_dir_all(&entry.folder).map_err(|e| {
                fail(
                    ErrorCode::Internal,
                    format!(
                        "the sandbox is deleted but its folder {} could not be removed ({e}); files made by an \
                         isolated user need `podman unshare rm -rf {}`",
                        entry.folder.display(),
                        entry.folder.display()
                    ),
                )
            })?;
            let _ = fs::remove_dir_all(self.paths.log(id).parent().expect("log folder"));
        }
        if let Err(e) = menu::remove_all(&self.paths, id, true) {
            eprintln!("cardea daemon: menu entries of {id}: {e}");
        }
        self.emit(Event::Removed { id: id.to_owned() });
        Ok(Value::Null)
    }

    fn set_config(&self, id: &str, text: &str) -> Reply {
        let new = load::config(text, Kind::Sandbox).map_err(|e| {
            fail(
                ErrorCode::InvalidConfig,
                format!("the configuration is not valid:\n{e}"),
            )
        })?;
        self.save(id, new)
    }

    fn set_folders(&self, id: &str, folders: Vec<Folder>, protected: Option<Vec<String>>) -> Reply {
        let mut new = self.with(id, |e| Ok(e.config.clone()))?;
        new.folders = folders;
        if let Some(names) = protected {
            new.protected_paths.names = names;
        }
        validate::config(&new, Kind::Sandbox).map_err(|e| {
            fail(
                ErrorCode::InvalidConfig,
                format!("the configuration is not valid:\n{e}"),
            )
        })?;
        self.save(id, new)
    }

    fn set_firewall(&self, id: &str, firewall: Firewall) -> Reply {
        let mut new = self.with(id, |e| Ok(e.config.clone()))?;
        new.firewall = firewall;
        validate::config(&new, Kind::Sandbox).map_err(|e| {
            fail(
                ErrorCode::InvalidConfig,
                format!("the configuration is not valid:\n{e}"),
            )
        })?;
        self.save(id, new)
    }

    /// Saves a validated configuration and says what it needs to apply.
    fn save(&self, id: &str, new: Config) -> Reply {
        let mut inner = self.lock();
        let e = inner.sandboxes.get(id).ok_or_else(|| not_found(id))?;
        if new.id != id {
            return Err(fail(
                ErrorCode::InvalidConfig,
                format!("  - id: the id cannot be changed (it is {id:?})"),
            ));
        }
        if let Some(t) = &e.task {
            return Err(fail(
                ErrorCode::InvalidState,
                format!("{id} is busy: {} in progress", t.label),
            ));
        }
        let home = home_of(&self.paths, &new, &e.folder);
        if let Some(other) = inner
            .sandboxes
            .values()
            .find(|o| o.config.id != id && home_of(&self.paths, &o.config, &o.folder) == home)
        {
            return Err(fail(
                ErrorCode::InvalidConfig,
                format!(
                    "  - home.path: {} is already the home of sandbox {:?}",
                    self.paths.display(&home),
                    other.config.id
                ),
            ));
        }
        let e = inner.sandboxes.get_mut(id).expect("checked above");
        let change = impact(&e.config, &new);
        store::save_config(&e.folder, &new).map_err(internal)?;
        // Before answering, so a removed app's entry is gone when the client hears back; icons come later.
        if let Err(err) = menu::sync(&self.paths, &new, true) {
            eprintln!("cardea daemon: menu entries of {id}: {err}");
        }
        let saved = match change {
            Impact::None => Saved::Applied,
            Impact::Restart if e.state == State::Running => {
                e.flags.restart_required = true;
                Saved::RestartRequired
            }
            Impact::Restart => Saved::Applied,
            Impact::Rebuild => {
                e.flags.rebuild_required = e.state != State::BuildFailed;
                Saved::RebuildRequired
            }
        };
        e.config = new;
        self.changed(&inner, id);
        drop(inner);
        let what = match saved {
            Saved::Applied => "applied",
            Saved::RestartRequired => "restart required",
            Saved::RebuildRequired => "rebuild required",
        };
        self.log(id, &format!("Configuration changed ({what})"));
        ok(saved)
    }
}

impl Entry {
    fn new(config: Config, folder: PathBuf, state: State) -> Entry {
        Entry {
            config,
            folder,
            state,
            flags: Flags::default(),
            error: None,
            task: None,
            net: None,
            password: None,
            services: Vec::new(),
            service_warnings: Vec::new(),
        }
    }
}

fn not_found(id: &str) -> Error {
    fail(ErrorCode::NotFound, format!("no sandbox {id:?}"))
}

fn home_of(paths: &Paths, config: &Config, folder: &Path) -> PathBuf {
    config
        .home
        .path
        .as_deref()
        .map_or_else(|| folder.join("home"), |p| paths.expand(p))
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// A sandbox id made from a display name: "My Box!" → "my-box".
pub fn slug(name: &str) -> String {
    let mut id = String::new();
    for c in name.trim().chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            id.push(c);
        } else if !id.ends_with('-') && !id.is_empty() {
            id.push('-');
        }
    }
    id.truncate(32);
    let id = id.trim_end_matches('-');
    if id.is_empty() {
        "sandbox".to_owned()
    } else {
        id.to_owned()
    }
}

/// The package manager the last build found (`state/package-manager`).
fn manager_of(folder: &Path) -> Option<Manager> {
    fs::read_to_string(folder.join("state/package-manager"))
        .ok()
        .and_then(|m| packages::manager_from(&m))
}

/// The last lines of a command's output, for the log and error messages.
fn tail(out: &str) -> Vec<&str> {
    let lines: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(15)..].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slug("My Box!"), "my-box");
        assert_eq!(slug("  Été 2026  "), "t-2026");
        assert_eq!(slug("***"), "sandbox");
        assert!(validate::id(&slug(&"x".repeat(50))).is_ok());
    }
}
