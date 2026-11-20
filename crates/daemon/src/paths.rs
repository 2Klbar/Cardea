//! Where Cardea keeps its files.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Paths {
    /// `~/.config/cardea`: settings and user profiles.
    pub config: PathBuf,
    /// `~/.local/share/cardea`: registry and logs.
    pub data: PathBuf,
    /// `$XDG_RUNTIME_DIR/cardea`: the control socket (directory `0700`).
    pub runtime: PathBuf,
    /// Default location of new sandboxes, `~/Sandboxes`.
    pub sandboxes: PathBuf,
    /// The user's home, for `~/` in settings.
    pub home: PathBuf,
}

impl Paths {
    /// The real paths, from the XDG variables.
    pub fn from_env() -> Result<Paths, String> {
        let var = |k: &str| {
            std::env::var_os(k)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        let home = var("HOME").ok_or("HOME is not set")?;
        let runtime = var("XDG_RUNTIME_DIR")
            .ok_or("XDG_RUNTIME_DIR is not set: Cardea needs a user session (systemd --user or a desktop login)")?;
        let config = var("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
        let data = var("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share"));
        Ok(Paths {
            config: config.join("cardea"),
            data: data.join("cardea"),
            runtime: runtime.join("cardea"),
            sandboxes: home.join("Sandboxes"),
            home,
        })
    }

    /// Demo mode: everything except the socket lives in `<runtime>/demo`, wiped at each demo start, so the
    /// demo never touches real settings or sandboxes.
    pub fn demo(self) -> Paths {
        let root = self.runtime.join("demo");
        Paths {
            config: root.join("config"),
            data: root.join("data"),
            sandboxes: root.join("Sandboxes"),
            home: self.home,
            runtime: self.runtime,
        }
    }

    pub fn socket(&self) -> PathBuf {
        self.runtime.join("control.sock")
    }

    pub fn settings(&self) -> PathBuf {
        self.config.join("settings.json")
    }

    pub fn profiles(&self) -> PathBuf {
        self.config.join("profiles")
    }

    pub fn registry(&self) -> PathBuf {
        self.data.join("registry.json")
    }

    pub fn log(&self, id: &str) -> PathBuf {
        self.data.join("logs").join(id).join("sandbox.log")
    }

    /// Expands a leading `~/`.
    pub fn expand(&self, path: &str) -> PathBuf {
        match path.strip_prefix("~/") {
            Some(rest) => self.home.join(rest),
            None => Path::new(path).to_path_buf(),
        }
    }

    /// Shows a path with `~/` for the home.
    pub fn display(&self, path: &Path) -> String {
        match path.strip_prefix(&self.home) {
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => path.display().to_string(),
        }
    }
}
