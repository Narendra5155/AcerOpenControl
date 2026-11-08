//! Async client for the aocd socket, keeping the one persistent connection the
//! fan lease is tied to. If aocd restarts, the connection is re-opened and the
//! lease and the last requested duties are restored transparently.

use serde_json::{json, Value};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};

pub const HOLDER: &str = "CoolerControl";
const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub enum AocdError {
    /// aocd unreachable or the connection broke.
    Unavailable(String),
    /// aocd answered `{"ok": false}`.
    Refused { message: String, forbidden: bool },
}

impl std::fmt::Display for AocdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AocdError::Unavailable(m) => write!(f, "aocd unavailable: {m}"),
            AocdError::Refused { message, .. } => write!(f, "aocd: {message}"),
        }
    }
}

struct Conn {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
}

impl Conn {
    async fn open(path: &PathBuf) -> std::io::Result<Conn> {
        let s = timeout(TIMEOUT, UnixStream::connect(path))
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "connect timed out"))??;
        let (r, w) = s.into_split();
        Ok(Conn { reader: BufReader::new(r), writer: w })
    }

    async fn send(&mut self, req: &Value) -> std::io::Result<Value> {
        let io = async {
            self.writer.write_all(format!("{req}\n").as_bytes()).await?;
            let mut line = String::new();
            if self.reader.read_line(&mut line).await? == 0 {
                return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "aocd closed the connection"));
            }
            serde_json::from_str(&line).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
        };
        timeout(TIMEOUT, io).await.map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "aocd timed out"))?
    }
}

/// What the plugin has asked for, so it can be restored on reconnect.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Desired {
    pub claimed: bool,
    /// fan index -> duty (None = BIOS)
    pub duties: Vec<Option<u8>>,
}

pub struct Aocd {
    path: PathBuf,
    inner: Mutex<(Option<Conn>, Desired)>,
}

fn check(v: Value) -> Result<Value, AocdError> {
    if v.get("ok") == Some(&Value::Bool(true)) {
        Ok(v)
    } else {
        Err(AocdError::Refused {
            message: v.get("error").and_then(Value::as_str).unwrap_or("request failed").to_string(),
            forbidden: v.get("code").and_then(Value::as_str) == Some("forbidden"),
        })
    }
}

impl Aocd {
    pub fn new(path: impl Into<PathBuf>) -> Aocd {
        Aocd { path: path.into(), inner: Mutex::new((None, Desired::default())) }
    }

    /// Send one request; reconnects once (restoring the lease) if the
    /// connection was lost.
    pub async fn call(&self, req: Value) -> Result<Value, AocdError> {
        let mut g = self.inner.lock().await;
        let mut last_err = String::new();
        for _ in 0..2 {
            if g.0.is_none() {
                match Conn::open(&self.path).await {
                    Ok(mut c) => {
                        let desired = g.1.clone();
                        if desired.claimed {
                            if let Err(e) = restore(&mut c, &desired).await {
                                log::warn!("could not restore fan control after reconnect: {e}");
                            }
                        }
                        g.0 = Some(c);
                    }
                    Err(e) => return Err(AocdError::Unavailable(e.to_string())),
                }
            }
            let conn = g.0.as_mut().expect("connected above");
            match conn.send(&req).await {
                Ok(v) => return check(v),
                Err(e) => {
                    log::debug!("aocd connection lost: {e}");
                    last_err = e.to_string();
                    g.0 = None;
                }
            }
        }
        Err(AocdError::Unavailable(last_err))
    }

    pub async fn claim(&self) -> Result<(), AocdError> {
        self.call(json!({"cmd": "claim_fans", "holder": HOLDER})).await?;
        self.inner.lock().await.1.claimed = true;
        Ok(())
    }

    /// Set one fan (claiming first if needed). `None` hands the fan to the BIOS.
    pub async fn set_fan(&self, fan: usize, duty: Option<u8>, fan_count: usize) -> Result<(), AocdError> {
        if !self.inner.lock().await.1.claimed {
            self.claim().await?;
        }
        self.call(json!({"cmd": "set_fan_duty", "fan": fan, "duty": duty})).await?;
        let mut g = self.inner.lock().await;
        if g.1.duties.len() < fan_count {
            g.1.duties.resize(fan_count, None);
        }
        if let Some(d) = g.1.duties.get_mut(fan) {
            *d = duty;
        }
        Ok(())
    }

    /// Give the fans back to AcerOpenControl's own fan mode.
    pub async fn release(&self) -> Result<(), AocdError> {
        let claimed = self.inner.lock().await.1.claimed;
        if claimed {
            self.call(json!({"cmd": "release_fans"})).await?;
        }
        self.inner.lock().await.1 = Desired::default();
        Ok(())
    }

    pub async fn desired(&self) -> Desired {
        self.inner.lock().await.1.clone()
    }
}

async fn restore(c: &mut Conn, d: &Desired) -> Result<(), AocdError> {
    let r = c.send(&json!({"cmd": "claim_fans", "holder": HOLDER})).await;
    check(r.map_err(|e| AocdError::Unavailable(e.to_string()))?)?;
    for (fan, duty) in d.duties.iter().enumerate() {
        if duty.is_some() {
            let r = c.send(&json!({"cmd": "set_fan_duty", "fan": fan, "duty": duty})).await;
            check(r.map_err(|e| AocdError::Unavailable(e.to_string()))?)?;
        }
    }
    log::info!("fan control restored after reconnecting to aocd");
    Ok(())
}
