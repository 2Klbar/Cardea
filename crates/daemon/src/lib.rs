//! Cardea daemon: the only process that talks to the engine. It owns the sandboxes' files and states, runs
//! tasks and serves the control socket.

pub mod daemon;
pub mod paths;
pub mod server;
pub mod store;

use std::sync::Arc;
use std::time::Duration;

use cardea_engine::demo::DemoEngine;
use cardea_engine::{Engine, Unavailable};

/// `cardea daemon [--demo]`: runs until stopped.
pub fn main(demo: bool) -> Result<(), String> {
    let paths = paths::Paths::from_env()?;
    let paths = if demo { paths.demo() } else { paths };
    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let listener = server::bind(&paths.runtime)?;
        let engine: Arc<dyn Engine> = if demo {
            Arc::new(DemoEngine::new(Duration::from_millis(900)))
        } else {
            Arc::new(Unavailable)
        };
        let daemon = daemon::Daemon::open(paths.clone(), engine, demo)?;
        eprintln!(
            "cardea daemon {} listening on {}{}",
            cardea_core::VERSION,
            paths.socket().display(),
            if demo {
                " (demo engine, simulated sandboxes)"
            } else {
                ""
            }
        );
        server::run(daemon, listener).await
    })
}
