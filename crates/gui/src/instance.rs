//! One window at a time: the first instance listens on `gui.sock` (in the runtime folder, `0700`); a second
//! launch connects to it, which asks the first one to show itself, and exits.

use std::fs::DirBuilder;
use std::io::ErrorKind;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Mutex;

use cardea_daemon::paths::Paths;
use iced::futures::channel::mpsc;
use iced::futures::{SinkExt, Stream};

/// The listener claimed in `main`, taken by the subscription.
static LISTENER: Mutex<Option<UnixListener>> = Mutex::new(None);

/// Claims the single instance. `Ok(false)`: another instance runs and was asked to show itself.
pub fn claim() -> Result<bool, String> {
    let paths = Paths::from_env()?;
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&paths.runtime)
        .map_err(|e| format!("cannot create {}: {e}", paths.runtime.display()))?;
    let socket = paths.runtime.join("gui.sock");
    match UnixStream::connect(&socket) {
        Ok(_) => return Ok(false),
        // A socket left by a crash.
        Err(e) if e.kind() == ErrorKind::ConnectionRefused => {
            let _ = std::fs::remove_file(&socket);
        }
        Err(_) => {}
    }
    let listener = UnixListener::bind(&socket)
        .map_err(|e| format!("cannot bind {}: {e}", socket.display()))?;
    *LISTENER.lock().expect("listener lock") = Some(listener);
    Ok(true)
}

/// One item each time another launch asks this window to show itself.
pub fn requests() -> impl Stream<Item = ()> {
    iced::stream::channel(4, async |mut out: mpsc::Sender<()>| {
        let Some(std) = LISTENER.lock().expect("listener lock").take() else {
            return;
        };
        let listener = std
            .set_nonblocking(true)
            .and_then(|()| tokio::net::UnixListener::from_std(std));
        let Ok(listener) = listener else { return };
        while listener.accept().await.is_ok() {
            if out.send(()).await.is_err() {
                return;
            }
        }
    })
}
