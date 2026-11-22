//! `cardea folders <id> list|add|remove`.

use std::path::{Path, PathBuf};

use cardea_core::model::{Folder, Mode};
use cardea_proto::{Method, Saved, Status};

pub fn list(id: String) -> Result<(), String> {
    let (_, mut c) = crate::connect()?;
    let status: Status = crate::call(&mut c, Method::Status { id })?;
    if status.config.folders.is_empty() {
        out!("No shared folder.");
    }
    for f in &status.config.folders {
        let access = match f.mode {
            Mode::Rw => "read-write",
            Mode::Ro => "read-only",
        };
        let missing = if status.missing_folders.contains(&f.host) {
            "  MISSING"
        } else {
            ""
        };
        out!("{} → {}  {access}{missing}", f.host, f.mount);
    }
    out!(
        "Protected paths (read-only in read-write folders): {}",
        status.config.protected_paths.names.join(", ")
    );
    Ok(())
}

pub fn add(
    id: String,
    host: &Path,
    mount: Option<String>,
    mode: Option<Mode>,
) -> Result<(), String> {
    let host = absolute(host)?;
    let (_, mut c) = crate::connect()?;
    let status: Status = crate::call(&mut c, Method::Status { id: id.clone() })?;
    let host = host.display().to_string();
    let mut folders = status.config.folders;
    folders.push(Folder {
        mount: mount.unwrap_or_else(|| Folder::default_mount(&host)),
        mode: mode.unwrap_or(Folder::default_mode(status.config.trust)),
        host,
    });
    save(&mut c, id, folders)
}

/// `folder`: its mount point, its name or its host path.
pub fn remove(id: String, folder: &str) -> Result<(), String> {
    let (_, mut c) = crate::connect()?;
    let status: Status = crate::call(&mut c, Method::Status { id: id.clone() })?;
    let mut folders = status.config.folders;
    let before = folders.len();
    let host = absolute(Path::new(folder))
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    folders
        .retain(|f| f.mount != folder && f.name() != folder && f.host != folder && f.host != host);
    if folders.len() == before {
        return Err(format!(
            "{id} has no shared folder {folder:?} (see `cardea folders {id} list`)"
        ));
    }
    save(&mut c, id, folders)
}

fn save(c: &mut crate::client::Client, id: String, folders: Vec<Folder>) -> Result<(), String> {
    let saved: Saved = crate::call(
        c,
        Method::SetFolders {
            id,
            folders,
            protected: None,
        },
    )?;
    out!("{}", crate::saved_text(saved));
    Ok(())
}

/// An absolute, clean host path; the folder itself may not exist yet.
fn absolute(p: &Path) -> Result<PathBuf, String> {
    if let Ok(real) = p.canonicalize() {
        return Ok(real);
    }
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    std::path::absolute(cwd.join(p)).map_err(|e| e.to_string())
}
