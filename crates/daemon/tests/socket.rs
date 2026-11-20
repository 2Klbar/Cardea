//! The daemon over its real socket, with the demo engine.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use cardea_daemon::daemon::Daemon;
use cardea_daemon::paths::Paths;
use cardea_daemon::server;
use cardea_engine::demo::DemoEngine;
use cardea_proto::{ErrorCode, Event, Incoming, Response};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

struct Client {
    lines: Lines<BufReader<OwnedReadHalf>>,
    writer: OwnedWriteHalf,
    next: u64,
    events: Vec<Event>,
}

impl Client {
    async fn connect(socket: &Path) -> Client {
        let (r, w) = UnixStream::connect(socket).await.unwrap().into_split();
        Client {
            lines: BufReader::new(r).lines(),
            writer: w,
            next: 0,
            events: Vec::new(),
        }
    }

    async fn read(&mut self) -> Incoming {
        let line = tokio::time::timeout(Duration::from_secs(10), self.lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        serde_json::from_str(&line).unwrap()
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Value, cardea_proto::Error> {
        self.next += 1;
        let line =
            json!({ "id": self.next, "method": method, "params": params }).to_string() + "\n";
        self.writer.write_all(line.as_bytes()).await.unwrap();
        loop {
            match self.read().await {
                Incoming::Event { event } => self.events.push(event),
                Incoming::Response(Response::Ok { result, .. }) => return Ok(result),
                Incoming::Response(Response::Err { error, .. }) => return Err(error),
            }
        }
    }

    async fn event(&mut self) -> Event {
        if !self.events.is_empty() {
            return self.events.remove(0);
        }
        match self.read().await {
            Incoming::Event { event } => event,
            other => panic!("unexpected {other:?}"),
        }
    }

    /// Waits for the end of the task of `id`; returns its error.
    async fn task_end(&mut self, id: &str) -> Option<String> {
        loop {
            if let Event::TaskEnded { id: eid, error, .. } = self.event().await {
                if eid == id {
                    return error;
                }
            }
        }
    }
}

fn paths(root: &Path) -> Paths {
    Paths {
        config: root.join("config"),
        data: root.join("data"),
        runtime: root.join("run/cardea"),
        sandboxes: root.join("Sandboxes"),
        home: root.to_path_buf(),
    }
}

/// Starts a demo daemon serving `served_uid`.
async fn start(root: &Path, served_uid: u32) -> Paths {
    let paths = paths(root);
    let listener = server::bind(&paths.runtime).unwrap();
    let daemon = Daemon::open(
        paths.clone(),
        Arc::new(DemoEngine::new(Duration::from_millis(2))),
        true,
    )
    .unwrap();
    tokio::spawn(server::serve(listener, daemon, served_uid));
    paths
}

#[tokio::test]
async fn socket_permissions_and_handshake() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = start(tmp.path(), server::own_uid().unwrap()).await;
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&paths.runtime), 0o700);
    assert_eq!(mode(&paths.socket()), 0o600);

    let mut c = Client::connect(&paths.socket()).await;
    assert_eq!(
        c.call("list", json!({})).await.unwrap_err().code,
        ErrorCode::Version
    );
    assert_eq!(
        c.call("hello", json!({ "version": 99 }))
            .await
            .unwrap_err()
            .code,
        ErrorCode::Version
    );
    let hello = c.call("hello", json!({ "version": 1 })).await.unwrap();
    assert_eq!(hello["demo"], true);
    let list = c.call("list", json!({})).await.unwrap();
    assert_eq!(list.as_array().unwrap().len(), 7);

    // A second daemon on the same socket is refused.
    assert_eq!(
        server::bind(&paths.runtime).unwrap_err(),
        "a Cardea daemon is already running"
    );
}

#[tokio::test]
async fn another_user_is_refused() {
    assert!(!server::authorized(1001, 1000));
    let tmp = tempfile::tempdir().unwrap();
    // The daemon serves another UID than ours: we are "the other user".
    let paths = start(tmp.path(), server::own_uid().unwrap() + 1).await;
    let mut c = Client::connect(&paths.socket()).await;
    match c.read().await {
        Incoming::Response(Response::Err { error, .. }) => {
            assert_eq!(error.code, ErrorCode::Forbidden)
        }
        other => panic!("{other:?}"),
    }
    assert!(
        c.lines.next_line().await.unwrap().is_none(),
        "the connection is closed"
    );
}

#[tokio::test]
async fn lifecycle_through_the_socket() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = start(tmp.path(), server::own_uid().unwrap()).await;
    let mut c = Client::connect(&paths.socket()).await;
    c.call("hello", json!({ "version": 1 })).await.unwrap();
    c.call("subscribe", json!({})).await.unwrap();

    let created = c
        .call("create", json!({ "profile": "empty", "name": "My Box" }))
        .await
        .unwrap();
    assert_eq!(created["id"], "my-box");
    assert_eq!(created["state"], "building");
    assert_eq!(c.task_end("my-box").await, None);
    let folder = paths.sandboxes.join("my-box");
    assert!(folder.join("sandbox.json").exists() && folder.join("home").is_dir());

    // Invalid and busy actions.
    let err = c.call("stop", json!({ "id": "my-box" })).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidState);
    c.call("start", json!({ "id": "my-box" })).await.unwrap();
    assert_eq!(
        c.call("start", json!({ "id": "my-box" }))
            .await
            .unwrap_err()
            .code,
        ErrorCode::InvalidState
    );
    assert_eq!(c.task_end("my-box").await, None);
    let status = c.call("status", json!({ "id": "my-box" })).await.unwrap();
    assert_eq!(status["summary"]["state"], "running");

    // Editing: invalid text is refused with its problems; a base image change needs a rebuild.
    let text = c
        .call("get_config", json!({ "id": "my-box" }))
        .await
        .unwrap();
    let bad = text
        .as_str()
        .unwrap()
        .replace("\"trust\": \"work\"", "\"trust\": \"maybe\"");
    let err = c
        .call("set_config", json!({ "id": "my-box", "config": bad }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidConfig);
    assert!(err.message.contains("trust"), "{}", err.message);
    let new = text.as_str().unwrap().replace("trixie-slim", "sid-slim");
    let saved = c
        .call("set_config", json!({ "id": "my-box", "config": new }))
        .await
        .unwrap();
    assert_eq!(saved, "rebuild_required");
    c.call("rebuild", json!({ "id": "my-box" })).await.unwrap();
    assert_eq!(c.task_end("my-box").await, None);
    let status = c.call("status", json!({ "id": "my-box" })).await.unwrap();
    assert_eq!(status["summary"]["state"], "running");
    assert_eq!(status["summary"]["flags"]["rebuild_required"], false);

    // Logs.
    let logs = c.call("logs", json!({ "id": "my-box" })).await.unwrap();
    assert!(
        logs.as_array()
            .unwrap()
            .iter()
            .any(|l| l.as_str().unwrap().ends_with("Rebuild finished"))
    );

    // Delete: refused while running, then removes the folder.
    assert_eq!(
        c.call("delete", json!({ "id": "my-box" }))
            .await
            .unwrap_err()
            .code,
        ErrorCode::InvalidState
    );
    c.call("stop", json!({ "id": "my-box" })).await.unwrap();
    assert_eq!(c.task_end("my-box").await, None);
    c.call("delete", json!({ "id": "my-box" })).await.unwrap();
    assert!(!folder.exists());
    assert_eq!(
        c.call("status", json!({ "id": "my-box" }))
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn demo_build_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = start(tmp.path(), server::own_uid().unwrap()).await;
    let mut c = Client::connect(&paths.socket()).await;
    c.call("hello", json!({ "version": 1 })).await.unwrap();
    c.call("subscribe", json!({})).await.unwrap();
    let config = json!({ "schema": 1, "id": "bad", "name": "Bad",
                         "packages": [ { "source": "system", "name": "demo-fail-x" } ] });
    c.call("create", json!({ "config": config.to_string() }))
        .await
        .unwrap();
    assert_eq!(
        c.task_end("bad").await.as_deref(),
        Some("package \"demo-fail-x\" not found")
    );
    let status = c.call("status", json!({ "id": "bad" })).await.unwrap();
    assert_eq!(status["summary"]["state"], "build_failed");
}
