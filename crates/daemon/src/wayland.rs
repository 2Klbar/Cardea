//! The Wayland filter (`cardea wayland-filter`): one process per running sandbox, between its apps and the
//! desktop's compositor. It passes every message through, except the announcements of the protocols the sandbox
//! must not have (background clipboard access, screen capture, fake input; the clipboard itself when it is off).
//! An app that binds one of them anyway is disconnected. A window's app ID that matches the `StartupWMClass` of one of
//! the sandbox's menu entries becomes that entry's name: the desktop then shows the app's own icon, where it would
//! otherwise name the window after the process it talks to, this filter (`cardea`). It reads data sent by the
//! sandbox, so it runs apart from the daemon: a fault here ends one sandbox's display, nothing else.

use std::collections::HashSet;
use std::io::{IoSlice, IoSliceMut, Write};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cardea_core::model::Clipboard;
use rustix::net::{
    RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, SendAncillaryBuffer,
    SendAncillaryMessage, SendFlags, recvmsg, sendmsg,
};

/// Never given: they read or fake what happens in your other apps.
const ALWAYS_HIDDEN: [&str; 10] = [
    "zwlr_data_control_manager_v1",
    "ext_data_control_manager_v1",
    "zwlr_screencopy_manager_v1",
    "ext_image_copy_capture_manager_v1",
    "ext_output_image_capture_source_manager_v1",
    "ext_foreign_toplevel_image_capture_source_manager_v1",
    "zkde_screencast_unstable_v1",
    "org_kde_kwin_fake_input",
    "zwp_virtual_keyboard_manager_v1",
    "zwlr_virtual_pointer_manager_v1",
];

/// The clipboard, the primary selection (middle click) and drag and drop.
const CLIPBOARD: [&str; 3] = [
    "wl_data_device_manager",
    "zwp_primary_selection_device_manager_v1",
    "gtk_primary_selection_device_manager",
];

/// libwayland sends at most 28 descriptors at once; more waiting means a hostile peer.
const MAX_FDS: usize = 28;
const MAX_PENDING_FDS: usize = 256;

pub fn hidden(clipboard: Clipboard) -> Vec<&'static str> {
    let mut h = ALWAYS_HIDDEN.to_vec();
    if clipboard == Clipboard::Off {
        h.extend(CLIPBOARD);
    }
    h
}

#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Pass,
    Drop,
    /// Sent instead of the message.
    Replace(Vec<u8>),
    /// A malformed message or a hidden protocol bound anyway: end the connection.
    Close,
}

/// The menu entry name for a window's app ID, if the sandbox has one.
type Rename = Arc<dyn Fn(&[u8]) -> Option<String> + Send + Sync>;

/// What one connection's filter remembers: its registries, the globals it hid, and the objects that lead to a
/// window's app ID (`xdg_wm_base` → `xdg_surface` → `xdg_toplevel`).
struct Filter {
    hidden: Vec<&'static str>,
    registries: HashSet<u32>,
    hidden_names: HashSet<u32>,
    rename: Option<Rename>,
    wm_bases: HashSet<u32>,
    xdg_surfaces: HashSet<u32>,
    toplevels: HashSet<u32>,
}

fn word(msg: &[u8], at: usize) -> Option<u32> {
    msg.get(at..at + 4)
        .map(|b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]]))
}

/// A string argument at `at`: its length (with the NUL), then its bytes.
fn string(msg: &[u8], at: usize) -> Option<&[u8]> {
    let len = word(msg, at)? as usize;
    let bytes = msg.get(at + 4..(at + 4).checked_add(len)?)?;
    bytes.strip_suffix(&[0])
}

/// A message with one string argument.
fn with_string(id: u32, opcode: u32, s: &str) -> Vec<u8> {
    let mut args = ((s.len() + 1) as u32).to_ne_bytes().to_vec();
    args.extend(s.as_bytes());
    args.push(0);
    while args.len() % 4 != 0 {
        args.push(0);
    }
    let mut m = id.to_ne_bytes().to_vec();
    m.extend((((8 + args.len()) as u32) << 16 | opcode).to_ne_bytes());
    m.extend(args);
    m
}

/// The length of the first message in `buf` once it is complete; `Err` when its header is invalid.
fn frame(buf: &[u8]) -> Result<Option<usize>, String> {
    let Some(head) = word(buf, 4) else {
        return Ok(None);
    };
    let size = (head >> 16) as usize;
    if size < 8 || size % 4 != 0 {
        return Err(format!("invalid message size {size}"));
    }
    Ok((buf.len() >= size).then_some(size))
}

impl Filter {
    fn new(clipboard: Clipboard, rename: Option<Rename>) -> Filter {
        Filter {
            hidden: hidden(clipboard),
            registries: HashSet::new(),
            hidden_names: HashSet::new(),
            rename,
            wm_bases: HashSet::new(),
            xdg_surfaces: HashSet::new(),
            toplevels: HashSet::new(),
        }
    }

    fn is_hidden(&self, interface: &[u8]) -> bool {
        self.hidden.iter().any(|h| h.as_bytes() == interface)
    }

    /// From an app to the compositor.
    fn request(&mut self, msg: &[u8]) -> Verdict {
        let (Some(id), Some(head)) = (word(msg, 0), word(msg, 4)) else {
            return Verdict::Close;
        };
        let opcode = head & 0xffff;
        // wl_display.get_registry(new_id)
        if id == 1 && opcode == 1 {
            let Some(registry) = word(msg, 8) else {
                return Verdict::Close;
            };
            self.registries.insert(registry);
        }
        // wl_registry.bind(name, interface, version, new_id): the compositor checks the interface matches the
        // global, so a guessed name cannot reach a hidden one under another interface.
        if opcode == 0 && self.registries.contains(&id) {
            match string(msg, 12) {
                Some(b"xdg_wm_base") => {
                    // new_id after the padded string and the version.
                    let len = word(msg, 12).unwrap_or(0) as usize;
                    if let Some(new) = word(msg, 16 + len.div_ceil(4) * 4 + 4) {
                        self.wm_bases.insert(new);
                    }
                }
                Some(interface) if !self.is_hidden(interface) => {}
                _ => return Verdict::Close,
            }
            return Verdict::Pass;
        }
        match opcode {
            // xdg_wm_base.get_xdg_surface(new_id, surface)
            2 if self.wm_bases.contains(&id) => {
                if let Some(new) = word(msg, 8) {
                    self.xdg_surfaces.insert(new);
                }
            }
            // xdg_surface.get_toplevel(new_id)
            1 if self.xdg_surfaces.contains(&id) => {
                if let Some(new) = word(msg, 8) {
                    self.toplevels.insert(new);
                }
            }
            // xdg_toplevel.set_app_id(app_id)
            3 if self.toplevels.contains(&id) => {
                if let (Some(rename), Some(app_id)) = (&self.rename, string(msg, 8))
                    && let Some(name) = rename(app_id)
                {
                    return Verdict::Replace(with_string(id, 3, &name));
                }
            }
            _ => {}
        }
        Verdict::Pass
    }

    /// From the compositor to an app.
    fn event(&mut self, msg: &[u8]) -> Verdict {
        let (Some(id), Some(head)) = (word(msg, 0), word(msg, 4)) else {
            return Verdict::Close;
        };
        // wl_display.delete_id(id): the id may come back for another kind of object.
        if id == 1 && head & 0xffff == 1 {
            if let Some(gone) = word(msg, 8) {
                self.wm_bases.remove(&gone);
                self.xdg_surfaces.remove(&gone);
                self.toplevels.remove(&gone);
            }
            return Verdict::Pass;
        }
        if !self.registries.contains(&id) {
            return Verdict::Pass;
        }
        match (head & 0xffff, word(msg, 8)) {
            // global(name, interface, version)
            (0, Some(name)) => match string(msg, 12) {
                Some(interface) if self.is_hidden(interface) => {
                    self.hidden_names.insert(name);
                    Verdict::Drop
                }
                Some(_) => Verdict::Pass,
                None => Verdict::Close,
            },
            // global_remove(name)
            (1, Some(name)) if self.hidden_names.remove(&name) => Verdict::Drop,
            _ => Verdict::Pass,
        }
    }
}

/// Reads what is there, with the descriptors that came along; 0 bytes: the peer closed.
fn receive(s: &UnixStream, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, String> {
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(MAX_FDS))];
    let mut cmsg = RecvAncillaryBuffer::new(&mut space);
    let got = recvmsg(
        s,
        &mut [IoSliceMut::new(buf)],
        &mut cmsg,
        RecvFlags::CMSG_CLOEXEC,
    )
    .map_err(|e| e.to_string())?;
    for m in cmsg.drain() {
        if let RecvAncillaryMessage::ScmRights(received) = m {
            fds.extend(received);
        }
    }
    Ok(got.bytes)
}

/// Writes `bytes` (never empty), the descriptors with the first part: the receiver queues them for the messages
/// that use them, which come in these bytes or later.
fn send(mut s: &UnixStream, bytes: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(), String> {
    let mut sent = 0;
    while !fds.is_empty() {
        let batch: Vec<_> = fds.iter().take(MAX_FDS).map(AsFd::as_fd).collect();
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(MAX_FDS))];
        let mut cmsg = SendAncillaryBuffer::new(&mut space);
        cmsg.push(SendAncillaryMessage::ScmRights(&batch));
        // One byte at least carries each batch.
        let end = if fds.len() > MAX_FDS {
            sent + 1
        } else {
            bytes.len()
        };
        sent += sendmsg(
            s,
            &[IoSlice::new(&bytes[sent..end.min(bytes.len())])],
            &mut cmsg,
            SendFlags::NOSIGNAL,
        )
        .map_err(|e| e.to_string())?;
        let n = batch.len();
        fds.drain(..n);
        if sent >= bytes.len() && !fds.is_empty() {
            return Err("more descriptors than bytes to carry them".into());
        }
    }
    s.write_all(&bytes[sent..]).map_err(|e| e.to_string())
}

/// Copies one direction until either side closes or breaks a rule.
fn pump(
    from: &UnixStream,
    to: &UnixStream,
    filter: &Mutex<Filter>,
    requests: bool,
) -> Result<(), String> {
    let mut chunk = vec![0u8; 16 * 1024];
    let mut buf = Vec::new();
    let mut fds = Vec::new();
    loop {
        let n = receive(from, &mut chunk, &mut fds)?;
        if n == 0 {
            return Ok(());
        }
        if fds.len() > MAX_PENDING_FDS {
            return Err("too many descriptors".into());
        }
        buf.extend_from_slice(&chunk[..n]);
        let mut out = Vec::with_capacity(buf.len());
        let mut used = 0;
        while let Some(len) = frame(&buf[used..])? {
            let msg = &buf[used..used + len];
            let mut f = filter.lock().map_err(|_| "filter lock")?;
            match if requests {
                f.request(msg)
            } else {
                f.event(msg)
            } {
                Verdict::Pass => out.extend_from_slice(msg),
                Verdict::Replace(m) => out.extend_from_slice(&m),
                Verdict::Drop => {}
                Verdict::Close => return Err("a hidden protocol was requested".into()),
            }
            used += len;
        }
        buf.drain(..used);
        // Descriptors wait for bytes to travel with.
        if !out.is_empty() {
            send(to, &out, &mut fds)?;
        }
    }
}

fn connection(
    app: UnixStream,
    compositor: UnixStream,
    clipboard: Clipboard,
    rename: Option<Rename>,
) {
    let filter = Arc::new(Mutex::new(Filter::new(clipboard, rename)));
    let (Ok(app2), Ok(compositor2)) = (app.try_clone(), compositor.try_clone()) else {
        return;
    };
    let f = filter.clone();
    let up = std::thread::spawn(move || {
        let r = pump(&app2, &compositor2, &f, true);
        let _ = app2.shutdown(std::net::Shutdown::Both);
        let _ = compositor2.shutdown(std::net::Shutdown::Both);
        r
    });
    if let Err(e) = pump(&compositor, &app, &filter, false) {
        eprintln!("cardea wayland-filter: {e}");
    }
    let _ = app.shutdown(std::net::Shutdown::Both);
    let _ = compositor.shutdown(std::net::Shutdown::Both);
    if let Ok(Err(e)) = up.join() {
        eprintln!("cardea wayland-filter: {e}");
    }
}

/// The name of the sandbox's menu entry (`cardea-<sandbox>-<app>`, without `.desktop`) whose `StartupWMClass` is
/// `app_id`, ignoring case.
fn menu_entry_for(applications: &Path, sandbox: &str, app_id: &[u8]) -> Option<String> {
    let app_id = std::str::from_utf8(app_id).ok()?;
    let prefix = format!("cardea-{sandbox}-");
    let owner = format!("X-Cardea-Sandbox={sandbox}");
    for entry in std::fs::read_dir(applications).ok()?.flatten() {
        let name = entry.file_name();
        let Some(stem) = name.to_str().and_then(|n| n.strip_suffix(".desktop")) else {
            continue;
        };
        if !stem.starts_with(&prefix) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let class = text.lines().any(|l| {
            l.strip_prefix("StartupWMClass=")
                .is_some_and(|w| w.eq_ignore_ascii_case(app_id))
        });
        // The prefix alone is not enough: sandbox `a` and sandbox `a-b`.
        if class && text.lines().any(|l| l == owner) {
            return Some(stem.to_owned());
        }
    }
    None
}

/// `cardea wayland-filter`: serves `listen` (readable by the sandbox's user, whatever its UID: the folder holding
/// it is yours alone) and connects each app to `upstream`, the desktop's socket. With `sandbox`, windows take the
/// name of its menu entries (see the module).
pub fn main(
    listen: &Path,
    upstream: &Path,
    clipboard: Clipboard,
    sandbox: Option<String>,
) -> Result<(), String> {
    let rename: Option<Rename> = sandbox.and_then(|id| {
        let applications: PathBuf = crate::paths::Paths::from_env().ok()?.applications;
        Some(Arc::new(move |app_id: &[u8]| menu_entry_for(&applications, &id, app_id)) as Rename)
    });
    let _ = std::fs::remove_file(listen);
    let listener = UnixListener::bind(listen)
        .map_err(|e| format!("cannot listen on {}: {e}", listen.display()))?;
    std::fs::set_permissions(listen, std::fs::Permissions::from_mode(0o666))
        .map_err(|e| e.to_string())?;
    for app in listener.incoming() {
        let app = app.map_err(|e| e.to_string())?;
        match UnixStream::connect(upstream) {
            Ok(compositor) => {
                let rename = rename.clone();
                std::thread::spawn(move || connection(app, compositor, clipboard, rename));
            }
            Err(e) => eprintln!("cardea wayland-filter: {}: {e}", upstream.display()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: u32, opcode: u32, args: &[u8]) -> Vec<u8> {
        let size = (8 + args.len()) as u32;
        let mut m = id.to_ne_bytes().to_vec();
        m.extend((size << 16 | opcode).to_ne_bytes());
        m.extend(args);
        m
    }

    fn string_arg(s: &str) -> Vec<u8> {
        let mut b = ((s.len() + 1) as u32).to_ne_bytes().to_vec();
        b.extend(s.as_bytes());
        b.push(0);
        while b.len() % 4 != 0 {
            b.push(0);
        }
        b
    }

    fn global(registry: u32, name: u32, interface: &str) -> Vec<u8> {
        let mut args = name.to_ne_bytes().to_vec();
        args.extend(string_arg(interface));
        args.extend(1u32.to_ne_bytes());
        msg(registry, 0, &args)
    }

    fn bind(registry: u32, name: u32, interface: &str) -> Vec<u8> {
        let mut args = name.to_ne_bytes().to_vec();
        args.extend(string_arg(interface));
        args.extend(1u32.to_ne_bytes());
        args.extend(9u32.to_ne_bytes());
        msg(registry, 0, &args)
    }

    #[test]
    fn hides_the_clipboard_when_off_and_background_access_always() {
        for (clipboard, data_device) in [
            (Clipboard::Shared, Verdict::Pass),
            (Clipboard::Off, Verdict::Drop),
        ] {
            let mut f = Filter::new(clipboard, None);
            // Before get_registry, id 2 is not a registry.
            assert_eq!(
                f.event(&global(2, 5, "wl_data_device_manager")),
                Verdict::Pass
            );
            assert_eq!(f.request(&msg(1, 1, &2u32.to_ne_bytes())), Verdict::Pass);
            assert_eq!(f.event(&global(2, 1, "wl_compositor")), Verdict::Pass);
            assert_eq!(
                f.event(&global(2, 5, "wl_data_device_manager")),
                data_device
            );
            assert_eq!(
                f.event(&global(2, 6, "zwlr_data_control_manager_v1")),
                Verdict::Drop
            );
            // The removal of a hidden global is hidden too, once.
            assert_eq!(f.event(&msg(2, 1, &6u32.to_ne_bytes())), Verdict::Drop);
            assert_eq!(f.event(&msg(2, 1, &6u32.to_ne_bytes())), Verdict::Pass);
            assert_eq!(f.request(&bind(2, 1, "wl_compositor")), Verdict::Pass);
            assert_eq!(
                f.request(&bind(2, 6, "zwlr_data_control_manager_v1")),
                Verdict::Close
            );
        }
    }

    #[test]
    fn windows_take_the_menu_entry_name() {
        let rename: Rename = Arc::new(|id: &[u8]| {
            (id == b"com.anthropic.Claude").then(|| "cardea-box-claude-desktop".to_owned())
        });
        let mut f = Filter::new(Clipboard::Shared, Some(rename));
        f.request(&msg(1, 1, &2u32.to_ne_bytes()));
        // bind(…, "xdg_wm_base", …) as 9, then a surface 10 and its toplevel 11.
        assert_eq!(f.request(&bind(2, 3, "xdg_wm_base")), Verdict::Pass);
        let mut args = 10u32.to_ne_bytes().to_vec();
        args.extend(5u32.to_ne_bytes());
        assert_eq!(f.request(&msg(9, 2, &args)), Verdict::Pass);
        assert_eq!(f.request(&msg(10, 1, &11u32.to_ne_bytes())), Verdict::Pass);
        let set = |s: &str| msg(11, 3, &string_arg(s));
        assert_eq!(
            f.request(&set("com.anthropic.Claude")),
            Verdict::Replace(set("cardea-box-claude-desktop"))
        );
        assert_eq!(f.request(&set("other")), Verdict::Pass);
        // Once deleted, id 11 may be anything: left alone.
        assert_eq!(f.event(&msg(1, 1, &11u32.to_ne_bytes())), Verdict::Pass);
        assert_eq!(f.request(&set("com.anthropic.Claude")), Verdict::Pass);
    }

    #[test]
    fn menu_entries_by_window_class() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, sandbox: &str, class: &str| {
            std::fs::write(
                dir.path().join(name),
                format!("[Desktop Entry]\nX-Cardea-Sandbox={sandbox}\nStartupWMClass={class}\n"),
            )
            .unwrap();
        };
        write("cardea-a-claude.desktop", "a", "com.anthropic.Claude");
        write("cardea-a-b-claude.desktop", "a-b", "Other");
        assert_eq!(
            menu_entry_for(dir.path(), "a", b"com.anthropic.claude").as_deref(),
            Some("cardea-a-claude")
        );
        assert_eq!(menu_entry_for(dir.path(), "a", b"Other"), None);
        assert_eq!(menu_entry_for(dir.path(), "z", b"Other"), None);
    }

    #[test]
    fn malformed_messages_close() {
        let mut f = Filter::new(Clipboard::Off, None);
        f.request(&msg(1, 1, &2u32.to_ne_bytes()));
        // A string longer than the message.
        let mut bad = bind(2, 1, "wl_compositor");
        bad[12] = 200;
        assert_eq!(f.request(&bad), Verdict::Close);
        assert_eq!(f.request(&msg(1, 1, &[])), Verdict::Close);
        assert!(frame(&[0, 0, 0, 0, 1, 0, 3, 0]).is_err());
        let ok = msg(3, 0, &[]);
        assert_eq!(frame(&ok[..6]), Ok(None));
        assert_eq!(frame(&ok), Ok(Some(8)));
    }

    /// Through real sockets: the app's view of the registry, and descriptors carried across.
    #[test]
    fn filters_between_sockets() {
        let (app, app_side) = UnixStream::pair().unwrap();
        let (compositor_side, compositor) = UnixStream::pair().unwrap();
        std::thread::spawn(move || connection(app_side, compositor_side, Clipboard::Off, None));
        let (mut app, mut compositor) = (app, compositor);
        app.write_all(&msg(1, 1, &2u32.to_ne_bytes())).unwrap();
        let mut got = vec![0u8; 12];
        std::io::Read::read_exact(&mut compositor, &mut got).unwrap();
        let file = std::fs::File::open("/dev/null").unwrap();
        let mut fds = vec![OwnedFd::from(file)];
        let mut events = global(2, 5, "wl_data_device_manager");
        events.extend(global(2, 1, "wl_compositor"));
        send(&compositor, &events, &mut fds).unwrap();
        let mut buf = vec![0u8; 256];
        let mut received = Vec::new();
        let n = receive(&app, &mut buf, &mut received).unwrap();
        assert_eq!(&buf[..n], &global(2, 1, "wl_compositor")[..]);
        assert_eq!(received.len(), 1);
    }
}
