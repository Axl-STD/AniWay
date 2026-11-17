use crate::{App, Debounce};
use aniway_proto::ipc::{self, Request};
use aniway_proto::paths;
use anyhow::{Context, Result};
use calloop::generic::Generic;
use calloop::{Interest, Mode, PostAction};
use serde_json::json;
use std::io::{ErrorKind, Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::time::Duration;

impl App {
    pub(crate) fn start_ipc(&self, listener: UnixListener) -> Result<()> {
        self.handle
            .insert_source(Generic::new(listener, Interest::READ, Mode::Level), |_, listener, app| {
                while let Ok((stream, _)) = listener.accept() {
                    app.add_client(stream);
                }
                Ok(PostAction::Continue)
            })
            .map_err(|e| e.error)?;
        Ok(())
    }

    pub(crate) fn add_client(&mut self, stream: UnixStream) {
        if stream.set_nonblocking(true).is_err() {
            return;
        }
        let mut buf = Vec::new();
        let source = Generic::new(stream, Interest::READ, Mode::Edge);
        let _ = self.handle.insert_source(source, move |_, stream, app| {
            let Ok((lines, eof)) = read_lines(stream, &mut buf) else {
                return Ok(PostAction::Remove);
            };
            let mut out: &UnixStream = stream;
            for line in lines {
                let request = ipc::parse_request(&line);
                let subscribe = matches!(request, Ok(Request::Subscribe));
                if writeln!(out, "{}", app.respond(request).to_line()).is_err() {
                    return Ok(PostAction::Remove);
                }
                if subscribe {
                    app.subscribe(stream);
                }
            }
            Ok(if eof { PostAction::Remove } else { PostAction::Continue })
        });
    }
}

const STATE_DELAY: Duration = Duration::from_millis(30);

impl App {
    /// Registers the connection and sends it the current state right away.
    fn subscribe(&mut self, stream: &UnixStream) {
        if let Ok(stream) = stream.try_clone() {
            self.subscribers.push(stream);
            self.broadcast_state();
        }
    }

    /// Schedules a state event for the subscribers; bursts of changes are coalesced.
    pub(crate) fn notify_state(&mut self) {
        self.update_tray();
        if !self.subscribers.is_empty() {
            self.debounce(Debounce::State, STATE_DELAY);
        }
    }

    /// Writes the state to every subscriber without blocking; one that cannot keep up or went away is dropped.
    pub(crate) fn broadcast_state(&mut self) {
        if self.subscribers.is_empty() {
            return;
        }
        let line = format!("{}\n", json!({ "event": "state", "data": self.state() }));
        self.subscribers.retain_mut(|stream| {
            let sent = stream.write_all(line.as_bytes()).is_ok();
            if !sent {
                let _ = stream.shutdown(Shutdown::Both);
            }
            sent
        });
    }
}

const MAX_LINE: usize = 1 << 20;

pub(crate) fn bind_socket() -> Result<UnixListener> {
    let path = paths::socket_path();
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).with_context(|| format!("binding {}", path.display()))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

/// Reads whatever is available and splits complete lines; the flag reports end of stream.
pub(crate) fn read_lines(mut stream: &UnixStream, buf: &mut Vec<u8>) -> std::io::Result<(Vec<String>, bool)> {
    let mut chunk = [0u8; 4096];
    let mut eof = false;
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => {
                eof = true;
                break;
            }
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let mut lines = Vec::new();
    while let Some(i) = buf.iter().position(|b| *b == b'\n') {
        lines.push(String::from_utf8_lossy(&buf[..i]).into_owned());
        buf.drain(..=i);
    }
    if buf.len() > MAX_LINE {
        return Err(ErrorKind::InvalidData.into());
    }
    Ok((lines, eof))
}
