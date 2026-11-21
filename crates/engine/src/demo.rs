//! The demo engine: simulated builds, starts and stops with realistic steps and timings, and the seven
//! sandboxes of the mock-ups.

use std::time::Duration;

use cardea_core::builtin;
use cardea_core::model::{Config, Folder, Icon, Mode, Trust};
use cardea_core::state::{Flags, State};

use crate::{BoxFuture, Engine, Progress};

/// Package names starting with this make a demo build fail, to try the Build failed state.
pub const FAIL_PACKAGE_PREFIX: &str = "demo-fail";

pub struct DemoEngine {
    /// Duration of one simulated step.
    step: Duration,
}

impl DemoEngine {
    pub fn new(step: Duration) -> DemoEngine {
        DemoEngine { step }
    }

    /// Runs `steps`, each in `ticks` progress ticks, reporting percent over the whole task.
    async fn run(&self, progress: &Progress, steps: &[&str], ticks: u32) {
        let total = steps.len() as u32 * ticks;
        for (i, label) in steps.iter().enumerate() {
            let n = i as u32 + 1;
            progress.step(n, steps.len() as u32, label);
            progress.log(format!("Step {n}/{} · {label}", steps.len()));
            for t in 1..=ticks {
                tokio::time::sleep(self.step / ticks).await;
                progress.percent(((i as u32 * ticks + t) * 100 / total) as u8);
            }
        }
    }
}

const BUILD_STEPS: [&str; 6] = [
    "base image",
    "apt repositories",
    "system packages",
    "other packages",
    "user and helpers",
    "setup script",
];

impl Engine for DemoEngine {
    fn build<'a>(
        &'a self,
        config: &'a Config,
        progress: Progress,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            progress.log(format!(
                "Building localhost/cardea/{} from {}",
                config.id, config.base_image
            ));
            if let Some(bad) = config
                .packages
                .iter()
                .find(|p| p.name.starts_with(FAIL_PACKAGE_PREFIX))
            {
                self.run(&progress, &BUILD_STEPS[..3], 4).await;
                let message = format!("package {:?} not found", bad.name);
                progress.log(format!("E: {message}"));
                return Err(message);
            }
            for p in &config.packages {
                progress.log(format!("  {} package {}", p.source, p.name));
            }
            self.run(&progress, &BUILD_STEPS, 4).await;
            Ok(())
        })
    }

    fn start<'a>(
        &'a self,
        config: &'a Config,
        progress: Progress,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let steps = ["firewall rules", "app container", "services"];
            self.run(&progress, &steps, 3).await;
            for f in &config.folders {
                progress.log(format!(
                    "Mounted {} on {} ({})",
                    f.host,
                    f.mount,
                    format!("{:?}", f.mode).to_lowercase()
                ));
            }
            Ok(())
        })
    }

    fn stop<'a>(&'a self, _: &'a str, progress: Progress) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.run(&progress, &["stopping containers"], 3).await;
            Ok(())
        })
    }

    fn remove<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
}

/// Simulated CPU (percent of one core) and memory (bytes) of a running demo sandbox at `tick` seconds: a
/// calm baseline with a burst now and then, different for each sandbox.
pub fn stats(id: &str, tick: u64) -> (f32, u64) {
    let seed = id
        .bytes()
        .fold(7u64, |h, b| h.wrapping_mul(31).wrapping_add(u64::from(b)));
    // A cheap hash of (seed, tick) in 0..1.
    let noise = |t: u64| {
        let x = (seed ^ t.wrapping_mul(0x9E37_79B9_7F4A_7C15)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        (x >> 40) as f32 / (1u64 << 24) as f32
    };
    let burst = if (tick + seed) % 20 < 3 { 30.0 } else { 0.0 };
    let cpu = 2.0 + 6.0 * noise(tick) + burst;
    let base = 600 + seed % 900;
    let memory = (base + (tick % 120) * 3 + (noise(tick / 5) * 80.0) as u64) << 20;
    (cpu, memory)
}

/// A task the daemon resumes when it seeds a demo sandbox in a transient state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resume {
    Build,
    Start,
    Stop,
}

/// A sandbox of the mock-ups, with its initial state.
pub struct Seed {
    pub config: Config,
    pub state: State,
    pub flags: Flags,
    pub error: Option<String>,
    pub resume: Option<Resume>,
}

/// The seven sandboxes of the mock-ups.
pub fn seeds() -> Vec<Seed> {
    let make = |profile: &str, id: &str, name: &str, color: &str, trust: Trust| {
        let mut c = builtin::profile(profile).expect("built-in profile");
        c.id = id.into();
        c.name = name.into();
        c.description = None;
        c.profile = Some(profile.into());
        c.icon = Some(Icon {
            letter: name[..1].into(),
            color: color.into(),
        });
        c.apply_trust(trust);
        c
    };
    let seed = |config, state, resume| Seed {
        config,
        state,
        flags: Flags::default(),
        error: None,
        resume,
    };

    let mut claude = make("claude", "claude", "Claude", "#c8643f", Trust::Work);
    claude.folders = vec![
        Folder {
            host: "/home/me/Projects/site".into(),
            mount: "/work/site".into(),
            mode: Mode::Rw,
        },
        Folder {
            host: "/home/me/Notes".into(),
            mount: "/work/notes".into(),
            mode: Mode::Ro,
        },
    ];
    let mut scratch = make("empty", "scratch", "Scratch", "#8a6d3b", Trust::Untrusted);
    scratch.firewall.enabled = false;
    let mut test = seed(
        make("empty", "test-app", "Test-app", "#6b7280", Trust::Untrusted),
        State::Stopped,
        None,
    );
    test.flags.rebuild_required = true;
    let mut browser = seed(
        make("empty", "browser", "Browser", "#7c4dba", Trust::Untrusted),
        State::Error,
        None,
    );
    browser.error =
        Some("the firewall rules could not be applied: the netguard sidecar did not answer".into());

    vec![
        seed(claude, State::Running, None),
        seed(
            make("empty", "build-env", "Build-env", "#2f8f7a", Trust::Work),
            State::Building,
            Some(Resume::Build),
        ),
        seed(
            make("empty", "docs", "Docs", "#2563c9", Trust::Work),
            State::Starting,
            Some(Resume::Start),
        ),
        test,
        seed(scratch, State::Stopping, Some(Resume::Stop)),
        browser,
        seed(
            make("empty", "private", "Private", "#3f3d56", Trust::Work),
            State::Running,
            None,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Report;
    use cardea_core::model::{Kind, Package};
    use std::sync::{Arc, Mutex};

    #[test]
    fn seeds_are_valid_sandboxes() {
        let seeds = seeds();
        assert_eq!(seeds.len(), 7);
        for s in &seeds {
            cardea_core::validate::config(&s.config, Kind::Sandbox).unwrap();
        }
    }

    #[test]
    fn stats_stay_in_range() {
        for t in 0..200 {
            let (cpu, mem) = stats("claude", t);
            assert!((2.0..=38.0).contains(&cpu));
            assert!(mem > 500 << 20 && mem < 2 << 30);
        }
    }

    #[tokio::test]
    async fn build_reports_steps_and_can_fail() {
        let engine = DemoEngine::new(Duration::from_millis(1));
        let reports = Arc::new(Mutex::new(Vec::new()));
        let r = reports.clone();
        let progress = Progress::new(move |rep| r.lock().unwrap().push(rep));
        let mut config = seeds().remove(0).config;
        engine.build(&config, progress.clone()).await.unwrap();
        let steps = reports
            .lock()
            .unwrap()
            .iter()
            .filter(|r| matches!(r, Report::Step { .. }))
            .count();
        assert_eq!(steps, 6);
        assert!(reports.lock().unwrap().contains(&Report::Percent(100)));

        config.packages.push(Package {
            source: "system".into(),
            name: "demo-fail-x".into(),
            version: None,
            when: None,
        });
        assert_eq!(
            engine.build(&config, progress).await.unwrap_err(),
            "package \"demo-fail-x\" not found"
        );
    }
}
