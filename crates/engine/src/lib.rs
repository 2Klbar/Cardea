//! Cardea engine: what actually builds, starts and stops sandboxes. The daemon owns states and tasks; an
//! engine only does the work and reports progress.
//!
//! - [`podman_engine::PodmanEngine`]: real sandboxes, on rootless Podman.
//! - [`podman::Client`]: the libpod REST client it is built on.

pub mod folders;
pub mod net;
pub mod podman;
pub mod podman_engine;

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use cardea_core::apps::{Detected, Launch};
use cardea_core::firewall::NetStatus;
use cardea_core::model::Config;
use cardea_core::services::Instance;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What an engine reports while it works.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Report {
    /// A new step starts: "Step 3/6 · other packages".
    Step {
        step: u32,
        steps: u32,
        label: String,
    },
    /// Progress of the whole task.
    Percent(u8),
    /// A line for the sandbox log.
    Log(String),
}

/// Where an engine sends its [`Report`]s.
#[derive(Clone)]
pub struct Progress(Arc<dyn Fn(Report) + Send + Sync>);

impl Progress {
    pub fn new(f: impl Fn(Report) + Send + Sync + 'static) -> Progress {
        Progress(Arc::new(f))
    }

    pub fn step(&self, step: u32, steps: u32, label: &str) {
        (self.0)(Report::Step {
            step,
            steps,
            label: label.to_owned(),
        });
    }

    pub fn percent(&self, p: u8) {
        (self.0)(Report::Percent(p.min(100)));
    }

    pub fn log(&self, line: impl Into<String>) {
        (self.0)(Report::Log(line.into()));
    }
}

/// An icon file read from a sandbox: its path there and its bytes.
pub type IconFile = (String, Vec<u8>);

const UNSUPPORTED: &str = "this engine cannot run commands in a sandbox";

/// A sandbox as the daemon hands it to the engine.
pub struct Sandbox<'a> {
    pub config: &'a Config,
    /// The sandbox folder (`sandbox.json`, `home/`, `state/`).
    pub folder: &'a Path,
    /// The persistent home folder, resolved from `home.path`.
    pub home: &'a Path,
    /// The keyring password (`keyring.mode: ask`), only for a start; never stored.
    pub password: Option<&'a str>,
    /// Build only: pull the base image again and use no cached layer.
    pub fresh: bool,
}

/// What an engine sees happening to running sandboxes.
#[derive(Clone, Debug, PartialEq)]
pub enum Watch {
    Stats {
        id: String,
        cpu: f32,
        memory: u64,
    },
    /// The sandbox's app container, or its firewall sidecar, stopped.
    Exited {
        id: String,
        code: Option<i64>,
        sidecar: bool,
    },
}

/// A command to run in a sandbox.
#[derive(Clone, Debug, Default)]
pub struct Exec {
    /// `None`: the user's login shell.
    pub cmd: Option<Vec<String>>,
    pub root: bool,
    pub workdir: Option<String>,
    pub tty: bool,
    pub rows: u16,
    pub cols: u16,
    /// `TERM` of the caller's terminal.
    pub term: Option<String>,
    /// `NAME=value`, added to the container's environment.
    pub env: Vec<String>,
}

/// A started exec: its id and the connection carrying its input and output.
pub struct Started {
    pub exec: String,
    pub stream: std::os::unix::net::UnixStream,
}

/// Builds, starts, stops and removes sandboxes. Errors are messages for the user.
pub trait Engine: Send + Sync {
    /// Builds (or rebuilds) the sandbox's image.
    fn build<'a>(
        &'a self,
        sandbox: &'a Sandbox<'a>,
        progress: Progress,
    ) -> BoxFuture<'a, Result<(), String>>;
    /// Starts the sandbox; returns once it runs.
    fn start<'a>(
        &'a self,
        sandbox: &'a Sandbox<'a>,
        progress: Progress,
    ) -> BoxFuture<'a, Result<(), String>>;
    fn stop<'a>(&'a self, id: &'a str, progress: Progress) -> BoxFuture<'a, Result<(), String>>;
    /// Removes the sandbox's image and containers.
    fn remove<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<(), String>>;
    /// Why this engine cannot run sandboxes, if it cannot.
    fn problem(&self) -> Option<String> {
        None
    }
    /// The ids of the sandboxes running now (read when the daemon starts).
    fn running(&self) -> BoxFuture<'_, Result<Vec<String>, String>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    /// Reports what happens to running sandboxes until the engine's stream ends.
    fn watch(&self, sink: Arc<dyn Fn(Watch) + Send + Sync>) -> BoxFuture<'_, Result<(), String>> {
        drop(sink);
        Box::pin(std::future::pending())
    }
    /// Brings a running sandbox's firewall up to date (references resolved again, configuration applied when the
    /// ruleset changed) and reads its counters. `None`: this engine has no firewall.
    fn firewall<'a>(
        &'a self,
        sandbox: &'a Sandbox<'a>,
    ) -> BoxFuture<'a, Result<Option<NetStatus>, String>> {
        let _ = sandbox;
        Box::pin(async { Ok(None) })
    }
    /// Starts an app in the background, as the sandbox user.
    fn launch<'a>(&'a self, id: &'a str, launch: Launch) -> BoxFuture<'a, Result<(), String>> {
        let _ = (id, launch);
        Box::pin(async { Err(UNSUPPORTED.to_owned()) })
    }
    /// The apps of the sandbox's `.desktop` files.
    fn detect_apps<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Vec<Detected>, String>> {
        let _ = id;
        Box::pin(async { Ok(Vec::new()) })
    }
    /// An icon file of the sandbox (name from a `.desktop` file, or absolute path), at most 1 MiB: its path and bytes.
    fn icon<'a>(
        &'a self,
        id: &'a str,
        icon: &'a str,
    ) -> BoxFuture<'a, Result<Option<IconFile>, String>> {
        let _ = (id, icon);
        Box::pin(async { Ok(None) })
    }
    /// The service instances of a running sandbox, with their state.
    fn services<'a>(&'a self, config: &'a Config) -> BoxFuture<'a, Result<Vec<Instance>, String>> {
        let _ = config;
        Box::pin(async { Ok(Vec::new()) })
    }
    /// Starts (again) or stops a service instance.
    fn service<'a>(
        &'a self,
        config: &'a Config,
        service: &'a str,
        folder: Option<&'a str>,
        start: bool,
    ) -> BoxFuture<'a, Result<(), String>> {
        let _ = (config, service, folder, start);
        Box::pin(async { Err(UNSUPPORTED.to_owned()) })
    }
    /// Runs a command in a running sandbox, as the sandbox user or as root, and collects its output (stdout and
    /// stderr together): (exit code, output).
    fn run<'a>(
        &'a self,
        id: &'a str,
        argv: Vec<String>,
        root: bool,
    ) -> BoxFuture<'a, Result<(i32, String), String>> {
        let _ = (id, argv, root);
        Box::pin(async { Err(UNSUPPORTED.to_owned()) })
    }
    fn exec<'a>(&'a self, id: &'a str, exec: Exec) -> BoxFuture<'a, Result<Started, String>> {
        let _ = (id, exec);
        Box::pin(async { Err(UNSUPPORTED.to_owned()) })
    }
    /// Waits for an exec to end; returns its exit code.
    fn exec_wait<'a>(&'a self, exec: &'a str) -> BoxFuture<'a, Result<i32, String>> {
        let _ = exec;
        Box::pin(async { Err(UNSUPPORTED.to_owned()) })
    }
    fn exec_resize<'a>(
        &'a self,
        exec: &'a str,
        rows: u16,
        cols: u16,
    ) -> BoxFuture<'a, Result<(), String>> {
        let _ = (exec, rows, cols);
        Box::pin(async { Ok(()) })
    }
}
