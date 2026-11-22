//! Files on disk: registry, settings, user profiles, sandbox configs and logs.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use cardea_core::load;
use cardea_core::model::{Config, Kind, Settings};
use serde::{Deserialize, Serialize};

use crate::paths::Paths;

/// `registry.json`: the sandbox folders, which can live anywhere.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    pub schema: u32,
    pub sandboxes: Vec<Registered>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registered {
    pub id: String,
    pub folder: PathBuf,
}

/// Writes `text` to `path` through a temporary file and a rename, so a crash never leaves half a file.
pub fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut f = fs::File::create(&tmp)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    fs::rename(tmp, path)
}

fn to_json<T: Serialize>(t: &T) -> String {
    serde_json::to_string_pretty(t).expect("serializable") + "\n"
}

pub fn load_registry(paths: &Paths) -> Result<Registry, String> {
    match fs::read_to_string(paths.registry()) {
        Ok(text) => {
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", paths.registry().display()))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Registry {
            schema: 1,
            sandboxes: Vec::new(),
        }),
        Err(e) => Err(format!("{}: {e}", paths.registry().display())),
    }
}

pub fn save_registry(paths: &Paths, registry: &Registry) -> io::Result<()> {
    write_atomic(&paths.registry(), &to_json(registry))
}

/// The settings, or the defaults when the file does not exist. An invalid file is an error.
pub fn load_settings(paths: &Paths) -> Result<Settings, String> {
    match fs::read_to_string(paths.settings()) {
        Ok(text) => load::settings(&text)
            .map_err(|e| format!("{} is invalid:\n{e}", paths.settings().display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(e) => Err(format!("{}: {e}", paths.settings().display())),
    }
}

/// User profiles; invalid files are returned as errors, one per file, and skipped.
pub fn load_user_profiles(paths: &Paths) -> (Vec<Config>, Vec<String>) {
    let (mut profiles, mut errors) = (Vec::new(), Vec::new());
    let Ok(dir) = fs::read_dir(paths.profiles()) else {
        return (profiles, errors);
    };
    let mut files: Vec<PathBuf> = dir.flatten().map(|e| e.path()).collect();
    files.sort();
    for path in files
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
    {
        match fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| load::config(&t, Kind::Profile).map_err(|e| format!("\n{e}")))
        {
            Ok(p) => profiles.push(p),
            Err(e) => errors.push(format!("{} is skipped: {e}", path.display())),
        }
    }
    (profiles, errors)
}

pub fn config_file(folder: &Path) -> PathBuf {
    folder.join("sandbox.json")
}

pub fn load_config(folder: &Path) -> Result<Config, String> {
    let path = config_file(folder);
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    load::config(&text, Kind::Sandbox).map_err(|e| format!("{} is invalid:\n{e}", path.display()))
}

pub fn save_config(folder: &Path, config: &Config) -> io::Result<()> {
    write_atomic(&config_file(folder), &to_json(config))
}

pub fn save_settings(paths: &Paths, settings: &Settings) -> io::Result<()> {
    fs::create_dir_all(&paths.config)?;
    write_atomic(&paths.settings(), &to_json(settings))
}

pub fn user_profile_file(paths: &Paths, id: &str) -> PathBuf {
    paths.profiles().join(format!("{id}.json"))
}

pub fn save_user_profile(paths: &Paths, profile: &Config) -> io::Result<()> {
    fs::create_dir_all(paths.profiles())?;
    write_atomic(&user_profile_file(paths, &profile.id), &to_json(profile))
}

/// Bytes of the regular files under `path`; links are not followed.
pub fn disk_use(path: &Path) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    if !meta.is_dir() {
        return if meta.is_file() { meta.len() } else { 0 };
    }
    fs::read_dir(path)
        .map(|d| d.flatten().map(|e| disk_use(&e.path())).sum())
        .unwrap_or(0)
}

/// Appends a timestamped line to a sandbox log; returns the line as written.
pub fn append_log(paths: &Paths, id: &str, text: &str) -> String {
    let line = format!(
        "{} {text}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    );
    let path = paths.log(id);
    let result = fs::create_dir_all(path.parent().expect("log has a folder"))
        .and_then(|()| fs::OpenOptions::new().create(true).append(true).open(&path))
        .and_then(|mut f| writeln!(f, "{line}"));
    if let Err(e) = result {
        eprintln!("cardea daemon: cannot write {}: {e}", path.display());
    }
    line
}

/// The last `n` lines of a sandbox log.
pub fn tail_log(paths: &Paths, id: &str, n: usize) -> Vec<String> {
    let text = fs::read_to_string(paths.log(id)).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..]
        .iter()
        .map(|l| l.to_string())
        .collect()
}
