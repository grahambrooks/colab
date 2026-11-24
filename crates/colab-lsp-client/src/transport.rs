//! LSP base protocol: `Content-Length`-framed JSON-RPC over a child's
//! stdio, or over a Unix socket to a `colab lsp-daemon` holding one, with
//! a reader thread so every wait can time out.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

/// A running language server, or the daemon in front of one.
pub struct Transport {
    /// The server process, when this transport started it.
    child: Option<Child>,
    writer: Box<dyn Write + Send>,
    incoming: Receiver<Value>,
    /// The last lines the server wrote to stderr, when this transport
    /// started it: what explains a server that exits.
    stderr: Arc<Mutex<VecDeque<String>>>,
}

/// How many of a server's last stderr lines a transport keeps.
const STDERR_LINES: usize = 20;

/// Drain `stderr` on a thread, keeping its last [`STDERR_LINES`] lines.
/// Draining also keeps a server that logs a lot from blocking on a full pipe.
fn stderr_thread<R: Read + Send + 'static>(stderr: R) -> Arc<Mutex<VecDeque<String>>> {
    let lines = Arc::new(Mutex::new(VecDeque::new()));
    let kept = Arc::clone(&lines);
    thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            let mut kept = kept.lock().unwrap_or_else(PoisonError::into_inner);
            if kept.len() == STDERR_LINES {
                kept.pop_front();
            }
            kept.push_back(line);
        }
    });
    lines
}

/// Read frames from `reader` on a thread, into a channel.
fn reader_thread<R: Read + Send + 'static>(reader: R) -> Receiver<Value> {
    let (tx, incoming) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        while let Some(message) = read_frame(&mut reader) {
            if tx.send(message).is_err() {
                break;
            }
        }
    });
    incoming
}

impl Transport {
    /// Spawn `program args…` in `cwd`. Its stderr is drained and only its
    /// last lines kept ([`stderr_tail`](Self::stderr_tail)): servers log
    /// copiously there and nothing in it is protocol, but it is what says
    /// why a server exited.
    pub fn spawn(program: &str, args: &[String], cwd: &std::path::Path) -> std::io::Result<Self> {
        let mut child = Command::new(program)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdin = child.stdin.take().expect("stdin was piped");
        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");
        Ok(Self {
            child: Some(child),
            writer: Box::new(stdin),
            incoming: reader_thread(stdout),
            stderr: stderr_thread(stderr),
        })
    }

    /// Talk over a connected socket (a `colab lsp-daemon`).
    #[cfg(unix)]
    pub fn over(stream: std::os::unix::net::UnixStream) -> std::io::Result<Self> {
        let reader = stream.try_clone()?;
        Ok(Self {
            child: None,
            writer: Box::new(stream),
            incoming: reader_thread(reader),
            stderr: Arc::default(),
        })
    }

    /// The last non-empty lines the server wrote to stderr, joined by
    /// `; `, or `None` (a daemon connection, or a server that said nothing).
    /// Waits a moment first, so a server that just exited has been read.
    pub fn stderr_tail(&self) -> Option<String> {
        if self.child.is_some() {
            thread::sleep(Duration::from_millis(50));
        }
        let lines = self.stderr.lock().unwrap_or_else(PoisonError::into_inner);
        let kept: Vec<&str> = lines
            .iter()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();
        let tail = kept[kept.len().saturating_sub(3)..].join("; ");
        (!tail.is_empty()).then(|| tail.chars().take(500).collect())
    }

    pub fn send(&mut self, message: &Value) -> std::io::Result<()> {
        write_frame(&mut self.writer, message)
    }

    /// The next message, or `None` if `deadline` passes or the server
    /// closed its stdout.
    pub fn recv_until(&self, deadline: Instant) -> Result<Value, Closed> {
        let wait = deadline.saturating_duration_since(Instant::now());
        match self
            .incoming
            .recv_timeout(wait.max(Duration::from_millis(1)))
        {
            Ok(message) => Ok(message),
            Err(RecvTimeoutError::Timeout) => Err(Closed::TimedOut),
            Err(RecvTimeoutError::Disconnected) => Err(Closed::Exited),
        }
    }
}

/// Why no message arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Closed {
    TimedOut,
    Exited,
}

/// Write one framed message.
pub(crate) fn write_frame<W: Write + ?Sized>(
    writer: &mut W,
    message: &Value,
) -> std::io::Result<()> {
    let body = message.to_string();
    write!(writer, "Content-Length: {}\r\n\r\n{}", body.len(), body)?;
    writer.flush()
}

impl Drop for Transport {
    fn drop(&mut self) {
        let Some(child) = self.child.as_mut() else {
            return;
        };
        // `shutdown`/`exit` are sent by the session; this is the backstop
        // for a server that ignores them or a session that bailed early.
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = child.try_wait() {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Read one framed message. `None` at end of stream or on a malformed
/// frame (a server that writes junk to stdout is not one we can talk to).
pub(crate) fn read_frame<R: BufRead>(reader: &mut R) -> Option<Value> {
    let mut length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            if length.is_some() {
                break;
            }
            continue;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().ok();
        }
    }
    let mut body = vec![0u8; length?];
    reader.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_consecutive_frames_and_ignores_extra_headers() {
        let raw =
            "Content-Length: 7\r\nContent-Type: x\r\n\r\n{\"a\":1}Content-Length: 2\r\n\r\n[]";
        let mut reader = std::io::Cursor::new(raw.as_bytes());
        assert_eq!(read_frame(&mut reader), Some(serde_json::json!({"a": 1})));
        assert_eq!(read_frame(&mut reader), Some(serde_json::json!([])));
        assert_eq!(read_frame(&mut reader), None);
    }
}
