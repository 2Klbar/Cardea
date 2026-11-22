//! Thin typed client for the libpod REST API on the user's `podman.sock`: only the endpoints Cardea uses. One
//! HTTP/1.1 connection per request; events and stats are streamed as one JSON object per line.

use std::collections::HashMap;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::net::UnixStream;

/// Oldest API Cardea needs (Podman 4.4); newer Podman versions accept it.
const API: &str = "/v4.0.0/libpod";

#[derive(Clone, Debug)]
pub struct Client {
    socket: PathBuf,
}

impl Client {
    pub fn new(socket: impl Into<PathBuf>) -> Client {
        Client {
            socket: socket.into(),
        }
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// `GET /info`: version, rootless ID mappings, network helpers.
    pub async fn info(&self) -> Result<Info, String> {
        self.json(Method::GET, "/info", None).await
    }

    /// `GET /events`: every Podman event from now on.
    pub async fn events(&self) -> Result<Lines<Event>, String> {
        self.stream(Method::GET, "/events?stream=true", None).await
    }

    /// `GET /containers/stats`: resource use of every running container, once a second.
    pub async fn stats(&self) -> Result<Lines<StatsReport>, String> {
        self.stream(
            Method::GET,
            "/containers/stats?stream=true&interval=1",
            None,
        )
        .await
    }

    pub async fn image_exists(&self, name: &str) -> Result<bool, String> {
        match self
            .send(Method::GET, &format!("/images/{name}/exists"), None)
            .await
        {
            Ok(_) => Ok(true),
            Err(f) if f.status == Some(404) => Ok(false),
            Err(f) => Err(f.message),
        }
    }

    /// `POST /images/pull`: the pull's output, line by line.
    pub async fn pull(&self, reference: &str) -> Result<Lines<Output>, String> {
        let path = format!("/images/pull?reference={}", encode(reference));
        self.stream(Method::POST, &path, None).await
    }

    /// `POST /build` from a tar context holding a `Containerfile`: the build's output, line by line.
    /// `fresh`: no cached layer, so every step runs again (packages updated, repository keys checked again).
    pub async fn build(
        &self,
        tag: &str,
        context: Vec<u8>,
        fresh: bool,
    ) -> Result<Lines<Output>, String> {
        let path = format!(
            "/build?t={}&dockerfile=Containerfile&layers=true&rm=true&nocache={fresh}",
            encode(tag)
        );
        self.stream(Method::POST, &path, Some(("application/x-tar", context)))
            .await
    }

    /// `DELETE /images/{name}`; a missing image is not an error.
    pub async fn remove_image(&self, name: &str) -> Result<(), String> {
        self.quiet(Method::DELETE, &format!("/images/{name}?force=true"))
            .await
    }

    /// `POST /containers/create` from a libpod spec; returns the container id.
    pub async fn create_container(&self, spec: &Value) -> Result<String, String> {
        let created: Created = self
            .json(Method::POST, "/containers/create", Some(spec))
            .await?;
        Ok(created.id)
    }

    pub async fn start_container(&self, name: &str) -> Result<(), String> {
        self.ok(Method::POST, &format!("/containers/{name}/start"))
            .await
    }

    /// Waits until the container stops; returns its exit code.
    pub async fn wait(&self, name: &str) -> Result<i64, String> {
        self.json(Method::POST, &format!("/containers/{name}/wait"), None)
            .await
    }

    /// `DELETE /containers/{name}`, with its anonymous volumes; a missing container is not an error.
    pub async fn remove_container(&self, name: &str) -> Result<(), String> {
        self.quiet(
            Method::DELETE,
            &format!("/containers/{name}?force=true&v=true"),
        )
        .await
    }

    /// The running containers.
    pub async fn containers(&self) -> Result<Vec<Listed>, String> {
        self.json(Method::GET, "/containers/json", None).await
    }

    pub async fn inspect(&self, name: &str) -> Result<Inspected, String> {
        self.json(Method::GET, &format!("/containers/{name}/json"), None)
            .await
    }

    /// `POST /pods/create` from a libpod pod spec.
    pub async fn create_pod(&self, spec: &Value) -> Result<(), String> {
        let _: Created = self.json(Method::POST, "/pods/create", Some(spec)).await?;
        Ok(())
    }

    pub async fn start_pod(&self, name: &str) -> Result<(), String> {
        self.ok(Method::POST, &format!("/pods/{name}/start")).await
    }

    /// Stops the pod's containers, killing them after `timeout` seconds.
    pub async fn stop_pod(&self, name: &str, timeout: u32) -> Result<(), String> {
        self.ok(Method::POST, &format!("/pods/{name}/stop?t={timeout}"))
            .await
    }

    /// `DELETE /pods/{name}`; a missing pod is not an error.
    pub async fn remove_pod(&self, name: &str) -> Result<(), String> {
        self.quiet(Method::DELETE, &format!("/pods/{name}?force=true"))
            .await
    }

    /// `POST /containers/{name}/exec`; returns the exec id.
    pub async fn exec_create(&self, container: &str, spec: &Value) -> Result<String, String> {
        let created: Created = self
            .json(
                Method::POST,
                &format!("/containers/{container}/exec"),
                Some(spec),
            )
            .await?;
        Ok(created.id)
    }

    /// Starts an exec and returns the connection carrying its input and output. Without a TTY the output is
    /// multiplexed (8-byte frame headers, stream 1 = stdout, 2 = stderr).
    pub async fn exec_start(
        &self,
        id: &str,
        tty: bool,
        rows: u16,
        cols: u16,
    ) -> Result<std::os::unix::net::UnixStream, String> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut s = self.connect().await?;
        let body =
            serde_json::json!({ "Detach": false, "Tty": tty, "h": rows, "w": cols }).to_string();
        let request = format!(
            "POST {API}/exec/{id}/start HTTP/1.1\r\nHost: podman\r\nContent-Type: application/json\r\n\
             Connection: Upgrade\r\nUpgrade: tcp\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let io = |e: std::io::Error| format!("Podman exec: {e}");
        s.write_all(request.as_bytes()).await.map_err(io)?;
        // Byte by byte, so that no byte of the stream that follows is taken.
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if s.read(&mut byte).await.map_err(io)? == 0 || head.len() > 16 * 1024 {
                return Err("Podman closed the exec connection".into());
            }
            head.push(byte[0]);
        }
        let status = String::from_utf8_lossy(&head)
            .lines()
            .next()
            .unwrap_or("")
            .to_owned();
        if !(status.starts_with("HTTP/1.1 101") || status.starts_with("HTTP/1.1 200")) {
            return Err(format!("Podman refused the exec: {status}"));
        }
        let s = s.into_std().map_err(io)?;
        s.set_nonblocking(false).map_err(io)?;
        Ok(s)
    }

    /// Starts an exec in the background: Podman answers at once, the command goes on.
    pub async fn exec_detach(&self, id: &str) -> Result<(), String> {
        let body = serde_json::json!({ "Detach": true, "Tty": false });
        let body = Some(("application/json", body.to_string().into_bytes()));
        collect(
            self.send(Method::POST, &format!("/exec/{id}/start"), body)
                .await?,
        )
        .await
        .map(drop)
    }

    pub async fn exec_inspect(&self, id: &str) -> Result<ExecState, String> {
        self.json(Method::GET, &format!("/exec/{id}/json"), None)
            .await
    }

    pub async fn exec_resize(&self, id: &str, rows: u16, cols: u16) -> Result<(), String> {
        self.ok(
            Method::POST,
            &format!("/exec/{id}/resize?h={rows}&w={cols}"),
        )
        .await
    }

    async fn json<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<T, String> {
        let body = body.map(|v| ("application/json", v.to_string().into_bytes()));
        let bytes = collect(self.send(method, path, body).await?).await?;
        serde_json::from_slice(&bytes)
            .map_err(|e| format!("Podman sent an unexpected answer to {path}: {e}"))
    }

    async fn ok(&self, method: Method, path: &str) -> Result<(), String> {
        collect(self.send(method, path, None).await?)
            .await
            .map(drop)
    }

    /// Like [`Client::ok`], but a missing object is not an error.
    async fn quiet(&self, method: Method, path: &str) -> Result<(), String> {
        match self.send(method, path, None).await {
            Ok(r) => collect(r).await.map(drop),
            Err(f) if f.status == Some(404) => Ok(()),
            Err(f) => Err(f.message),
        }
    }

    async fn stream<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<(&str, Vec<u8>)>,
    ) -> Result<Lines<T>, String> {
        Ok(Lines {
            body: self.send(method, path, body).await?.into_body(),
            buf: Vec::new(),
            item: PhantomData,
        })
    }

    async fn connect(&self) -> Result<UnixStream, String> {
        UnixStream::connect(&self.socket)
            .await
            .map_err(|e| format!("cannot reach Podman at {}: {e}", self.socket.display()))
    }

    /// Sends a request; a non-2xx answer (304 "already done" aside) becomes Podman's error message.
    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<(&str, Vec<u8>)>,
    ) -> Result<Response<Incoming>, Failure> {
        // Podman's database is briefly locked under load: a read is safe to ask again.
        let mut tries = 0;
        loop {
            match self.send_once(method.clone(), path, body.clone()).await {
                Err(f) if method == Method::GET && tries < 5 && busy(&f.message) => {
                    tries += 1;
                    tokio::time::sleep(std::time::Duration::from_millis(200 * tries)).await;
                }
                r => return r,
            }
        }
    }

    async fn send_once(
        &self,
        method: Method,
        path: &str,
        body: Option<(&str, Vec<u8>)>,
    ) -> Result<Response<Incoming>, Failure> {
        let failure = |message: String| Failure {
            status: None,
            message,
        };
        let stream = self.connect().await.map_err(failure)?;
        let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|e| failure(format!("Podman: {e}")))?;
        // Drives the connection until the response body is read or dropped.
        tokio::spawn(conn);
        let mut request = Request::builder()
            .method(method)
            .uri(format!("{API}{path}"))
            .header("Host", "podman");
        let bytes = match body {
            Some((kind, bytes)) => {
                request = request.header("Content-Type", kind);
                bytes
            }
            None => Vec::new(),
        };
        let request = request
            .body(Full::new(Bytes::from(bytes)))
            .map_err(|e| failure(e.to_string()))?;
        let response = sender
            .send_request(request)
            .await
            .map_err(|e| failure(format!("Podman: {e}")))?;
        let status = response.status();
        if status.is_success() || status == StatusCode::NOT_MODIFIED {
            return Ok(response);
        }
        let body = collect(response).await.unwrap_or_default();
        #[derive(Deserialize)]
        struct Message {
            message: String,
        }
        let message = serde_json::from_slice::<Message>(&body)
            .map(|f| f.message)
            .unwrap_or_else(|_| String::from_utf8_lossy(&body).trim().to_owned());
        Err(Failure {
            status: Some(status.as_u16()),
            message: format!("Podman answered {status} to {path}: {message}"),
        })
    }
}

/// Podman's SQLite database was busy: the request did nothing.
fn busy(message: &str) -> bool {
    message.contains("database is locked")
}

struct Failure {
    status: Option<u16>,
    message: String,
}

impl From<Failure> for String {
    fn from(f: Failure) -> String {
        f.message
    }
}

async fn collect(response: Response<Incoming>) -> Result<Bytes, String> {
    Ok(response
        .into_body()
        .collect()
        .await
        .map_err(|e| format!("Podman: {e}"))?
        .to_bytes())
}

/// Percent-encodes a query value.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// A streamed answer: one JSON object per line.
pub struct Lines<T> {
    body: Incoming,
    buf: Vec<u8>,
    item: PhantomData<fn() -> T>,
}

impl<T: DeserializeOwned> Lines<T> {
    /// The next object; `None` when Podman closes the stream.
    pub async fn next(&mut self) -> Option<Result<T, String>> {
        loop {
            if let Some(end) = self.buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=end).collect();
                if line.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                return Some(parse(&line));
            }
            match self.body.frame().await {
                Some(Ok(frame)) => {
                    if let Ok(data) = frame.into_data() {
                        self.buf.extend_from_slice(&data);
                    }
                }
                Some(Err(e)) => return Some(Err(format!("Podman: {e}"))),
                None if self.buf.iter().all(u8::is_ascii_whitespace) => return None,
                None => return Some(parse(&std::mem::take(&mut self.buf))),
            }
        }
    }
}

fn parse<T: DeserializeOwned>(line: &[u8]) -> Result<T, String> {
    serde_json::from_slice(line).map_err(|e| format!("Podman sent an unexpected line: {e}"))
}

#[derive(Deserialize)]
struct Created {
    #[serde(rename = "Id")]
    id: String,
}

/// A line of a pull or build output.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Output {
    pub stream: Option<String>,
    pub error: Option<String>,
}

/// A container in `GET /containers/json`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Listed {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Names")]
    pub names: Vec<String>,
    #[serde(rename = "Labels")]
    pub labels: Option<HashMap<String, String>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Inspected {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Mounts")]
    pub mounts: Vec<Mount>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Mount {
    #[serde(rename = "Source")]
    pub source: String,
    #[serde(rename = "Destination")]
    pub destination: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ExecState {
    #[serde(rename = "Running")]
    pub running: bool,
    #[serde(rename = "ExitCode")]
    pub exit_code: i32,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Info {
    pub host: Host,
    pub version: Version,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Host {
    pub id_mappings: IdMappings,
    /// Absent before Podman 4.5.
    pub pasta: Option<Pasta>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct IdMappings {
    pub uidmap: Option<Vec<IdMap>>,
    pub gidmap: Option<Vec<IdMap>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct IdMap {
    pub container_id: u64,
    pub host_id: u64,
    pub size: u64,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Pasta {
    pub executable: String,
    pub version: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Version {
    #[serde(rename = "Version")]
    pub version: String,
    #[serde(rename = "APIVersion")]
    pub api_version: String,
}

/// A Podman event (Docker-compatible shape).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Event {
    /// `container`, `pod`, `image`…
    #[serde(rename = "Type")]
    pub kind: String,
    /// `start`, `died`, `remove`…
    #[serde(rename = "Action")]
    pub action: String,
    #[serde(rename = "Actor")]
    pub actor: Actor,
    /// Unix seconds.
    pub time: i64,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Actor {
    #[serde(rename = "ID")]
    pub id: String,
    /// `name`, `image`, labels…
    #[serde(rename = "Attributes")]
    pub attributes: HashMap<String, String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct StatsReport {
    #[serde(rename = "Stats")]
    pub stats: Option<Vec<Stats>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Stats {
    #[serde(rename = "ContainerID")]
    pub id: String,
    #[serde(rename = "Name")]
    pub name: String,
    /// Percent of one CPU (can exceed 100 with several CPUs).
    #[serde(rename = "CPU")]
    pub cpu: f64,
    /// Bytes.
    #[serde(rename = "MemUsage")]
    pub memory: u64,
    #[serde(rename = "MemLimit")]
    pub memory_limit: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixListener;

    /// Serves one canned HTTP answer, sent in the given pieces, and returns the request line it got.
    async fn serve(socket: &Path, pieces: Vec<String>) -> tokio::task::JoinHandle<String> {
        let listener = UnixListener::bind(socket).unwrap();
        tokio::spawn(async move {
            let (mut conn, _) = listener.accept().await.unwrap();
            let mut req = vec![0; 4096];
            let n = conn.read(&mut req).await.unwrap();
            for p in pieces {
                conn.write_all(p.as_bytes()).await.unwrap();
                conn.flush().await.unwrap();
                tokio::task::yield_now().await;
            }
            String::from_utf8_lossy(&req[..n])
                .lines()
                .next()
                .unwrap()
                .to_owned()
        })
    }

    fn chunk(s: &str) -> String {
        format!("{:x}\r\n{s}\r\n", s.len())
    }

    #[tokio::test]
    async fn info_is_parsed() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("podman.sock");
        let body = r#"{"host":{"idMappings":{"uidmap":[{"container_id":0,"host_id":1000,"size":1},
            {"container_id":1,"host_id":100000,"size":65536}]},"pasta":{"executable":"/usr/bin/pasta",
            "version":"pasta 2024"}},"version":{"Version":"5.4.2","APIVersion":"5.4.2"}}"#;
        let answer = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let server = serve(&sock, vec![answer]).await;
        let info = Client::new(&sock).info().await.unwrap();
        assert_eq!(server.await.unwrap(), "GET /v4.0.0/libpod/info HTTP/1.1");
        assert_eq!(info.version.version, "5.4.2");
        assert_eq!(info.host.id_mappings.uidmap.unwrap()[1].size, 65536);
        assert_eq!(info.host.pasta.unwrap().executable, "/usr/bin/pasta");
    }

    #[tokio::test]
    async fn a_locked_database_is_asked_again() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("podman.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let answer = |status: &str, body: &str| {
            format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
        };
        let answers = [
            answer(
                "404 Not Found",
                r#"{"message":"retrieving exec session: database is locked"}"#,
            ),
            answer("200 OK", r#"{"Running":false,"ExitCode":3}"#),
        ];
        tokio::spawn(async move {
            for a in answers {
                let (mut conn, _) = listener.accept().await.unwrap();
                let mut req = vec![0; 4096];
                let _ = conn.read(&mut req).await.unwrap();
                conn.write_all(a.as_bytes()).await.unwrap();
            }
        });
        let state = Client::new(&sock).exec_inspect("e").await.unwrap();
        assert_eq!(state.exit_code, 3);
    }

    #[tokio::test]
    async fn errors_carry_podmans_message() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("podman.sock");
        let body = r#"{"cause":"x","message":"no such thing","response":404}"#;
        let answer = format!(
            "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let _server = serve(&sock, vec![answer]).await;
        let err = Client::new(&sock).info().await.unwrap_err();
        assert!(
            err.contains("404") && err.contains("no such thing"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn missing_socket_is_a_clear_error() {
        let err = Client::new("/nonexistent/podman.sock")
            .info()
            .await
            .unwrap_err();
        assert!(
            err.starts_with("cannot reach Podman at /nonexistent/podman.sock"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn stream_lines_split_across_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("podman.sock");
        let head = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n";
        let pieces = vec![
            head.to_owned(),
            chunk(r#"{"Type":"container","Action":"start","Actor":{"ID":"ab","Attri"#),
            chunk("butes\":{\"name\":\"claude\"}},\"time\":7}\n\n{\"Type\":\"pod\","),
            chunk("\"Action\":\"died\"}\n"),
            "0\r\n\r\n".to_owned(),
        ];
        let server = serve(&sock, pieces).await;
        let mut events = Client::new(&sock).events().await.unwrap();
        let first = events.next().await.unwrap().unwrap();
        assert_eq!(
            (first.kind.as_str(), first.action.as_str(), first.time),
            ("container", "start", 7)
        );
        assert_eq!(first.actor.attributes["name"], "claude");
        let second = events.next().await.unwrap().unwrap();
        assert_eq!(
            (second.kind.as_str(), second.action.as_str()),
            ("pod", "died")
        );
        assert!(events.next().await.is_none());
        assert_eq!(
            server.await.unwrap(),
            "GET /v4.0.0/libpod/events?stream=true HTTP/1.1"
        );
    }

    #[test]
    fn stats_report_is_parsed() {
        let line = br#"{"Error":null,"Stats":[{"ContainerID":"ab","Name":"claude","CPU":12.5,
            "MemUsage":1048576,"MemLimit":8589934592,"MemPerc":0.01}]}"#;
        let report: StatsReport = parse(line).unwrap();
        let s = &report.stats.unwrap()[0];
        assert_eq!(
            (s.name.as_str(), s.cpu, s.memory),
            ("claude", 12.5, 1_048_576)
        );
    }
}
