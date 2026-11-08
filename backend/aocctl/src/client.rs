//! Client for the aocd socket.

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

pub const SOCKET_PATH: &str = "/run/acer-open-control/aocd.sock";

pub fn request(socket: &str, req: &Value) -> Result<Value> {
    let mut s = UnixStream::connect(socket).map_err(|e| match e.kind() {
        ErrorKind::NotFound | ErrorKind::ConnectionRefused => {
            anyhow!("aocd is not running (start it with: sudo systemctl start aocd)")
        }
        ErrorKind::PermissionDenied => anyhow!("permission denied opening {socket}"),
        _ => anyhow!("cannot reach aocd at {socket}: {e}"),
    })?;
    s.set_read_timeout(Some(Duration::from_secs(10)))?;
    s.write_all(format!("{req}\n").as_bytes())?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).context("reading reply from aocd")?;
    serde_json::from_str(&line).context("invalid reply from aocd")
}

/// Send a request and turn `{"ok": false}` into an error.
pub fn call(socket: &str, req: Value) -> Result<Value> {
    let r = request(socket, &req)?;
    if r.get("ok") == Some(&Value::Bool(true)) {
        Ok(r)
    } else {
        Err(anyhow!("{}", r.get("error").and_then(Value::as_str).unwrap_or("request failed")))
    }
}
