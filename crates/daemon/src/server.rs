//! The control socket: `$XDG_RUNTIME_DIR/cardea/control.sock`, directory `0700`, socket `0600`, and only the
//! user's own processes are served (peer UID check, security R1).

use std::fs::{self, DirBuilder, Permissions};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::Path;
use std::sync::Arc;

use cardea_proto::{ErrorCode, Event, Incoming, Method, PROTOCOL_VERSION, Request, Response};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast;

use crate::daemon::{Daemon, Reply, Sub, fail};

/// The UID of this process.
pub fn own_uid() -> std::io::Result<u32> {
    Ok(fs::metadata("/proc/self")?.uid())
}

/// Whether a peer may use the socket: only the daemon's own user.
pub fn authorized(peer_uid: u32, own_uid: u32) -> bool {
    peer_uid == own_uid
}

/// Creates the runtime directory (`0700`) and binds the socket (`0600`). Fails if a daemon already answers.
pub fn bind(runtime: &Path) -> Result<UnixListener, String> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(runtime)
        .map_err(|e| format!("cannot create {}: {e}", runtime.display()))?;
    let meta = fs::metadata(runtime).map_err(|e| e.to_string())?;
    let uid = own_uid().map_err(|e| e.to_string())?;
    if meta.uid() != uid {
        return Err(format!("{} belongs to another user", runtime.display()));
    }
    fs::set_permissions(runtime, Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    let socket = runtime.join("control.sock");
    if socket.exists() {
        if std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            return Err("a Cardea daemon is already running".into());
        }
        fs::remove_file(&socket)
            .map_err(|e| format!("cannot remove the stale {}: {e}", socket.display()))?;
    }
    let listener = UnixListener::bind(&socket)
        .map_err(|e| format!("cannot listen on {}: {e}", socket.display()))?;
    fs::set_permissions(&socket, Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    Ok(listener)
}

/// Serves connections until the listener fails. `own_uid` is the only UID served.
pub async fn serve(listener: UnixListener, daemon: Arc<Daemon>, own_uid: u32) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let daemon = daemon.clone();
                tokio::spawn(async move {
                    if let Err(e) = connection(stream, daemon, own_uid).await {
                        eprintln!("cardea daemon: connection: {e}");
                    }
                });
            }
            Err(e) => eprintln!("cardea daemon: accept: {e}"),
        }
    }
}

async fn send<T: serde::Serialize>(
    w: &mut (impl AsyncWriteExt + Unpin),
    msg: &T,
) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(msg).expect("protocol types serialize");
    line.push(b'\n');
    w.write_all(&line).await
}

/// Waits for the next event of a subscription; never returns without one.
async fn next_event(sub: &mut Option<Sub>) -> Event {
    loop {
        let Some(s) = sub else {
            return std::future::pending().await;
        };
        let (rx, only): (&mut broadcast::Receiver<Event>, Option<&str>) = match s {
            Sub::All(rx) => (rx, None),
            Sub::Logs(id, rx) => (rx, Some(id.as_str())),
        };
        match rx.recv().await {
            Ok(event) => match (only, &event) {
                (None, _) => return event,
                (Some(id), Event::Log { id: eid, .. }) if eid == id => return event,
                _ => {}
            },
            // A slow client misses events rather than slowing the daemon.
            Err(broadcast::error::RecvError::Lagged(_)) => {}
            Err(broadcast::error::RecvError::Closed) => *sub = None,
        }
    }
}

async fn connection(stream: UnixStream, daemon: Arc<Daemon>, own_uid: u32) -> std::io::Result<()> {
    let peer = stream.peer_cred()?.uid();
    let (reader, mut writer) = stream.into_split();
    if !authorized(peer, own_uid) {
        let error = fail(ErrorCode::Forbidden, "this daemon only serves its own user");
        return send(&mut writer, &Response::Err { id: None, error }).await;
    }
    let mut lines = BufReader::new(reader).lines();
    let mut greeted = false;
    let mut sub: Option<Sub> = None;
    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line? else { return Ok(()) };
                if line.trim().is_empty() {
                    continue;
                }
                let request: Request = match serde_json::from_str(&line) {
                    Ok(r) => r,
                    Err(e) => {
                        let error = fail(ErrorCode::BadRequest, format!("not a valid request: {e}"));
                        send(&mut writer, &Response::Err { id: None, error }).await?;
                        continue;
                    }
                };
                let reply: Reply = match request.method {
                    Method::Hello { version } if version == PROTOCOL_VERSION => {
                        greeted = true;
                        Ok(serde_json::to_value(daemon.hello()).expect("serializable"))
                    }
                    Method::Hello { version } => Err(fail(
                        ErrorCode::Version,
                        format!("protocol {version} is not supported: the daemon speaks protocol {PROTOCOL_VERSION}; \
                                 restart the daemon after an update"),
                    )),
                    _ if !greeted => Err(fail(ErrorCode::Version, "send hello first")),
                    method => {
                        let (reply, new_sub) = daemon.handle(method).await;
                        if new_sub.is_some() {
                            sub = new_sub;
                        }
                        reply
                    }
                };
                let response = match reply {
                    Ok(result) => Response::Ok { id: request.id, result },
                    Err(error) => Response::Err { id: Some(request.id), error },
                };
                send(&mut writer, &response).await?;
            }
            event = next_event(&mut sub) => {
                send(&mut writer, &Incoming::Event { event }).await?;
            }
        }
    }
}

/// Runs the daemon until SIGINT or SIGTERM, then removes the socket.
pub async fn run(daemon: Arc<Daemon>, listener: UnixListener) -> Result<(), String> {
    let uid = own_uid().map_err(|e| e.to_string())?;
    let socket = daemon.paths().socket();
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|e| e.to_string())?;
    tokio::select! {
        () = serve(listener, daemon, uid) => {}
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
    let _ = fs::remove_file(socket);
    Ok(())
}
