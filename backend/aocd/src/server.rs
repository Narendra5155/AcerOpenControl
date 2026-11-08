//! JSON-lines Unix socket server. One request object per line, one response
//! object per line. See docs/PROTOCOL.md.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::auth;
use crate::service::Service;

const MAX_LINE: usize = 64 * 1024;
static NEXT_CONN: AtomicU64 = AtomicU64::new(1);

pub fn bind(path: &Path) -> std::io::Result<UnixListener> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let _ = std::fs::remove_file(path);
    let l = UnixListener::bind(path)?;
    // Anyone may connect; auth::Access gates changes per request.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o666))?;
    Ok(l)
}

pub fn respond(service: &Mutex<Service>, line: &str, access: auth::Access, conn: u64) -> Value {
    let req: Value = match serde_json::from_str(line) {
        Ok(v @ Value::Object(_)) => v,
        Ok(_) => return json!({"ok": false, "error": "request must be a JSON object"}),
        Err(_) => return json!({"ok": false, "error": "invalid JSON"}),
    };
    let cmd = req.get("cmd").and_then(Value::as_str).unwrap_or("");
    if !auth::allowed(access, cmd) {
        let error = if access == auth::Access::FanLease {
            "permission denied: this account may only borrow the fans (lease_users)"
        } else {
            "permission denied: changing settings needs an administrator account \
             (groups: sudo, admin, wheel or acer-open-control)"
        };
        return json!({"ok": false, "code": "forbidden", "error": error});
    }
    service.lock().unwrap_or_else(|p| p.into_inner()).handle_from(&req, conn)
}

fn client(service: Arc<Mutex<Service>>, stream: UnixStream) {
    let conn = NEXT_CONN.fetch_add(1, Ordering::Relaxed);
    let lease_users = service.lock().unwrap_or_else(|p| p.into_inner()).cfg.lease_users.clone();
    let access = auth::peer_access(&stream, &lease_users);
    serve_lines(&service, stream, access, conn);
    // Whatever ended the connection (EOF, error, crash of the client), a fan
    // lease it held must not outlive it.
    service.lock().unwrap_or_else(|p| p.into_inner()).connection_closed(conn);
}

fn serve_lines(service: &Mutex<Service>, stream: UnixStream, access: auth::Access, conn: u64) {
    let Ok(mut out) = stream.try_clone() else { return };
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.by_ref().take(MAX_LINE as u64).read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        if !line.ends_with('\n') && line.len() >= MAX_LINE {
            return;
        }
        let resp = respond(service, line.trim(), access, conn);
        if out.write_all(format!("{resp}\n").as_bytes()).is_err() {
            return;
        }
    }
}

pub fn serve(listener: UnixListener, service: Arc<Mutex<Service>>) {
    for stream in listener.incoming().flatten() {
        let s = service.clone();
        let _ = std::thread::Builder::new().name("client".into()).spawn(move || client(s, stream));
    }
}
