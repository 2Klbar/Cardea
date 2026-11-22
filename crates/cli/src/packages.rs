//! `cardea packages`, `cardea sources` and `cardea repo`.

use std::path::PathBuf;

use cardea_core::load;
use cardea_core::model::{AptRepo, Capability, Config, Package, When};
use cardea_core::packages::PackageInfo;
use cardea_proto::{Method, Saved, Status};
use clap::{Subcommand, ValueEnum};

use crate::client::Client;

#[derive(Clone, Copy, ValueEnum)]
pub enum CapabilityArg {
    Wayland,
    X11,
    Gpu,
    Kvm,
    Keyring,
}

#[derive(Subcommand)]
pub enum PackagesCommand {
    /// The packages, with the version installed in the running sandbox
    List,
    /// Add a package: a home package installs at once in a running sandbox, an image package at the next rebuild
    Add {
        /// `system`, `npm`, `cargo`, `rustup`, `mise`, `pipx` or a custom source
        source: String,
        name: String,
        #[arg(long)]
        version: Option<String>,
        /// Only installed while this capability is on
        #[arg(long)]
        when: Option<CapabilityArg>,
    },
    /// Remove a package (uninstalled at once from the home of a running sandbox)
    Remove { source: String, name: String },
    /// Update the home packages (all, one source's, or one); with --image, rebuild the image afresh instead
    Update {
        source: Option<String>,
        name: Option<String>,
        /// Update the image's packages: a rebuild without cache, the base image pulled again
        #[arg(long, conflicts_with = "source")]
        image: bool,
    },
}

#[derive(Subcommand)]
pub enum SourcesCommand {
    /// The sources a sandbox can use
    List,
    /// Add a custom source from a JSON file (`cardea check source <file>` checks it first)
    Add { file: PathBuf },
    /// Remove a custom source (its packages must be removed first)
    Remove { source: String },
}

#[derive(Subcommand)]
pub enum RepoCommand {
    /// The third-party apt repositories
    List,
    /// Add an apt repository; its signing key must have the given fingerprint at every download
    Add {
        name: String,
        /// `https://` address of the repository
        #[arg(long)]
        url: String,
        #[arg(long)]
        suite: String,
        #[arg(long, value_delimiter = ',', default_value = "main")]
        components: Vec<String>,
        /// `https://` address of the signing key
        #[arg(long)]
        key_url: String,
        /// The key's 40 hexadecimal characters
        #[arg(long)]
        fingerprint: String,
    },
    /// Remove an apt repository
    Remove { name: String },
}

fn save(c: &mut Client, id: &str, config: &Config) -> Result<(), String> {
    let text = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    let saved: Saved = crate::call(
        c,
        Method::SetConfig {
            id: id.to_owned(),
            config: text,
        },
    )?;
    out!("{}", crate::saved_text(saved));
    Ok(())
}

pub fn packages(id: String, command: PackagesCommand) -> Result<(), String> {
    let (_, mut c) = crate::connect()?;
    match command {
        PackagesCommand::List => {
            let list: Vec<PackageInfo> = crate::call(&mut c, Method::Packages { id })?;
            for p in list {
                let state = match (&p.installed, p.skipped) {
                    (_, true) => "skipped (capability off)".to_owned(),
                    (Some(v), _) => format!("installed {v}"),
                    (None, _) => "not installed".to_owned(),
                };
                let wanted = p.version.map(|v| format!(" ({v})")).unwrap_or_default();
                let place = if p.image { "image" } else { "home" };
                out!(
                    "{:<8} {:<28} {:<6} {state}",
                    p.source,
                    format!("{}{wanted}", p.name),
                    place
                );
            }
            Ok(())
        }
        PackagesCommand::Add {
            source,
            name,
            version,
            when,
        } => {
            let capability = when.map(|w| match w {
                CapabilityArg::Wayland => Capability::Wayland,
                CapabilityArg::X11 => Capability::X11,
                CapabilityArg::Gpu => Capability::Gpu,
                CapabilityArg::Kvm => Capability::Kvm,
                CapabilityArg::Keyring => Capability::Keyring,
            });
            let package = Package {
                source,
                name,
                version,
                when: capability.map(|capability| When { capability }),
            };
            let saved: Saved = crate::call(&mut c, Method::PackageAdd { id, package })?;
            out!("{}", crate::saved_text(saved));
            Ok(())
        }
        PackagesCommand::Remove { source, name } => {
            let saved: Saved = crate::call(&mut c, Method::PackageRemove { id, source, name })?;
            out!("{}", crate::saved_text(saved));
            Ok(())
        }
        PackagesCommand::Update { image: true, .. } => {
            drop(c);
            crate::rebuild(&id, true)
        }
        PackagesCommand::Update { source, name, .. } => {
            crate::call::<()>(&mut c, Method::PackageUpdate { id, source, name })?;
            out!("Updated.");
            Ok(())
        }
    }
}

pub fn sources(id: String, command: SourcesCommand) -> Result<(), String> {
    let (_, mut c) = crate::connect()?;
    let status: Status = crate::call(&mut c, Method::Status { id: id.clone() })?;
    let mut config = status.config;
    match command {
        SourcesCommand::List => {
            for s in cardea_core::packages::sources(&config, None) {
                let custom = if config.sources.iter().any(|x| x.id == s.id) {
                    "  custom"
                } else {
                    ""
                };
                out!("{:<8} {:<34} {:?}{custom}", s.id, s.name, s.scope);
            }
            out!(
                "{:<8} {:<34} Image",
                "system",
                "System packages (follows the base image)"
            );
            Ok(())
        }
        SourcesCommand::Add { file } => {
            let text = std::fs::read_to_string(&file)
                .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
            let source = load::source(&text)
                .map_err(|e| format!("{} is not a valid source:\n{e}", file.display()))?;
            config.sources.push(source);
            save(&mut c, &id, &config)
        }
        SourcesCommand::Remove { source } => {
            let before = config.sources.len();
            config.sources.retain(|s| s.id != source);
            if config.sources.len() == before {
                return Err(format!("{id} has no custom source {source:?}"));
            }
            save(&mut c, &id, &config)
        }
    }
}

pub fn repo(id: String, command: RepoCommand) -> Result<(), String> {
    let (_, mut c) = crate::connect()?;
    let status: Status = crate::call(&mut c, Method::Status { id: id.clone() })?;
    let mut config = status.config;
    match command {
        RepoCommand::List => {
            if config.apt_repos.is_empty() {
                out!("No apt repository.");
            }
            for r in &config.apt_repos {
                out!(
                    "{}  {} {} {}  key {} ({})",
                    r.name,
                    r.url,
                    r.suite,
                    r.components.join(" "),
                    r.fingerprint,
                    r.key_url
                );
            }
            Ok(())
        }
        RepoCommand::Add {
            name,
            url,
            suite,
            components,
            key_url,
            fingerprint,
        } => {
            config.apt_repos.push(AptRepo {
                name,
                url,
                suite,
                components,
                key_url,
                fingerprint: fingerprint.replace(' ', ""),
            });
            save(&mut c, &id, &config)
        }
        RepoCommand::Remove { name } => {
            let before = config.apt_repos.len();
            config.apt_repos.retain(|r| r.name != name);
            if config.apt_repos.len() == before {
                return Err(format!("{id} has no apt repository {name:?}"));
            }
            save(&mut c, &id, &config)
        }
    }
}
