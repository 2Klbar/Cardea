//! The connection to the daemon: one control socket connection, kept open and reopened when lost. The app
//! sends [`Call`]s; replies and events come back as [`Out`] messages.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use cardea_daemon::paths::Paths;
use cardea_proto::{Event, Hello, Incoming, Method, PROTOCOL_VERSION, Request, Response};
use iced::futures::channel::mpsc;
use iced::futures::{SinkExt, Stream, StreamExt};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// What to do with a reply.
#[derive(Clone, Debug)]
pub enum Then {
    List,
    Status(String),
    Logs(String),
    Profiles,
    Hello,
    /// A user action: an error is shown to the user.
    Action,
    Ignore,
}

#[derive(Debug)]
pub struct Call {
    pub method: Method,
    pub then: Then,
}

#[derive(Clone, Debug)]
pub enum Out {
    Connected(Hello, mpsc::UnboundedSender<Call>),
    Event(Event),
    Reply(Then, Result<Value, String>),
    Lost(String),
}

/// The connection, as a subscription stream: reconnects two seconds after a loss.
pub fn run() -> impl Stream<Item = Out> {
    iced::stream::channel(100, async |mut out: mpsc::Sender<Out>| {
        loop {
            let e = session(&mut out).await;
            if out.send(Out::Lost(e)).await.is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    })
}

/// One connection, until it fails.
async fn session(out: &mut mpsc::Sender<Out>) -> String {
    match session_inner(out).await {
        Ok(()) => "the daemon closed the connection".into(),
        Err(e) => e,
    }
}

async fn session_inner(out: &mut mpsc::Sender<Out>) -> Result<(), String> {
    let paths = Paths::from_env()?;
    let stream = connect(&paths.socket()).await?;
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    write(
        &mut writer,
        0,
        Method::Hello {
            version: PROTOCOL_VERSION,
        },
    )
    .await?;
    let hello: Hello = match read(&mut lines).await? {
        Incoming::Response(Response::Ok { result, .. }) => {
            serde_json::from_value(result).map_err(|e| format!("unexpected answer: {e}"))?
        }
        Incoming::Response(Response::Err { error, .. }) => return Err(error.message),
        Incoming::Event { .. } => return Err("unexpected event before hello".into()),
    };
    write(&mut writer, 1, Method::Subscribe {}).await?;

    let (tx, mut calls) = mpsc::unbounded::<Call>();
    out.send(Out::Connected(hello, tx))
        .await
        .map_err(|e| e.to_string())?;
    let mut pending: HashMap<u64, Then> = HashMap::from([(1, Then::Ignore)]);
    let mut next = 2;
    loop {
        tokio::select! {
            incoming = read(&mut lines) => {
                let msg = match incoming? {
                    Incoming::Event { event } => Out::Event(event),
                    Incoming::Response(Response::Ok { id, result }) => {
                        Out::Reply(pending.remove(&id).unwrap_or(Then::Ignore), Ok(result))
                    }
                    Incoming::Response(Response::Err { id, error }) => {
                        let then = id.and_then(|id| pending.remove(&id)).unwrap_or(Then::Ignore);
                        Out::Reply(then, Err(error.message))
                    }
                };
                out.send(msg).await.map_err(|e| e.to_string())?;
            }
            call = calls.next() => {
                // The app dropped its sender: it is quitting.
                let Some(call) = call else { return Ok(()) };
                pending.insert(next, call.then);
                write(&mut writer, next, call.method).await?;
                next += 1;
            }
        }
    }
}

async fn read(
    lines: &mut tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
) -> Result<Incoming, String> {
    let line = lines
        .next_line()
        .await
        .map_err(|e| format!("lost the daemon: {e}"))?
        .ok_or("the daemon closed the connection")?;
    serde_json::from_str(&line).map_err(|e| format!("unexpected message from the daemon: {e}"))
}

async fn write(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    id: u64,
    method: Method,
) -> Result<(), String> {
    let mut line = serde_json::to_vec(&Request { id, method }).expect("serializable");
    line.push(b'\n');
    writer
        .write_all(&line)
        .await
        .map_err(|e| format!("lost the daemon: {e}"))
}

/// Connects to the daemon, starting it when none answers.
async fn connect(socket: &Path) -> Result<UnixStream, String> {
    match UnixStream::connect(socket).await {
        Ok(s) => return Ok(s),
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) => {}
        Err(e) => {
            return Err(format!(
                "cannot reach the daemon at {}: {e}",
                socket.display()
            ));
        }
    }
    Command::new(cli())
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("cannot start the daemon: {e}"))?;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Ok(s) = UnixStream::connect(socket).await {
            return Ok(s);
        }
    }
    Err("the daemon did not start; run `cardea daemon` to see why".into())
}

/// The `cardea` binary: next to this one, else from `PATH`.
fn cli() -> PathBuf {
    std::env::current_exe()
        .ok()
        .map(|exe| exe.with_file_name("cardea"))
        .filter(|p| p.exists())
        .unwrap_or_else(|| "cardea".into())
}
