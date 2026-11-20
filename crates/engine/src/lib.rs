//! Cardea engines: what actually builds, starts and stops sandboxes. The daemon owns states and tasks; an
//! engine only does the work and reports progress.
//!
//! - [`demo::DemoEngine`]: simulated sandboxes, for the GUI and the CLI without Podman.
//! - [`Unavailable`]: placeholder until the Podman engine exists.

pub mod demo;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use cardea_core::model::Config;

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

/// Builds, starts, stops and removes sandboxes. Errors are messages for the user.
pub trait Engine: Send + Sync {
    /// Builds (or rebuilds) the sandbox's image.
    fn build<'a>(
        &'a self,
        config: &'a Config,
        progress: Progress,
    ) -> BoxFuture<'a, Result<(), String>>;
    /// Starts the sandbox; returns once it runs.
    fn start<'a>(
        &'a self,
        config: &'a Config,
        progress: Progress,
    ) -> BoxFuture<'a, Result<(), String>>;
    fn stop<'a>(&'a self, id: &'a str, progress: Progress) -> BoxFuture<'a, Result<(), String>>;
    /// Removes the sandbox's image and containers.
    fn remove<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<(), String>>;
}

/// The engine used until the Podman engine exists: every operation fails with a clear message.
pub struct Unavailable;

const UNAVAILABLE: &str = "the Podman engine is not available in this version yet; run `cardea daemon --demo` to try \
                           Cardea with simulated sandboxes";

impl Engine for Unavailable {
    fn build<'a>(&'a self, _: &'a Config, _: Progress) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Err(UNAVAILABLE.to_owned()) })
    }
    fn start<'a>(&'a self, _: &'a Config, _: Progress) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Err(UNAVAILABLE.to_owned()) })
    }
    fn stop<'a>(&'a self, _: &'a str, _: Progress) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Err(UNAVAILABLE.to_owned()) })
    }
    fn remove<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<(), String>> {
        // Nothing was ever created by this engine.
        Box::pin(async { Ok(()) })
    }
}
