//! The Podman engine: builds images, runs each sandbox as a pod with its app container, through the libpod REST
//! API ([`crate::podman::Client`]).

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cardea_core::apps::{self, Detected, Launch};
use cardea_core::builtin::Manager;
use cardea_core::firewall::{self, NetStatus};
use cardea_core::image::{self, Ids, LABEL};
use cardea_core::model::{Clipboard, Config, KeyringMode, Trust, X11};
use cardea_core::services::{self, Instance};
use cardea_core::spec::{self, Host};
use cardea_core::{packages, risk};
use serde_json::json;
use std::os::unix::process::CommandExt;

use crate::podman::{Client, Lines, Output};
use crate::{BoxFuture, Engine, Exec, Progress, Sandbox, Started, Watch, folders, net};

/// Run in a throw-away container of the base image: the exit code names its package manager.
const DETECT: &str = "command -v apt-get >/dev/null && exit 10; command -v dnf >/dev/null && exit 11; \
                      command -v pacman >/dev/null && exit 12; exit 1";
/// The session bus and the keyring, as the sandbox user; the keyring password comes on standard input (empty for
/// `auto`), never in an argument or the environment. The daemons' output goes to files: an inherited stdout or
/// stderr would stay open after the command ends, and its output would never finish.
const KEYRING_SESSION: &str = "e=/tmp/xdg-runtime/session.err
dbus-daemon --session --address=\"$DBUS_SESSION_BUS_ADDRESS\" --fork --nopidfile >/dev/null 2>$e || { cat $e >&2; exit 1; }
i=0; while [ ! -S /tmp/xdg-runtime/bus ]; do i=$((i+1)); [ $i -gt 50 ] && exit 2; sleep 0.1; done
gnome-keyring-daemon --daemonize --unlock --components=secrets >/dev/null 2>$e || { cat $e >&2; exit 3; }
gnome-keyring-daemon --start --components=secrets >/dev/null 2>$e || { cat $e >&2; exit 4; }";
/// Every `.desktop` file, each after a `@@ <path>` line.
const LIST_DESKTOP: &str = "for f in /usr/share/applications/*.desktop /usr/local/share/applications/*.desktop \
                            \"$HOME\"/.local/share/applications/*.desktop; do \
                            [ -f \"$f\" ] && { printf '@@ %s\\n' \"$f\"; cat \"$f\"; echo; }; done; true";
/// The best file of icon `$1` (scalable, then the biggest PNG, then pixmaps), at most 1 MiB: its path, then its
/// bytes.
const FIND_ICON: &str = "n=$1; case $n in /*) f=$n ;; *) \
                         for f in /usr/share/icons/hicolor/scalable/apps/\"$n\".svg \
                         $(ls -1d /usr/share/icons/hicolor/*/apps/\"$n\".png 2>/dev/null | sort -rV) \
                         /usr/share/pixmaps/\"$n\".svg /usr/share/pixmaps/\"$n\".png; do [ -f \"$f\" ] && break; f=; done ;; \
                         esac; [ -f \"$f\" ] || exit 0; [ \"$(wc -c < \"$f\")\" -le 1048576 ] || exit 0; \
                         printf '%s\\n' \"$f\"; cat \"$f\"";
/// The user's login shell, from the account database.
const LOGIN_SHELL: &str = "exec \"$(getent passwd \"$(id -u)\" | cut -d: -f7)\" -l";

/// What the capabilities need from the host, resolved at start.
#[derive(Default)]
struct Extras {
    wayland: Option<PathBuf>,
    devices: Vec<String>,
    x11_host: Option<(String, Option<PathBuf>)>,
    keyrings: Option<PathBuf>,
    links: Option<(PathBuf, PathBuf)>,
}

pub struct PodmanEngine {
    client: Client,
    /// The daemon's runtime folder (yours alone): the Wayland filters' sockets and PIDs.
    runtime: PathBuf,
    ids: Ids,
    /// The ruleset last applied to each running sandbox.
    applied: Mutex<HashMap<String, String>>,
}

impl PodmanEngine {
    pub fn new(socket: &Path, runtime: &Path, ids: Ids) -> PodmanEngine {
        PodmanEngine {
            client: Client::new(socket),
            runtime: runtime.to_owned(),
            ids,
            applied: Mutex::new(HashMap::new()),
        }
    }

    fn applied(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        self.applied.lock().expect("ruleset cache lock")
    }

    /// Runs a command as root in a container and collects its output: (exit code, stdout, stderr).
    async fn run_in(&self, container: &str, cmd: &[&str]) -> Result<(i32, String, String), String> {
        let (code, out, err) = self.run_as(container, "0:0", cmd, &[], None).await?;
        Ok((code, String::from_utf8_lossy(&out).into_owned(), err))
    }

    /// Runs a command in a container as `user` and collects its output: (exit code, stdout, stderr). `input` is
    /// written to its standard input, then closed.
    async fn run_as<S: AsRef<str>>(
        &self,
        container: &str,
        user: &str,
        cmd: &[S],
        env: &[String],
        input: Option<Vec<u8>>,
    ) -> Result<(i32, Vec<u8>, String), String> {
        let cmd: Vec<&str> = cmd.iter().map(AsRef::as_ref).collect();
        let spec = json!({
            "AttachStdin": input.is_some(),
            "AttachStdout": true,
            "AttachStderr": true,
            "Tty": false,
            "Cmd": cmd,
            "User": user,
            "Env": env,
        });
        let exec = self.client.exec_create(container, &spec).await?;
        let stream = self.client.exec_start(&exec, false, 0, 0).await?;
        let (out, err) = tokio::task::spawn_blocking(move || demux_all(stream, input))
            .await
            .map_err(|e| e.to_string())??;
        let code = self.exec_wait(&exec).await?;
        Ok((code, out, String::from_utf8_lossy(&err).into_owned()))
    }

    /// Starts a command in the background in the app container, as the sandbox user.
    async fn detached(&self, id: &str, argv: &[String], env: &[String]) -> Result<(), String> {
        let spec = json!({
            "AttachStdin": false,
            "AttachStdout": false,
            "AttachStderr": false,
            "Tty": false,
            "Cmd": argv,
            "User": self.user(),
            "Env": env,
        });
        let exec = self.client.exec_create(&spec::app_name(id), &spec).await?;
        self.client.exec_detach(&exec).await
    }

    fn user(&self) -> String {
        format!("{}:{}", self.ids.uid, self.ids.gid)
    }

    /// What the capabilities need from the host: the Wayland socket, devices, the X display and its cookie, the
    /// kept keyring folder. What is missing is logged and left out.
    fn host_extras(&self, c: &Config, state: &Path, progress: &Progress) -> Result<Extras, String> {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let mut x = Extras::default();
        if c.capabilities.wayland {
            let display = env("WAYLAND_DISPLAY").unwrap_or_else(|| "wayland-0".into());
            let socket = match (display.starts_with('/'), env("XDG_RUNTIME_DIR")) {
                (true, _) => Some(PathBuf::from(&display)),
                (false, Some(dir)) => Some(Path::new(&dir).join(&display)),
                (false, None) => None,
            };
            match socket.filter(|p| p.exists()) {
                Some(p) => {
                    x.wayland =
                        Some(self.start_filter(&c.id, &p, c.capabilities.clipboard, state)?);
                }
                None => progress.log(
                    "Warning: no Wayland socket in this session: apps will not show their windows",
                ),
            }
        }
        let mut device = |path: &str, why: &str| {
            if Path::new(path).exists() {
                x.devices.push(path.to_owned());
            } else {
                progress.log(format!(
                    "Warning: {path} does not exist on this computer: {why}"
                ));
            }
        };
        if c.capabilities.gpu {
            device("/dev/dri", "no GPU acceleration");
        }
        if c.capabilities.kvm {
            device("/dev/kvm", "no virtual machines");
            device(
                "/dev/vhost-vsock",
                "virtual machines cannot talk to the sandbox",
            );
        }
        if c.capabilities.x11 == X11::Host {
            match env("DISPLAY") {
                Some(display) => {
                    let cookie = env("XAUTHORITY")
                        .map(PathBuf::from)
                        .or_else(|| env("HOME").map(|h| Path::new(&h).join(".Xauthority")))
                        .filter(|p| p.is_file())
                        .map(|p| {
                            let copy = state.join("xauthority");
                            std::fs::copy(&p, &copy)
                                .map(|_| copy)
                                .map_err(|e| format!("cannot copy {}: {e}", p.display()))
                        })
                        .transpose()?;
                    x.x11_host = Some((display, cookie));
                }
                None => {
                    progress.log("Warning: no X display in this session: X11 apps will not show")
                }
            }
        }
        if c.home.disposable
            && c.capabilities.keyring.keep
            && c.capabilities.keyring.mode != KeyringMode::Off
        {
            let dir = state.join("keyrings");
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
            x.keyrings = Some(dir);
        }
        if c.links.outgoing {
            x.links = Some(self.start_relay(&c.id, state)?);
        }
        Ok(x)
    }

    /// After the app container starts: the session bus and keyring, the private Xwayland, the services to start.
    async fn session(&self, sb: &Sandbox<'_>, progress: &Progress) -> Result<(), String> {
        let c = sb.config;
        let app = spec::app_name(&c.id);
        if c.capabilities.keyring.mode != KeyringMode::Off {
            let password = match c.capabilities.keyring.mode {
                KeyringMode::Ask => sb
                    .password
                    .ok_or("this sandbox's keyring needs its password")?,
                _ => "",
            };
            let input = Some(password.as_bytes().to_vec());
            let (code, _, err) = self
                .run_as(
                    &app,
                    &self.user(),
                    &["sh", "-c", KEYRING_SESSION],
                    &[],
                    input,
                )
                .await?;
            if code != 0 {
                return Err(format!(
                    "the keyring could not be started or unlocked: {}",
                    err.trim()
                ));
            }
            progress.log("Keyring unlocked");
        }
        if c.capabilities.x11 == X11::Isolated {
            let argv = [
                "xwayland-satellite".to_owned(),
                spec::ISOLATED_DISPLAY.to_owned(),
            ];
            self.detached(&c.id, &argv, &[]).await?;
        }
        for (s, folder) in services::declared(c) {
            if services::autostarts(s, folder) {
                let name = services::session(&s.id, folder);
                let cmd = services::start_command(c, s, folder);
                match self.run_as(&app, &self.user(), &cmd, &[], None).await {
                    Ok((0, _, _)) => progress.log(format!("Service {name} started")),
                    Ok((_, _, err)) => progress.log(format!(
                        "Warning: service {name} did not start: {}",
                        err.trim()
                    )),
                    Err(e) => progress.log(format!("Warning: service {name} did not start: {e}")),
                }
            }
        }
        Ok(())
    }

    /// Builds the netguard image when it is missing.
    async fn netguard_image(&self, progress: &Progress) -> Result<(), String> {
        if self.client.image_exists(firewall::NETGUARD_TAG).await? {
            return Ok(());
        }
        progress.log("Building the firewall sidecar image");
        let build = self
            .client
            .build(
                firewall::NETGUARD_TAG,
                tar(&firewall::netguard_context()),
                false,
            )
            .await?;
        self.follow(build, progress, |_, _| {}).await
    }

    /// Writes the ruleset where the sidecar reads it and applies it (atomically: all or nothing).
    async fn apply(&self, sb: &Sandbox<'_>, rules: &str) -> Result<(), String> {
        let dir = sb.folder.join("state/netguard");
        let tmp = dir.join(".rules.nft");
        std::fs::write(&tmp, rules)
            .and_then(|()| std::fs::rename(&tmp, dir.join(firewall::RULES_FILE)))
            .map_err(|e| format!("cannot write the firewall rules in {}: {e}", dir.display()))?;
        let file = format!("{}/{}", firewall::RULES_DIR, firewall::RULES_FILE);
        let (code, _, err) = self
            .run_in(&spec::netguard_name(&sb.config.id), &["nft", "-f", &file])
            .await?;
        if code != 0 {
            return Err(format!(
                "the firewall rules could not be applied: {}",
                err.trim()
            ));
        }
        self.applied()
            .insert(sb.config.id.clone(), rules.to_owned());
        Ok(())
    }

    /// Waits until the sidecar's drop-all ruleset is in place.
    async fn drop_all_applied(&self, id: &str) -> Result<(), String> {
        let netguard = spec::netguard_name(id);
        for _ in 0..50 {
            if let Ok((0, _, _)) = self
                .run_in(&netguard, &["nft", "list", "table", "inet", "cardea"])
                .await
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Err("the firewall sidecar did not set its first rules".to_owned())
    }

    /// Logs a pull or build output; fails on its first error line.
    async fn follow(
        &self,
        mut lines: Lines<Output>,
        progress: &Progress,
        on_step: impl Fn(u32, u32),
    ) -> Result<(), String> {
        while let Some(line) = lines.next().await {
            let line = line?;
            if let Some(e) = line.error {
                progress.log(e.trim_end());
                return Err(e.trim().to_owned());
            }
            for text in line
                .stream
                .iter()
                .flat_map(|s| s.lines())
                .filter(|l| !l.trim().is_empty())
            {
                if let Some((n, m)) = build_step(text) {
                    on_step(n, m);
                }
                progress.log(text.trim_end());
            }
        }
        Ok(())
    }

    async fn detect(&self, id: &str, base: &str) -> Result<Option<Manager>, String> {
        let name = format!("cardea-{id}-detect");
        self.client.remove_container(&name).await?;
        let spec = json!({
            "name": name,
            "image": base,
            "command": ["sh", "-c", DETECT],
            "netns": { "nsmode": "none" },
            "labels": { LABEL: id },
        });
        self.client.create_container(&spec).await?;
        let code = async {
            self.client.start_container(&name).await?;
            self.client.wait(&name).await
        }
        .await;
        self.client.remove_container(&name).await?;
        Ok(match code? {
            10 => Some(Manager::Apt),
            11 => Some(Manager::Dnf),
            12 => Some(Manager::Pacman),
            _ => None,
        })
    }

    /// R13: no other running container may mount this home.
    /// Creates the pod; when Podman finds no free sub-IDs for an Untrusted sandbox, says it is the limit of
    /// Untrusted sandboxes running at once, and what it is here.
    async fn create_pod(&self, c: &Config) -> Result<(), String> {
        let Err(e) = self.client.create_pod(&spec::pod(c, self.ids)).await else {
            return Ok(());
        };
        if !(c.security.isolated_uid && e.contains("not enough unused IDs")) {
            return Err(e);
        }
        let sub_ids = self
            .client
            .info()
            .await
            .ok()
            .and_then(|i| i.host.id_mappings.uidmap)
            .map_or(0, |m| {
                // The first range is you; the others are your sub-UIDs.
                m.iter().skip(1).map(|r| r.size).sum::<u64>()
            });
        Err(untrusted_limit_message(sub_ids, self.ids))
    }

    async fn home_in_use(&self, id: &str, home: &Path) -> Result<(), String> {
        let home = home.canonicalize().unwrap_or_else(|_| home.to_owned());
        let ours = spec::pod_name(id);
        for c in self.client.containers().await? {
            let name = c.names.first().cloned().unwrap_or_default();
            if name == ours || name.starts_with(&format!("{ours}-")) {
                continue;
            }
            let Ok(inspected) = self.client.inspect(&c.id).await else {
                continue;
            };
            if inspected
                .mounts
                .iter()
                .any(|m| Path::new(&m.source) == home)
            {
                return Err(format!(
                    "the home {} is already used by the running container {name}; stop it first",
                    home.display()
                ));
            }
        }
        Ok(())
    }

    /// Removes the pod and its containers (anonymous volumes included: a disposable home goes with them).
    /// Starts the sandbox's Wayland filter (`cardea wayland-filter`) and returns its socket. A process of its own,
    /// so it outlives a daemon restart like the containers do (the unit has `KillMode=process`).
    fn start_filter(
        &self,
        id: &str,
        upstream: &Path,
        clipboard: Clipboard,
        state: &Path,
    ) -> Result<PathBuf, String> {
        self.stop_filter(id);
        let (socket, pid_file) = self.filter_files(id);
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(state.join("wayland.log"))
            .map_err(|e| e.to_string())?;
        let mut cmd = std::process::Command::new(
            std::env::current_exe().map_err(|e| format!("cannot find cardea: {e}"))?,
        );
        cmd.arg("wayland-filter")
            .arg("--listen")
            .arg(&socket)
            .arg("--upstream")
            .arg(upstream)
            .arg("--sandbox")
            .arg(id);
        if clipboard == Clipboard::Off {
            cmd.arg("--clipboard-off");
        }
        let mut child = cmd
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(log)
            .process_group(0)
            .spawn()
            .map_err(|e| format!("cannot start the Wayland filter: {e}"))?;
        std::fs::write(&pid_file, child.id().to_string()).map_err(|e| e.to_string())?;
        for _ in 0..100 {
            if socket.exists() {
                // Reaped here while this daemon lives; by the session's manager after.
                std::thread::spawn(move || child.wait());
                return Ok(socket);
            }
            if let Ok(Some(status)) = child.try_wait() {
                return Err(format!(
                    "the Wayland filter stopped at once ({status}): see state/wayland.log"
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let _ = child.kill();
        let _ = child.wait();
        Err("the Wayland filter did not start".into())
    }

    fn filter_files(&self, id: &str) -> (PathBuf, PathBuf) {
        (
            self.runtime.join(format!("wayland-{id}.sock")),
            self.runtime.join(format!("wayland-{id}.pid")),
        )
    }

    fn stop_filter(&self, id: &str) {
        let (socket, pid_file) = self.filter_files(id);
        stop_helper(&pid_file, "wayland-filter", &socket);
    }

    fn relay_files(&self, id: &str) -> (PathBuf, PathBuf) {
        (
            self.runtime.join(format!("links-{id}.fifo")),
            self.runtime.join(format!("links-{id}.pid")),
        )
    }

    /// Starts the sandbox's link relay (`cardea link-relay`) on a fresh FIFO, writes the guest opener, and
    /// returns both for the app container (R3). A process of its own, like the Wayland filter: what the sandbox
    /// writes is read outside the daemon.
    fn start_relay(&self, id: &str, state: &Path) -> Result<(PathBuf, PathBuf), String> {
        use std::os::unix::fs::PermissionsExt;
        self.stop_relay(id);
        let (fifo, pid_file) = self.relay_files(id);
        rustix::fs::mknodat(
            rustix::fs::CWD,
            &fifo,
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::from_raw_mode(0o622),
            0,
        )
        .map_err(|e| format!("cannot create {}: {e}", fifo.display()))?;
        // Writable by an isolated sandbox user too; read by you alone. The umask is not applied to this.
        std::fs::set_permissions(&fifo, std::fs::Permissions::from_mode(0o622))
            .map_err(|e| e.to_string())?;
        let opener = state.join("xdg-open");
        std::fs::write(&opener, cardea_core::links::OPENER)
            .and_then(|()| {
                std::fs::set_permissions(&opener, std::fs::Permissions::from_mode(0o755))
            })
            .map_err(|e| format!("cannot write {}: {e}", opener.display()))?;
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(state.join("links.log"))
            .map_err(|e| e.to_string())?;
        let mut child = std::process::Command::new(
            std::env::current_exe().map_err(|e| format!("cannot find cardea: {e}"))?,
        )
        .arg("link-relay")
        .arg("--fifo")
        .arg(&fifo)
        .arg("--sandbox")
        .arg(id)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(log)
        .process_group(0)
        .spawn()
        .map_err(|e| format!("cannot start the link relay: {e}"))?;
        std::fs::write(&pid_file, child.id().to_string()).map_err(|e| e.to_string())?;
        std::thread::sleep(Duration::from_millis(50));
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!(
                "the link relay stopped at once ({status}): see state/links.log"
            ));
        }
        std::thread::spawn(move || child.wait());
        Ok((fifo, opener))
    }

    fn stop_relay(&self, id: &str) {
        let (fifo, pid_file) = self.relay_files(id);
        stop_helper(&pid_file, "link-relay", &fifo);
    }

    async fn tear_down(&self, id: &str) -> Result<(), String> {
        self.stop_filter(id);
        self.stop_relay(id);
        self.applied().remove(id);
        self.client.remove_container(&spec::app_name(id)).await?;
        self.client
            .remove_container(&spec::netguard_name(id))
            .await?;
        self.client.remove_pod(&spec::pod_name(id)).await
    }
}

/// Stops a helper process (`cardea <cmd> … <path>`), if its PID file still names it (not a reused PID), and
/// removes its files.
fn stop_helper(pid_file: &Path, cmd: &str, path: &Path) {
    let pid = std::fs::read_to_string(pid_file)
        .ok()
        .and_then(|p| p.trim().parse::<i32>().ok());
    if let Some(pid) = pid {
        let ours = std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|cmdline| {
            let cmdline = String::from_utf8_lossy(&cmdline);
            cmdline.contains(cmd) && cmdline.contains(&*path.to_string_lossy())
        });
        if let (true, Some(pid)) = (ours, rustix::process::Pid::from_raw(pid)) {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
        }
    }
    let _ = std::fs::remove_file(pid_file);
    let _ = std::fs::remove_file(path);
}

impl Engine for PodmanEngine {
    fn build<'a>(
        &'a self,
        sb: &'a Sandbox<'a>,
        progress: Progress,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let c = sb.config;
            progress.step(1, 3, "base image");
            if sb.fresh || !self.client.image_exists(&c.base_image).await? {
                let pull = self.client.pull(&c.base_image).await?;
                self.follow(pull, &progress, |_, _| {}).await?;
            }
            progress.percent(10);
            progress.step(2, 3, "package manager");
            let manager = self.detect(&c.id, &c.base_image).await?;
            progress.log(match manager {
                Some(m) => format!("Package manager: {}", packages::manager_name(m)),
                None => "No supported package manager in the base image".to_owned(),
            });
            // Read back by the daemon for the system source's commands in the running sandbox.
            let state = sb.folder.join("state");
            std::fs::create_dir_all(&state)
                .and_then(|()| {
                    std::fs::write(
                        state.join("package-manager"),
                        manager.map_or("", packages::manager_name),
                    )
                })
                .map_err(|e| format!("cannot write {}: {e}", state.display()))?;
            progress.percent(15);
            progress.step(3, 3, "image");
            let files = image::context(c, manager, self.ids)?;
            let build = self
                .client
                .build(&image::tag(&c.id), tar(&files), sb.fresh)
                .await?;
            self.follow(build, &progress, |n, m| {
                progress.percent((15 + 85 * (n - 1) / m.max(1)) as u8)
            })
            .await?;
            if !self.client.image_exists(&image::tag(&c.id)).await? {
                return Err("the build ended without an image".to_owned());
            }
            progress.percent(100);
            Ok(())
        })
    }

    fn start<'a>(
        &'a self,
        sb: &'a Sandbox<'a>,
        progress: Progress,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let c = sb.config;
            progress.step(1, 4, "checks");
            if !self.client.image_exists(&image::tag(&c.id)).await? {
                return Err("the image is missing: rebuild the sandbox".to_owned());
            }
            self.netguard_image(&progress).await?;
            let home = (!c.home.disposable).then(|| sb.home.to_owned());
            if let Some(home) = &home {
                std::fs::create_dir_all(home)
                    .map_err(|e| format!("cannot create the home {}: {e}", home.display()))?;
                self.home_in_use(&c.id, home).await?;
            }
            let state = sb.folder.join("state");
            let rules_dir = state.join("netguard");
            std::fs::create_dir_all(&rules_dir).map_err(|e| e.to_string())?;
            let machine_id = state.join("machine-id");
            if !machine_id.exists() {
                std::fs::write(&machine_id, format!("{}\n", random_hex()?))
                    .map_err(|e| format!("cannot write {}: {e}", machine_id.display()))?;
            }
            let (binds, warnings) = folders::resolve(c);
            for w in warnings {
                progress.log(w);
            }
            progress.percent(15);
            progress.step(2, 4, "pod");
            self.tear_down(&c.id).await?;
            self.create_pod(c).await?;
            self.client
                .create_container(&spec::netguard(c, &rules_dir))
                .await?;
            let extras = self.host_extras(c, &state, &progress)?;
            if c.trust == Trust::Untrusted {
                for w in risk::warnings(c) {
                    progress.log(format!("Warning: {w}"));
                }
            }
            let host = Host {
                ids: self.ids,
                home,
                machine_id,
                folders: binds,
                wayland: extras.wayland,
                devices: extras.devices,
                x11_host: extras.x11_host,
                keyrings: extras.keyrings,
                links: extras.links,
            };
            self.client.create_container(&spec::app(c, &host)).await?;
            progress.percent(40);
            // Fail closed (R7): the app container starts only once the real rules are in place.
            progress.step(3, 4, "firewall rules");
            let rules = async {
                self.client
                    .start_container(&spec::netguard_name(&c.id))
                    .await?;
                self.drop_all_applied(&c.id).await?;
                self.firewall(sb).await
            }
            .await;
            if let Err(e) = rules {
                let _ = self.tear_down(&c.id).await;
                return Err(format!("{e}; the sandbox was not started"));
            }
            progress.log(if c.firewall.enabled {
                "Firewall rules applied"
            } else {
                "Firewall off: every connection is allowed"
            });
            progress.percent(70);
            progress.step(4, 4, "app container");
            let started = async {
                self.client.start_container(&spec::app_name(&c.id)).await?;
                self.session(sb, &progress).await
            }
            .await;
            if let Err(e) = started {
                let _ = self.tear_down(&c.id).await;
                return Err(e);
            }
            progress.percent(100);
            Ok(())
        })
    }

    fn stop<'a>(&'a self, id: &'a str, progress: Progress) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            progress.step(1, 2, "stop");
            // A pod that is already gone is stopped.
            let _ = self.client.stop_pod(&spec::pod_name(id), 5).await;
            progress.percent(50);
            progress.step(2, 2, "clean up");
            self.tear_down(id).await?;
            progress.percent(100);
            Ok(())
        })
    }

    fn remove<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.tear_down(id).await?;
            self.client
                .remove_container(&format!("cardea-{id}-detect"))
                .await?;
            self.client.remove_image(&image::tag(id)).await
        })
    }

    fn problem(&self) -> Option<String> {
        (!self.client.socket().exists()).then(|| {
            format!(
                "Podman does not answer: {} is missing. Enable it with `systemctl --user enable --now podman.socket`, \
                 then run `cardea doctor`",
                self.client.socket().display()
            )
        })
    }

    fn running(&self) -> BoxFuture<'_, Result<Vec<String>, String>> {
        Box::pin(async move {
            Ok(self
                .client
                .containers()
                .await?
                .iter()
                .filter_map(|c| sandbox_of(c.names.first()?))
                .collect())
        })
    }

    fn watch(&self, sink: Arc<dyn Fn(Watch) + Send + Sync>) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            let events = async {
                let mut lines = self.client.events().await?;
                while let Some(e) = lines.next().await {
                    let e = e?;
                    if e.kind == "container"
                        && e.action == "died"
                        && let Some((id, sidecar)) =
                            e.actor.attributes.get("name").and_then(|n| part_of(n))
                    {
                        let code = e
                            .actor
                            .attributes
                            .get("containerExitCode")
                            .and_then(|c| c.parse().ok());
                        sink(Watch::Exited { id, code, sidecar });
                    }
                }
                Err::<(), _>("Podman closed its event stream".to_owned())
            };
            let stats = async {
                let mut lines = self.client.stats().await?;
                while let Some(report) = lines.next().await {
                    for s in report?.stats.unwrap_or_default() {
                        if let Some(id) = sandbox_of(&s.name) {
                            sink(Watch::Stats {
                                id,
                                cpu: s.cpu as f32,
                                memory: s.memory,
                            });
                        }
                    }
                }
                Err::<(), _>("Podman closed its stats stream".to_owned())
            };
            tokio::select! {
                r = events => r,
                r = stats => r,
            }
        })
    }

    fn firewall<'a>(
        &'a self,
        sb: &'a Sandbox<'a>,
    ) -> BoxFuture<'a, Result<Option<NetStatus>, String>> {
        Box::pin(async move {
            let netguard = spec::netguard_name(&sb.config.id);
            let (_, view, _) = self
                .run_in(&netguard, &["cat", "/etc/resolv.conf", "/etc/hosts"])
                .await?;
            let resolved = net::resolve(&view);
            let rules = firewall::ruleset(&sb.config.firewall, &resolved);
            let current = self.applied().get(&sb.config.id) == Some(&rules);
            if !current {
                self.apply(sb, &rules).await?;
            }
            let (blocked_count, blocked) = if sb.config.firewall.enabled {
                let (_, json, _) = self
                    .run_in(&netguard, &["nft", "-j", "list", "table", "inet", "cardea"])
                    .await?;
                firewall::read_state(&json)?
            } else {
                Default::default()
            };
            Ok(Some(NetStatus {
                references: NetStatus::references(&resolved),
                blocked_count,
                blocked,
            }))
        })
    }

    fn launch<'a>(&'a self, id: &'a str, l: Launch) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move { self.detached(id, &l.argv, &l.env).await })
    }

    fn detect_apps<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Vec<Detected>, String>> {
        Box::pin(async move {
            let app = spec::app_name(id);
            let (_, out, _) = self
                .run_as(&app, &self.user(), &["sh", "-c", LIST_DESKTOP], &[], None)
                .await?;
            let out = String::from_utf8_lossy(&out);
            let mut found: Vec<Detected> = Vec::new();
            for file in out.split("\n@@ ").map(|f| f.trim_start_matches("@@ ")) {
                let (path, text) = file.split_once('\n').unwrap_or((file, ""));
                let name = path.rsplit('/').next().unwrap_or(path);
                if let Some(d) = apps::parse_desktop(name, text)
                    && !found.iter().any(|f| f.id == d.id)
                {
                    found.push(d);
                }
            }
            found.sort_by_key(|d| d.name.to_lowercase());
            Ok(found)
        })
    }

    fn icon<'a>(
        &'a self,
        id: &'a str,
        icon: &'a str,
    ) -> BoxFuture<'a, Result<Option<crate::IconFile>, String>> {
        Box::pin(async move {
            let app = spec::app_name(id);
            let cmd = ["sh", "-c", FIND_ICON, "sh", icon];
            let (_, out, _) = self.run_as(&app, &self.user(), &cmd, &[], None).await?;
            let Some(end) = out.iter().position(|&b| b == b'\n') else {
                return Ok(None);
            };
            let path = String::from_utf8_lossy(&out[..end]).into_owned();
            Ok(Some((path, out[end + 1..].to_vec())))
        })
    }

    fn services<'a>(&'a self, c: &'a Config) -> BoxFuture<'a, Result<Vec<Instance>, String>> {
        Box::pin(async move {
            if c.services.is_empty() {
                return Ok(Vec::new());
            }
            // Without any session tmux has no server and fails: every instance is stopped.
            let app = spec::app_name(&c.id);
            let (_, out, _) = self
                .run_as(&app, &self.user(), &services::LIST_COMMAND, &[], None)
                .await?;
            Ok(services::instances(c, &String::from_utf8_lossy(&out)))
        })
    }

    fn service<'a>(
        &'a self,
        c: &'a Config,
        service: &'a str,
        folder: Option<&'a str>,
        start: bool,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let app = spec::app_name(&c.id);
            let s = c
                .services
                .iter()
                .find(|s| s.id == service)
                .ok_or_else(|| format!("no service {service:?}"))?;
            let _ = self
                .run_as(
                    &app,
                    &self.user(),
                    &services::stop_command(service, folder),
                    &[],
                    None,
                )
                .await;
            if start {
                let cmd = services::start_command(c, s, folder);
                let (code, _, err) = self.run_as(&app, &self.user(), &cmd, &[], None).await?;
                if code != 0 {
                    return Err(format!("the service did not start: {}", err.trim()));
                }
            }
            Ok(())
        })
    }

    fn run<'a>(
        &'a self,
        id: &'a str,
        argv: Vec<String>,
        root: bool,
    ) -> BoxFuture<'a, Result<(i32, String), String>> {
        Box::pin(async move {
            let user = if root { "0:0".to_owned() } else { self.user() };
            let (code, out, err) = self
                .run_as(&spec::app_name(id), &user, &argv, &[], None)
                .await?;
            Ok((code, format!("{}{err}", String::from_utf8_lossy(&out))))
        })
    }

    fn exec<'a>(&'a self, id: &'a str, e: Exec) -> BoxFuture<'a, Result<Started, String>> {
        Box::pin(async move {
            let cmd = e
                .cmd
                .unwrap_or_else(|| vec!["sh".into(), "-c".into(), LOGIN_SHELL.into()]);
            let user = if e.root {
                "0:0".to_owned()
            } else {
                format!("{}:{}", self.ids.uid, self.ids.gid)
            };
            let mut env = vec![format!(
                "TERM={}",
                e.term.as_deref().unwrap_or("xterm-256color")
            )];
            env.extend(e.env);
            let mut spec = json!({
                "AttachStdin": true,
                "AttachStdout": true,
                "AttachStderr": true,
                "Tty": e.tty,
                "Cmd": cmd,
                "User": user,
                "Env": env,
            });
            if let Some(dir) = e.workdir {
                spec["WorkingDir"] = json!(dir);
            }
            let exec = self
                .client
                .exec_create(&spec::app_name(id), &spec)
                .await
                .map_err(|m| {
                    if m.contains("404") {
                        format!("{id} is not running")
                    } else {
                        m
                    }
                })?;
            let stream = self.client.exec_start(&exec, e.tty, e.rows, e.cols).await?;
            Ok(Started { exec, stream })
        })
    }

    fn exec_wait<'a>(&'a self, exec: &'a str) -> BoxFuture<'a, Result<i32, String>> {
        Box::pin(async move {
            // The stream ends a moment before Podman records the exit code.
            for _ in 0..50 {
                let state = self.client.exec_inspect(exec).await?;
                if !state.running {
                    return Ok(state.exit_code);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err("the command did not report its exit code".to_owned())
        })
    }

    fn exec_resize<'a>(
        &'a self,
        exec: &'a str,
        rows: u16,
        cols: u16,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move { self.client.exec_resize(exec, rows, cols).await })
    }
}

/// The sandbox id of an app container or sidecar name, and whether it is the sidecar.
fn part_of(name: &str) -> Option<(String, bool)> {
    if let Some(id) = sandbox_of(name) {
        return Some((id, false));
    }
    let id = name
        .trim_start_matches('/')
        .strip_prefix("cardea-")?
        .strip_suffix("-netguard")?;
    (!id.is_empty()).then(|| (id.to_owned(), true))
}

/// Writes `input` to a non-TTY exec stream, then reads the stream to its end: frames `[stream, 0, 0, 0, size (u32,
/// big endian)] payload`.
/// The error of an Untrusted start that found no free sub-IDs.
fn untrusted_limit_message(sub_ids: u64, ids: Ids) -> String {
    let limit = spec::untrusted_limit(sub_ids, ids);
    format!(
        "limit reached: at most {limit} Untrusted sandboxes can run at once on this computer (each needs {} of \
         your {sub_ids} sub-UIDs, see /etc/subuid); stop one, then start this one again",
        spec::ISOLATED_IDS
    )
}

fn demux_all(
    mut s: std::os::unix::net::UnixStream,
    input: Option<Vec<u8>>,
) -> Result<(Vec<u8>, Vec<u8>), String> {
    use std::io::Write;
    if let Some(input) = input {
        s.write_all(&input)
            .map_err(|e| format!("cannot write to a command: {e}"))?;
        s.shutdown(std::net::Shutdown::Write)
            .map_err(|e| e.to_string())?;
    }
    // Installs (cargo, npm) can stay silent for minutes; a dead command still ends the read.
    s.set_read_timeout(Some(Duration::from_secs(30 * 60)))
        .map_err(|e| e.to_string())?;
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let mut head = [0u8; 8];
    loop {
        match s.read_exact(&mut head) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(format!("lost a command's output: {e}")),
        }
        let len = u32::from_be_bytes([head[4], head[5], head[6], head[7]]) as usize;
        let mut payload = vec![0u8; len];
        s.read_exact(&mut payload)
            .map_err(|e| format!("lost a command's output: {e}"))?;
        if head[0] == 2 { &mut err } else { &mut out }.extend_from_slice(&payload);
    }
    Ok((out, err))
}

/// The sandbox id of an app container name (`cardea-<id>-app`, with or without Podman's leading `/`).
fn sandbox_of(name: &str) -> Option<String> {
    let id = name
        .trim_start_matches('/')
        .strip_prefix("cardea-")?
        .strip_suffix("-app")?;
    (!id.is_empty()).then(|| id.to_owned())
}

/// `STEP 3/7: RUN …` in a build output.
fn build_step(line: &str) -> Option<(u32, u32)> {
    let (n, m) = line
        .strip_prefix("STEP ")?
        .split_once(':')?
        .0
        .split_once('/')?;
    Some((n.parse().ok()?, m.parse().ok()?))
}

fn random_hex() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|e| format!("cannot read /dev/urandom: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// A tar archive (ustar) of small regular files.
fn tar(files: &[(&str, String)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, content) in files {
        let mut h = [0u8; 512];
        let mut put = |at: usize, value: &[u8]| h[at..at + value.len()].copy_from_slice(value);
        put(0, name.as_bytes());
        put(100, b"0000644\0");
        put(108, b"0000000\0");
        put(116, b"0000000\0");
        put(124, format!("{:011o}\0", content.len()).as_bytes());
        put(136, b"00000000000\0");
        put(148, b"        ");
        put(156, b"0");
        put(257, b"ustar\0");
        put(263, b"00");
        let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
        h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        out.extend_from_slice(&h);
        out.extend_from_slice(content.as_bytes());
        out.resize(out.len().div_ceil(512) * 512, 0);
    }
    out.resize(out.len() + 1024, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untrusted_limit_is_explained() {
        let m = untrusted_limit_message(
            65536,
            Ids {
                uid: 1000,
                gid: 1000,
            },
        );
        assert!(
            m.starts_with("limit reached: at most 15 Untrusted sandboxes"),
            "{m}"
        );
        assert!(m.contains("4096 of your 65536 sub-UIDs"), "{m}");
    }

    #[test]
    fn names_steps_and_ids() {
        assert_eq!(sandbox_of("/cardea-claude-app").as_deref(), Some("claude"));
        assert_eq!(sandbox_of("cardea-my-box-app").as_deref(), Some("my-box"));
        assert_eq!(sandbox_of("cardea-claude-detect"), None);
        assert_eq!(sandbox_of("other"), None);
        assert_eq!(part_of("/cardea-box-netguard"), Some(("box".into(), true)));
        assert_eq!(part_of("cardea-box-app"), Some(("box".into(), false)));
        assert_eq!(build_step("STEP 3/7: RUN [\"sh\"]"), Some((3, 7)));
        assert_eq!(build_step("--> abc"), None);
        assert_eq!(random_hex().unwrap().len(), 32);
    }

    #[test]
    fn tar_is_readable_by_tar() {
        let t = tempfile::tempdir().unwrap();
        let archive = t.path().join("c.tar");
        std::fs::write(
            &archive,
            tar(&[
                ("Containerfile", "FROM x\n".into()),
                ("add-user.sh", "y".repeat(600)),
            ]),
        )
        .unwrap();
        let Ok(out) = std::process::Command::new("tar")
            .arg("-tvf")
            .arg(&archive)
            .output()
        else {
            return; // no tar on this machine
        };
        let list = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            list.contains("7 ") && list.contains("Containerfile"),
            "{list}"
        );
        assert!(
            list.contains("600 ") && list.contains("add-user.sh"),
            "{list}"
        );
    }
}
