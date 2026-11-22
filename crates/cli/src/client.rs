//! Blocking client of the control socket. Starts the daemon when none answers.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, ErrorKind, IoSliceMut, Write};
use std::mem::MaybeUninit;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::Duration;

use cardea_daemon::paths::Paths;
use cardea_proto::{
    Error, ErrorCode, Event, Hello, Incoming, Method, PROTOCOL_VERSION, Request, Response,
};
use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags};
use serde::de::DeserializeOwned;

pub struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    next: u64,
    events: VecDeque<Event>,
    pub hello: Hello,
}

impl Client {
    pub fn connect(paths: &Paths) -> Result<Client, String> {
        let socket = paths.socket();
        let stream = match UnixStream::connect(&socket) {
            Ok(s) => s,
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) => {
                start_daemon()?;
                wait_for(&socket)?
            }
            Err(e) => {
                return Err(format!(
                    "cannot reach the daemon at {}: {e}",
                    socket.display()
                ));
            }
        };
        let reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
        let hello = Hello {
            version: 0,
            daemon_version: String::new(),
            engine_error: None,
        };
        let mut client = Client {
            reader,
            writer: stream,
            next: 0,
            events: VecDeque::new(),
            hello,
        };
        client.hello = client
            .call(Method::Hello {
                version: PROTOCOL_VERSION,
            })
            .map_err(|e| e.message)?;
        Ok(client)
    }

    fn read(&mut self) -> Result<Incoming, String> {
        let mut line = String::new();
        let n = self
            .reader
            .read_line(&mut line)
            .map_err(|e| format!("lost the daemon: {e}"))?;
        if n == 0 {
            return Err("the daemon closed the connection".into());
        }
        serde_json::from_str(&line).map_err(|e| format!("unexpected message from the daemon: {e}"))
    }

    pub fn call<T: DeserializeOwned>(&mut self, method: Method) -> Result<T, Error> {
        self.send(method)?;
        loop {
            match self.read().map_err(local)? {
                Incoming::Event { event } => self.events.push_back(event),
                Incoming::Response(r) => return answer(r),
            }
        }
    }

    /// A call whose answer carries a file descriptor (`exec`). Only on a connection with no subscription.
    pub fn call_fd<T: DeserializeOwned>(
        &mut self,
        method: Method,
    ) -> Result<(T, Option<OwnedFd>), Error> {
        assert!(
            self.reader.buffer().is_empty(),
            "nothing may be pending before a descriptor"
        );
        self.send(method)?;
        let (mut line, mut fd) = (Vec::new(), None);
        while !line.ends_with(b"\n") {
            let mut buf = [0u8; 4096];
            let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
            let mut control = RecvAncillaryBuffer::new(&mut space);
            let got = rustix::net::recvmsg(
                self.reader.get_ref(),
                &mut [IoSliceMut::new(&mut buf)],
                &mut control,
                RecvFlags::CMSG_CLOEXEC,
            )
            .map_err(|e| local(format!("lost the daemon: {e}")))?;
            for message in control.drain() {
                if let RecvAncillaryMessage::ScmRights(fds) = message {
                    fd = fd.or(fds.into_iter().next());
                }
            }
            if got.bytes == 0 {
                return Err(local("the daemon closed the connection".into()));
            }
            line.extend_from_slice(&buf[..got.bytes]);
        }
        match serde_json::from_slice(&line) {
            Ok(Incoming::Response(r)) => answer(r).map(|t| (t, fd)),
            Ok(Incoming::Event { .. }) => Err(local("unexpected event from the daemon".into())),
            Err(e) => Err(local(format!("unexpected message from the daemon: {e}"))),
        }
    }

    fn send(&mut self, method: Method) -> Result<(), Error> {
        self.next += 1;
        let mut line = serde_json::to_vec(&Request {
            id: self.next,
            method,
        })
        .expect("serializable");
        line.push(b'\n');
        self.writer
            .write_all(&line)
            .map_err(|e| local(format!("lost the daemon: {e}")))
    }

    /// The next event, after `subscribe` or `logs` with `follow`.
    pub fn event(&mut self) -> Result<Event, String> {
        if let Some(e) = self.events.pop_front() {
            return Ok(e);
        }
        loop {
            if let Incoming::Event { event } = self.read()? {
                return Ok(event);
            }
        }
    }
}

fn local(message: String) -> Error {
    Error {
        code: ErrorCode::Internal,
        message,
    }
}

fn answer<T: DeserializeOwned>(r: Response) -> Result<T, Error> {
    match r {
        Response::Ok { result, .. } => {
            serde_json::from_value(result).map_err(|e| local(format!("unexpected answer: {e}")))
        }
        Response::Err { error, .. } => Err(error),
    }
}

fn start_daemon() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    Command::new(exe)
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // Out of the terminal's process group: Ctrl+C in this terminal does not stop it.
        .process_group(0)
        .spawn()
        .map_err(|e| format!("cannot start the daemon: {e}"))?;
    eprintln!("Started the Cardea daemon.");
    Ok(())
}

fn wait_for(socket: &std::path::Path) -> Result<UnixStream, String> {
    for _ in 0..50 {
        std::thread::sleep(Duration::from_millis(100));
        if let Ok(s) = UnixStream::connect(socket) {
            return Ok(s);
        }
    }
    Err("the daemon did not start; run `cardea daemon` to see why".into())
}
