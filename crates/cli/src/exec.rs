//! `cardea exec`: the daemon starts the command and hands its connection over; this side copies bytes between it
//! and the terminal. With a terminal: raw mode, and size changes forwarded to the daemon. Without: the output
//! comes multiplexed and is split back into stdout and stderr.

use std::io::{IsTerminal, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use cardea_proto::{ExecStarted, Method};
use rustix::termios::{OptionalActions, Termios, tcgetattr, tcgetwinsize, tcsetattr};

use crate::client::Client;

/// Runs the command; returns its exit code.
pub fn run(
    id: String,
    root: bool,
    workdir: Option<String>,
    cmd: Vec<String>,
    app: Option<String>,
    test_wayland: bool,
) -> Result<i32, String> {
    let (paths, mut c) = crate::connect()?;
    let tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let (rows, cols) = if tty { size() } else { (0, 0) };
    let (started, fd) = c
        .call_fd::<ExecStarted>(Method::Exec {
            id,
            cmd: (!cmd.is_empty()).then_some(cmd),
            app,
            test_wayland,
            root,
            workdir,
            tty,
            rows,
            cols,
            term: std::env::var("TERM").ok(),
        })
        .map_err(|e| e.message)?;
    let stream = UnixStream::from(fd.ok_or("the daemon sent no connection")?);
    let input = stream.try_clone().map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        let mut input = input;
        let _ = std::io::copy(&mut std::io::stdin().lock(), &mut input);
        // End of input reaches the command.
        let _ = input.shutdown(Shutdown::Write);
    });
    if tty {
        let raw = Raw::enter().map_err(|e| format!("cannot set the terminal to raw mode: {e}"))?;
        let exec = started.exec.clone();
        std::thread::spawn(move || resize(&paths, &exec, (rows, cols)));
        let result = copy(stream);
        drop(raw);
        result?;
    } else {
        demux(stream)?;
    }
    c.call(Method::ExecWait { exec: started.exec })
        .map_err(|e| e.message)
}

fn size() -> (u16, u16) {
    tcgetwinsize(std::io::stdout()).map_or((24, 80), |w| (w.ws_row, w.ws_col))
}

/// Forwards terminal size changes. shortcut: polls the size four times a second instead of handling SIGWINCH;
/// upgrade if the delay is ever noticed.
fn resize(paths: &cardea_daemon::paths::Paths, exec: &str, mut last: (u16, u16)) {
    let Ok(mut c) = Client::connect(paths) else {
        return;
    };
    // The first size may have reached the command before it had a terminal.
    let mut send = |(rows, cols): (u16, u16)| {
        c.call::<()>(Method::ExecResize {
            exec: exec.to_owned(),
            rows,
            cols,
        })
        .is_ok()
    };
    if !send(last) {
        return;
    }
    loop {
        std::thread::sleep(Duration::from_millis(250));
        let now = size();
        if now != last {
            if !send(now) {
                return;
            }
            last = now;
        }
    }
}

/// The command's output to stdout, unbuffered, until it ends.
fn copy(mut stream: UnixStream) -> Result<(), String> {
    let mut out = std::io::stdout().lock();
    let mut buf = [0u8; 8192];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => {
                if out.write_all(&buf[..n]).and_then(|()| out.flush()).is_err() {
                    return Ok(());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(format!("lost the command: {e}")),
        }
    }
}

/// Splits frames `[stream, 0, 0, 0, size (u32, big endian)] payload` into stdout (1) and stderr (2).
fn demux(stream: UnixStream) -> Result<(), String> {
    let (mut out, mut err) = (std::io::stdout().lock(), std::io::stderr().lock());
    demux_into(stream, &mut out, &mut err)
}

fn demux_into(mut r: impl Read, out: &mut impl Write, err: &mut impl Write) -> Result<(), String> {
    let mut head = [0u8; 8];
    loop {
        match r.read_exact(&mut head) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(format!("lost the command: {e}")),
        }
        let len = u32::from_be_bytes([head[4], head[5], head[6], head[7]]) as usize;
        let mut payload = vec![0u8; len];
        r.read_exact(&mut payload)
            .map_err(|e| format!("lost the command: {e}"))?;
        let w: &mut dyn Write = if head[0] == 2 { err } else { out };
        // A closed output (`| head`) is not an error of the command.
        let _ = w.write_all(&payload).and_then(|()| w.flush());
    }
}

/// The terminal in raw mode until dropped.
struct Raw(Termios);

impl Raw {
    fn enter() -> rustix::io::Result<Raw> {
        let old = tcgetattr(std::io::stdin())?;
        let mut raw = old.clone();
        raw.make_raw();
        tcsetattr(std::io::stdin(), OptionalActions::Now, &raw)?;
        Ok(Raw(old))
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        let _ = tcsetattr(std::io::stdin(), OptionalActions::Now, &self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_go_to_their_stream() {
        let mut input = Vec::new();
        for (stream, text) in [(1u8, "out1 "), (2, "err"), (1, "out2")] {
            input.extend_from_slice(&[stream, 0, 0, 0]);
            input.extend_from_slice(&(text.len() as u32).to_be_bytes());
            input.extend_from_slice(text.as_bytes());
        }
        let (mut out, mut err) = (Vec::new(), Vec::new());
        demux_into(&input[..], &mut out, &mut err).unwrap();
        assert_eq!(
            (out.as_slice(), err.as_slice()),
            (&b"out1 out2"[..], &b"err"[..])
        );
    }
}
