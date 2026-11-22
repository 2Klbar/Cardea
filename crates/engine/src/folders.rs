//! Shared folders as they are on the host at start: missing ones are skipped, read-write ones get their protected
//! paths re-mounted read-only (R12).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use cardea_core::model::{Config, Mode};
use cardea_core::spec::Bind;

/// Folders never searched for protected paths: big generated trees, and `.git` (its paths are matched from the
/// folder that holds it).
const PRUNED: [&str; 5] = ["node_modules", "target", ".venv", "vendor", ".git"];

/// The mounts of `c`'s shared folders, and a warning for each missing one.
pub fn resolve(c: &Config) -> (Vec<Bind>, Vec<String>) {
    let (mut binds, mut warnings) = (Vec::new(), Vec::new());
    for f in &c.folders {
        let source = PathBuf::from(&f.host);
        if !source.is_dir() {
            warnings.push(format!("Shared folder {} is missing: not mounted", f.host));
            continue;
        }
        let read_only = f.mode == Mode::Ro;
        binds.push(Bind {
            source: source.clone(),
            destination: f.mount.clone(),
            read_only,
        });
        if !read_only {
            binds.extend(
                protected(&source, &c.protected_paths.names, c.protected_paths.depth)
                    .into_iter()
                    .map(|(abs, rel)| Bind {
                        source: abs,
                        destination: format!("{}/{}", f.mount.trim_end_matches('/'), rel.display()),
                        read_only: true,
                    }),
            );
        }
    }
    (binds, warnings)
}

/// The protected paths found in `root`, down to `depth` folders: (host path, path relative to `root`). Symbolic
/// links are resolved; a target outside `root` is not reachable from the sandbox and is left out.
pub fn protected(root: &Path, names: &[String], depth: u8) -> Vec<(PathBuf, PathBuf)> {
    let Ok(root) = root.canonicalize() else {
        return Vec::new();
    };
    let mut found = BTreeSet::new();
    let mut level = vec![root.clone()];
    for d in 0..=depth {
        let mut next = Vec::new();
        for dir in &level {
            for name in names {
                if let Ok(abs) = dir.join(name).canonicalize()
                    && let Ok(rel) = abs.strip_prefix(&root)
                    && !rel.as_os_str().is_empty()
                {
                    found.insert((abs.clone(), rel.to_path_buf()));
                }
            }
            if d == depth {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            for e in entries.flatten() {
                // `file_type` does not follow links: linked folders are not walked.
                if e.file_type().is_ok_and(|t| t.is_dir())
                    && !PRUNED.iter().any(|p| e.file_name() == *p)
                {
                    next.push(e.path());
                }
            }
        }
        level = next;
    }
    found.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cardea_core::builtin;
    use cardea_core::model::Folder;
    use std::fs;

    fn names() -> Vec<String> {
        builtin::profile("empty").unwrap().protected_paths.names
    }

    fn rels(root: &Path, depth: u8) -> Vec<String> {
        protected(root, &names(), depth)
            .into_iter()
            .map(|(_, r)| r.display().to_string())
            .collect()
    }

    #[test]
    fn default_list_depth_and_pruned_folders() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        for d in [
            ".git/hooks",
            "a/b/.vscode",
            "a/b/c/d/e/f/.vscode",
            "node_modules/x/.git/hooks",
            "sub/.git/hooks",
        ] {
            fs::create_dir_all(r.join(d)).unwrap();
        }
        fs::write(r.join(".git/config"), "").unwrap();
        fs::write(r.join(".envrc"), "").unwrap();
        // depth 5: a/b/c/d/e is the 5th level below the root, f is beyond.
        assert_eq!(
            rels(r, 5),
            [
                ".envrc",
                ".git/config",
                ".git/hooks",
                "a/b/.vscode",
                "sub/.git/hooks"
            ]
        );
        assert_eq!(rels(r, 0), [".envrc", ".git/config", ".git/hooks"]);
        assert_eq!(rels(r, 6).len(), 6);
    }

    #[test]
    fn links_are_resolved_inside_the_folder_only() {
        let t = tempfile::tempdir().unwrap();
        let (r, outside) = (t.path().join("p"), t.path().join("outside"));
        fs::create_dir_all(r.join("real-hooks")).unwrap();
        fs::create_dir_all(r.join(".git")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink("../real-hooks", r.join(".git/hooks")).unwrap();
        std::os::unix::fs::symlink(&outside, r.join(".vscode")).unwrap();
        assert_eq!(rels(&r, 5), ["real-hooks"]);
    }

    #[test]
    fn missing_and_read_only_folders() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join("rw/.git/hooks")).unwrap();
        fs::create_dir_all(t.path().join("ro/.git/hooks")).unwrap();
        let mut c = builtin::profile("empty").unwrap();
        let folder = |host: &str, mount: &str, mode| Folder {
            host: t.path().join(host).display().to_string(),
            mount: mount.into(),
            mode,
        };
        c.folders = vec![
            folder("rw", "/work/rw", Mode::Rw),
            folder("ro", "/work/ro", Mode::Ro),
            folder("gone", "/work/gone", Mode::Rw),
        ];
        let (binds, warnings) = resolve(&c);
        let got: Vec<_> = binds
            .iter()
            .map(|b| (b.destination.as_str(), b.read_only))
            .collect();
        assert_eq!(
            got,
            [
                ("/work/rw", false),
                ("/work/rw/.git/hooks", true),
                ("/work/ro", true)
            ]
        );
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("gone") && warnings[0].contains("missing"));
    }
}
