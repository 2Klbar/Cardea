//! The daemon's state: the sandboxes, their lifecycle and tasks, and the handling of every request.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use cardea_core::impact::{Impact, impact};
use cardea_core::model::{Config, Kind};
use cardea_core::state::{self, Action, Flags, State};
use cardea_core::{builtin, load, risk, validate};
use cardea_engine::demo::{self, Resume};
use cardea_engine::{Engine, Progress, Report};
use cardea_proto::{Error, ErrorCode, Event, Method, ProfileInfo, Saved, Status, Summary, Task};
use serde_json::Value;
use tokio::sync::broadcast;

use crate::paths::Paths;
use crate::store::{self, Registered, Registry};

/// How many log lines `logs` returns before following.
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
    Rebuild { was_running: bool },
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
    demo: bool,
    engine: Arc<dyn Engine>,
    inner: Mutex<Inner>,
    events: broadcast::Sender<Event>,
    /// Held while a log line is written and sent, so `logs --follow` sees no gap and no duplicate.
    log_lock: Mutex<()>,
}

impl Daemon {
    /// Loads the registered sandboxes. In demo mode, starts from a fresh demo folder seeded with the mock-up
    /// sandboxes and resumes their tasks (needs a Tokio runtime).
    pub fn open(paths: Paths, engine: Arc<dyn Engine>, demo: bool) -> Result<Arc<Daemon>, String> {
        if demo {
            let root = paths.runtime.join("demo");
            if root.exists() {
                fs::remove_dir_all(&root)
                    .map_err(|e| format!("cannot reset {}: {e}", root.display()))?;
            }
        }
        let registry = store::load_registry(&paths)?;
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
            demo,
            engine,
            inner: Mutex::new(Inner {
                sandboxes,
                registry,
            }),
            events,
            log_lock: Mutex::new(()),
        });
        if demo {
            daemon.seed()?;
        }
        Ok(daemon)
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    pub fn demo(&self) -> bool {
        self.demo
    }

    fn seed(self: &Arc<Self>) -> Result<(), String> {
        let mut resumes = Vec::new();
        for s in demo::seeds() {
            let id = s.config.id.clone();
            let folder = self.paths.sandboxes.join(&id);
            self.register(&s.config, &folder).map_err(|e| e.message)?;
            let mut inner = self.lock();
            let entry = inner.sandboxes.get_mut(&id).expect("just registered");
            entry.flags = s.flags;
            entry.error = s.error;
            match s.resume {
                // The task sets the transient state itself.
                Some(r) => {
                    entry.state = match r {
                        Resume::Build => State::Building,
                        Resume::Start => State::Stopped,
                        Resume::Stop => State::Running,
                    };
                    resumes.push((id, r));
                }
                None => entry.state = s.state,
            }
        }
        for (id, r) in resumes {
            let (job, action) = match r {
                Resume::Build => (Job::Build, None),
                Resume::Start => (Job::Start, Some(Action::Start)),
                Resume::Stop => (Job::Stop, Some(Action::Stop)),
            };
            match action {
                Some(a) => self.begin(&id, a, job).map_err(|e| e.message)?,
                None => self.spawn(id, job, true),
            }
        }
        Ok(())
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
            warnings: risk::warnings(&e.config),
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
            demo: self.demo,
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
            } => self.create(profile, name, config),
            Method::Start { id } => self
                .begin(&id, Action::Start, Job::Start)
                .map(|()| Value::Null),
            Method::Stop { id } => self
                .begin(&id, Action::Stop, Job::Stop)
                .map(|()| Value::Null),
            Method::Restart { id } => self
                .begin(&id, Action::Restart, Job::Restart)
                .map(|()| Value::Null),
            Method::Rebuild { id } => {
                let was_running = self
                    .with(&id, |e| Ok(e.state == State::Running))
                    .unwrap_or(false);
                self.begin(&id, Action::Rebuild, Job::Rebuild { was_running })
                    .map(|()| Value::Null)
            }
            Method::Delete { id, keep_home } => self.delete(&id, keep_home).await,
            Method::GetConfig { id } => self.with(&id, |e| {
                fs::read_to_string(store::config_file(&e.folder))
                    .map(Value::String)
                    .map_err(internal)
            }),
            Method::SetConfig { id, config } => self.set_config(&id, &config),
        };
        (reply, None)
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
        let info = |p: &Config, builtin| ProfileInfo {
            id: p.id.clone(),
            name: p.name.clone(),
            description: p.description.clone(),
            builtin,
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
        let location = settings
            .sandbox_location
            .map_or_else(|| self.paths.sandboxes.clone(), |l| self.paths.expand(&l));
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
    fn begin(self: &Arc<Self>, id: &str, action: Action, job: Job) -> Result<(), Error> {
        {
            let inner = self.lock();
            let e = inner.sandboxes.get(id).ok_or_else(|| not_found(id))?;
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

    /// Runs `job` in the background. `seeded`: the state is already set (create, demo seeds).
    fn spawn(self: &Arc<Self>, id: String, job: Job, seeded: bool) {
        let first = match job {
            Job::Build => State::Building,
            Job::Start => State::Starting,
            Job::Stop | Job::Restart => State::Stopping,
            Job::Rebuild { .. } => State::Rebuilding,
        };
        let config = {
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
            let config = e.config.clone();
            self.changed(&inner, &id);
            config
        };
        self.log(&id, &format!("{} started", capitalize(job.label())));
        let daemon = self.clone();
        tokio::spawn(async move {
            let result = daemon.run(&id, job, &config).await;
            daemon.finish(&id, job, result);
        });
    }

    /// The work of a job; returns the final state, or the failure state and its message.
    async fn run(
        self: &Arc<Self>,
        id: &str,
        job: Job,
        config: &Config,
    ) -> Result<State, (State, String)> {
        let engine = &self.engine;
        let p = self.progress(id);
        match job {
            Job::Build => engine
                .build(config, p)
                .await
                .map(|()| State::Stopped)
                .map_err(|e| (State::BuildFailed, e)),
            Job::Start => engine
                .start(config, p)
                .await
                .map(|()| State::Running)
                .map_err(|e| (State::Error, e)),
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
                engine
                    .start(config, p)
                    .await
                    .map(|()| State::Running)
                    .map_err(|e| (State::Error, e))
            }
            Job::Rebuild { was_running } => {
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
                engine
                    .start(config, p)
                    .await
                    .map(|()| State::Running)
                    .map_err(|e| (State::Error, e))
            }
        }
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
            fs::remove_dir_all(&entry.folder).map_err(internal)?;
            let _ = fs::remove_dir_all(self.paths.log(id).parent().expect("log folder"));
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
