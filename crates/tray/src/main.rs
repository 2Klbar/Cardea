//! `cardea-tray`: the tray icon (StatusNotifierItem). A client of the daemon's control socket like the manager:
//! it follows the daemon's events, shows the state of the sandboxes, and sends their actions.

mod model;

use std::collections::HashMap;
use std::future::poll_fn;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cardea_daemon::paths::Paths;
use cardea_daemon::terminal;
use cardea_proto::{
    ErrorCode, Event, Incoming, Method, PROTOCOL_VERSION, Request, Response, Status, Summary,
};
use ksni::menu::{StandardItem, SubMenu};
use ksni::{Icon, MenuItem, Status as TrayStatus, ToolTip, TrayMethods};
use model::{Act, Item, Look, Note, World};
use resvg::{tiny_skia, usvg};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;

struct Tray {
    world: World,
    icons: Icons,
    tx: mpsc::UnboundedSender<Act>,
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "cardea".into()
    }
    fn title(&self) -> String {
        "Cardea".into()
    }
    fn status(&self) -> TrayStatus {
        match model::look(&self.world) {
            // Leaves the collapsed part of the tray.
            Look::Error => TrayStatus::NeedsAttention,
            Look::Idle => TrayStatus::Passive,
            _ => TrayStatus::Active,
        }
    }
    fn icon_pixmap(&self) -> Vec<Icon> {
        self.icons.get(model::look(&self.world))
    }
    fn attention_icon_pixmap(&self) -> Vec<Icon> {
        self.icons.get(Look::Error)
    }
    fn tool_tip(&self) -> ToolTip {
        let (title, description) = model::tooltip(&self.world);
        ToolTip {
            title,
            description,
            ..Default::default()
        }
    }
    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.send(Act::Manager);
    }
    fn menu(&self) -> Vec<MenuItem<Self>> {
        model::menu(&self.world).into_iter().map(item).collect()
    }
}

fn item(i: Item) -> MenuItem<Tray> {
    // In menu labels a single `_` marks the access key.
    let label = |l: String| l.replace('_', "__");
    match i {
        Item::Do(l, act) => StandardItem {
            label: label(l),
            enabled: act.is_some(),
            activate: Box::new(move |t: &mut Tray| {
                if let Some(a) = &act {
                    let _ = t.tx.send(a.clone());
                }
            }),
            ..Default::default()
        }
        .into(),
        Item::Sub(l, items) => SubMenu {
            label: label(l),
            submenu: items.into_iter().map(item).collect(),
            ..Default::default()
        }
        .into(),
        Item::Separator => MenuItem::Separator,
    }
}

/// The four tray icons, rendered from `assets/logo/tray` for the panel's background.
struct Icons(HashMap<&'static str, Vec<Icon>>);

impl Icons {
    fn new(dark: bool) -> Icons {
        macro_rules! svg {
            ($state:literal) => {
                if dark {
                    include_str!(concat!(
                        "../../../assets/logo/tray/cardea-tray-",
                        $state,
                        "-dark.svg"
                    ))
                } else {
                    include_str!(concat!(
                        "../../../assets/logo/tray/cardea-tray-",
                        $state,
                        "-light.svg"
                    ))
                }
            };
        }
        let sources = [
            ("idle", svg!("idle")),
            ("running", svg!("running")),
            ("busy", svg!("busy")),
            ("error", svg!("error")),
        ];
        Icons(
            sources
                .into_iter()
                .map(|(k, s)| {
                    (
                        k,
                        [22, 32, 48, 64]
                            .iter()
                            .filter_map(|px| render(s, *px))
                            .collect(),
                    )
                })
                .collect(),
        )
    }

    fn get(&self, look: Look) -> Vec<Icon> {
        let key = match look {
            Look::Idle => "idle",
            Look::Running => "running",
            Look::Busy => "busy",
            Look::Error => "error",
        };
        self.0.get(key).cloned().unwrap_or_default()
    }
}

/// An SVG as a StatusNotifierItem pixmap: ARGB32, network byte order, not premultiplied.
fn render(svg: &str, px: u32) -> Option<Icon> {
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).ok()?;
    let size = tree.size();
    let scale = px as f32 / size.width().max(size.height());
    let mut pixmap = tiny_skia::Pixmap::new(px, px)?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let data = pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.alpha(), c.red(), c.green(), c.blue()]
        })
        .collect();
    Some(Icon {
        width: px as i32,
        height: px as i32,
        data,
    })
}

/// A notification's buttons: label and what it does.
type Buttons = Vec<(String, Act)>;

/// Desktop notifications with buttons (`org.freedesktop.Notifications`).
#[derive(Clone)]
struct Notifier {
    bus: zbus::Connection,
    actions: Arc<Mutex<HashMap<u32, Buttons>>>,
}

const NOTIFY: &str = "org.freedesktop.Notifications";
const NOTIFY_PATH: &str = "/org/freedesktop/Notifications";

impl Notifier {
    async fn new(bus: zbus::Connection, tx: mpsc::UnboundedSender<Act>) -> Notifier {
        let n = Notifier {
            bus,
            actions: Arc::default(),
        };
        let listener = n.clone();
        tokio::spawn(async move {
            let Ok(proxy) = zbus::Proxy::new(&listener.bus, NOTIFY, NOTIFY_PATH, NOTIFY).await
            else {
                return;
            };
            let Ok(mut signals) = proxy.receive_signal("ActionInvoked").await else {
                return;
            };
            while let Some(msg) = poll_fn(|cx| {
                zbus::export::futures_core::Stream::poll_next(Pin::new(&mut signals), cx)
            })
            .await
            {
                let Ok((id, key)) = msg.body().deserialize::<(u32, String)>() else {
                    continue;
                };
                let act = listener
                    .actions
                    .lock()
                    .expect("actions lock")
                    .get(&id)
                    .and_then(|a| {
                        a.iter()
                            .find(|(k, _)| *k == key)
                            .map(|(_, act)| act.clone())
                    });
                if let Some(act) = act {
                    let _ = tx.send(act);
                }
            }
        });
        n
    }

    async fn show(&self, note: &Note) {
        let mut actions = Vec::new();
        for (label, _) in &note.actions {
            actions.push(label.as_str());
            actions.push(label.as_str());
        }
        let hints: HashMap<&str, zbus::zvariant::Value<'_>> = HashMap::new();
        let body = (
            "Cardea",
            0u32,
            "cardea",
            note.title.as_str(),
            note.body.as_str(),
            actions,
            hints,
            -1i32,
        );
        match self
            .bus
            .call_method(Some(NOTIFY), NOTIFY_PATH, Some(NOTIFY), "Notify", &body)
            .await
        {
            Ok(reply) => {
                if let Ok(id) = reply.body().deserialize::<u32>() {
                    self.actions
                        .lock()
                        .expect("actions lock")
                        .insert(id, note.actions.clone());
                }
            }
            Err(e) => eprintln!("cardea-tray: notification not shown: {e}"),
        }
    }
}

/// The panel is dark: the tool theme from the settings, else the desktop's colour scheme (XDG portal).
async fn dark_panel(bus: &zbus::Connection, theme: Option<&str>) -> bool {
    match theme {
        Some("dark") => return true,
        Some("light") => return false,
        _ => {}
    }
    let read = bus.call_method(
        Some("org.freedesktop.portal.Desktop"),
        "/org/freedesktop/portal/desktop",
        Some("org.freedesktop.portal.Settings"),
        "Read",
        &("org.freedesktop.appearance", "color-scheme"),
    );
    // A desktop without the portal must not hold the tray back.
    let reply = tokio::time::timeout(Duration::from_secs(2), read)
        .await
        .ok()
        .and_then(Result::ok);
    // The answer is a variant holding a variant holding 1 for dark.
    reply
        .and_then(|r| r.body().deserialize::<zbus::zvariant::OwnedValue>().ok())
        .is_some_and(|v| format!("{v:?}").contains("U32(1)"))
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("cardea-tray: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let paths = Paths::from_env()?;
    let bus = zbus::Connection::session()
        .await
        .map_err(|e| format!("no session bus: {e}"))?;
    let settings = load_settings(&paths).await;
    let dark = dark_panel(&bus, settings.as_ref().and_then(|s| s["theme"].as_str())).await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let notifier = Notifier::new(bus, tx.clone()).await;
    let tray = Tray {
        world: World::default(),
        icons: Icons::new(dark),
        tx,
    };
    if Conn::open(&paths).await.is_err() {
        // A session without `graphical-session.target` has no daemon yet: this one gets the session's display.
        use std::os::unix::process::CommandExt;
        let _ = std::process::Command::new(sibling("cardea"))
            .arg("daemon")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .spawn();
    }
    // At login the panel may come after this: the item registers whenever a tray host appears.
    let handle = tray
        .assume_sni_available(true)
        .spawn()
        .await
        .map_err(|e| format!("cannot show the tray icon (is there a tray host?): {e}"))?;
    loop {
        if let Err(e) = session(&paths, &handle, &mut rx, &notifier).await {
            eprintln!("cardea-tray: {e}");
        }
        handle.update(|t| t.world = World::default()).await;
        // Actions while the daemon is away: the manager and Quit still work.
        let wait = tokio::time::sleep(Duration::from_secs(2));
        tokio::pin!(wait);
        loop {
            tokio::select! {
                () = &mut wait => break,
                Some(act) = rx.recv() => {
                    if !local(&act, None) {
                        eprintln!("cardea-tray: the daemon is not running");
                    }
                }
            }
        }
    }
}

/// The tool settings, read once through a short connection.
async fn load_settings(paths: &Paths) -> Option<Value> {
    let mut c = Conn::open(paths).await.ok()?;
    c.send(Method::Settings {}).await.ok()?;
    loop {
        match c.next().await.ok()? {
            Incoming::Response(Response::Ok { result, .. }) => return Some(result),
            Incoming::Response(Response::Err { .. }) => return None,
            Incoming::Event { .. } => {}
        }
    }
}

/// Actions done here without the daemon; `false` when it needs the daemon.
fn local(act: &Act, terminal_setting: Option<&[String]>) -> bool {
    let gui = |args: &[&str]| {
        if let Err(e) = std::process::Command::new(sibling("cardea-gui"))
            .args(args)
            .spawn()
        {
            eprintln!("cardea-tray: cannot open the manager: {e}");
        }
    };
    match act {
        Act::Manager => gui(&[]),
        Act::Details(id) => gui(&["--sandbox", id]),
        Act::Terminal(argv) => {
            let mut full = vec![terminal::cardea().display().to_string()];
            full.extend(argv.iter().cloned());
            if let Err(e) = terminal::open(terminal_setting, &full) {
                eprintln!("cardea-tray: {e}");
            }
        }
        Act::Quit => std::process::exit(0),
        _ => return false,
    }
    true
}

/// A program next to this one, else from `PATH`.
fn sibling(name: &str) -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join(name)))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| name.into())
}

/// A JSON-lines connection to the control socket.
struct Conn {
    lines: tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    writer: tokio::net::unix::OwnedWriteHalf,
    next: u64,
}

impl Conn {
    async fn open(paths: &Paths) -> Result<Conn, String> {
        let stream = UnixStream::connect(paths.socket())
            .await
            .map_err(|e| format!("no daemon: {e}"))?;
        let (r, writer) = stream.into_split();
        let mut c = Conn {
            lines: BufReader::new(r).lines(),
            writer,
            next: 0,
        };
        c.send(Method::Hello {
            version: PROTOCOL_VERSION,
        })
        .await?;
        match c.next().await? {
            Incoming::Response(Response::Ok { .. }) => Ok(c),
            Incoming::Response(Response::Err { error, .. }) => Err(error.message),
            Incoming::Event { .. } => Err("unexpected event".into()),
        }
    }

    async fn send(&mut self, method: Method) -> Result<u64, String> {
        self.next += 1;
        let mut line = serde_json::to_vec(&Request {
            id: self.next,
            method,
        })
        .expect("serializable");
        line.push(b'\n');
        self.writer
            .write_all(&line)
            .await
            .map_err(|e| format!("lost the daemon: {e}"))?;
        Ok(self.next)
    }

    async fn next(&mut self) -> Result<Incoming, String> {
        let line = self
            .lines
            .next_line()
            .await
            .map_err(|e| e.to_string())?
            .ok_or("the daemon closed the connection")?;
        serde_json::from_str(&line).map_err(|e| format!("unexpected message: {e}"))
    }
}

/// What a request's answer is for.
enum Then {
    List,
    Status(String),
    Services(String),
    Settings,
    /// A user action on this sandbox: errors are notified.
    Action(String),
}

async fn session(
    paths: &Paths,
    handle: &ksni::Handle<Tray>,
    rx: &mut mpsc::UnboundedReceiver<Act>,
    notifier: &Notifier,
) -> Result<(), String> {
    let mut c = Conn::open(paths).await?;
    let mut pending: HashMap<u64, Then> = HashMap::new();
    let mut world = World {
        connected: true,
        ..World::default()
    };
    let mut terminal_setting: Option<Vec<String>> = None;
    c.send(Method::Subscribe {}).await?;
    pending.insert(c.send(Method::List {}).await?, Then::List);
    pending.insert(c.send(Method::Settings {}).await?, Then::Settings);
    handle.update(|t| t.world = world.clone()).await;
    loop {
        tokio::select! {
            msg = c.next() => {
                match msg? {
                    Incoming::Event { event } => {
                        for note in model::notes(&world, &event) {
                            notifier.show(&note).await;
                        }
                        let running_now = match &event {
                            Event::Changed { sandbox } | Event::Added { sandbox } => (sandbox.state == cardea_core::state::State::Running).then(|| sandbox.id.clone()),
                            _ => None,
                        };
                        if let Some(id) = model::apply(&mut world, event) {
                            pending.insert(c.send(Method::Status { id: id.clone() }).await?, Then::Status(id));
                        }
                        if let Some(id) = running_now.filter(|id| !world.services.contains_key(id)) {
                            pending.insert(c.send(Method::Services { id: id.clone() }).await?, Then::Services(id));
                        }
                    }
                    Incoming::Response(Response::Ok { id, result }) => match pending.remove(&id) {
                        Some(Then::List) => {
                            world.sandboxes = serde_json::from_value::<Vec<Summary>>(result).unwrap_or_default();
                            for s in world.sandboxes.clone() {
                                pending.insert(c.send(Method::Status { id: s.id.clone() }).await?, Then::Status(s.id.clone()));
                                if s.state == cardea_core::state::State::Running {
                                    pending.insert(c.send(Method::Services { id: s.id.clone() }).await?, Then::Services(s.id));
                                }
                            }
                        }
                        Some(Then::Status(sid)) => {
                            if let Ok(s) = serde_json::from_value::<Status>(result) {
                                world.configs.insert(sid, s.config);
                            }
                        }
                        Some(Then::Services(sid)) => {
                            if let Ok(list) = serde_json::from_value(result) {
                                world.services.insert(sid, list);
                            }
                        }
                        Some(Then::Settings) => {
                            if let Some(t) = result.get("terminal").and_then(|t| serde_json::from_value(t.clone()).ok()) {
                                terminal_setting = t;
                            }
                        }
                        Some(Then::Action(_)) | None => {}
                    },
                    Incoming::Response(Response::Err { id, error }) => {
                        if let Some(Then::Action(sid)) = id.and_then(|id| pending.remove(&id)) {
                            let note = if error.code == ErrorCode::PasswordRequired {
                                model::password_note(&world, &sid)
                            } else {
                                Note { title: "Cardea".into(), body: error.message, actions: vec![("Details".into(), Act::Details(sid))] }
                            };
                            notifier.show(&note).await;
                        }
                    }
                }
                handle.update(|t| t.world = world.clone()).await;
            }
            Some(act) = rx.recv() => {
                if local(&act, terminal_setting.as_deref()) {
                    continue;
                }
                let (sid, method) = match act {
                    Act::Start(id) => (id.clone(), Method::Start { id, password: None }),
                    Act::Stop(id) => (id.clone(), Method::Stop { id }),
                    Act::Launch(id, app) => (id.clone(), Method::Launch { id, app, args: Vec::new(), url: None, password: None }),
                    Act::Service { id, service, folder, start } => (id.clone(), Method::Service { id, service, folder, start }),
                    _ => continue,
                };
                pending.insert(c.send(method).await?, Then::Action(sid));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_render_for_both_panels() {
        for dark in [false, true] {
            let icons = Icons::new(dark);
            for look in [Look::Idle, Look::Running, Look::Busy, Look::Error] {
                let set = icons.get(look);
                assert_eq!(set.len(), 4, "{look:?}");
                assert_eq!(set[0].data.len(), 22 * 22 * 4);
                // Something is drawn.
                assert!(set[3].data.chunks(4).any(|p| p[0] > 0));
            }
        }
    }
}
