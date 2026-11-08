//! ACPI method calls through the `acpi_call` kernel module (`/proc/acpi/call`).
//!
//! acpi_call is one shared request/response file, so every call holds an
//! exclusive `flock` on [`LOCK_PATH`]. That serialises the daemon, the CLI and
//! the probe tools against each other.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::process::Command;

use crate::{Error, Result};

pub const PROC_CALL: &str = "/proc/acpi/call";
pub const LOCK_PATH: &str = "/run/acer-open-control-acpi.lock";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arg {
    Int(u64),
    Buf(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    None,
    Int(u64),
    Buf(Vec<u8>),
    Str(String),
    Other(String),
}

impl Value {
    /// Integer view: buffers are read little-endian (the firmware does the same
    /// when it converts a buffer argument with `ARGS = Arg2`).
    pub fn as_int(&self) -> Result<u64> {
        match self {
            Value::Int(v) => Ok(*v),
            Value::Buf(b) => {
                let mut bytes = [0u8; 8];
                for (i, x) in b.iter().take(8).enumerate() {
                    bytes[i] = *x;
                }
                Ok(u64::from_le_bytes(bytes))
            }
            other => Err(Error::Acpi(format!("expected an integer result, got {other:?}"))),
        }
    }
}

/// Anything that can evaluate ACPI methods. The real one is [`AcpiCall`];
/// tests use `fake::FakeFirmware`.
pub trait Firmware: Send + Sync {
    fn call(&self, method: &str, args: &[Arg]) -> Result<Value>;
}

pub fn encode_arg(a: &Arg) -> String {
    match a {
        Arg::Int(v) => format!("{v:#x}"),
        Arg::Buf(b) => {
            let mut s = String::with_capacity(1 + b.len() * 2);
            s.push('b');
            for x in b {
                s.push_str(&format!("{x:02x}"));
            }
            s
        }
    }
}

/// Parse what acpi_call leaves in /proc/acpi/call after a call.
pub fn parse_reply(raw: &str) -> Result<Value> {
    let t = raw.trim_matches(char::from(0)).trim();
    if t.is_empty() || t == "not called" {
        return Ok(Value::None);
    }
    if let Some(e) = t.strip_prefix("Error:") {
        return Err(Error::Acpi(e.trim().to_string()));
    }
    if let Some(h) = t.strip_prefix("0x") {
        if let Ok(v) = u64::from_str_radix(h, 16) {
            return Ok(Value::Int(v));
        }
    }
    if let Some(inner) = t.strip_prefix('{') {
        // "{0x01, 0x02}" or a truncated "{0x01, 0x02,"
        let inner = inner.trim_end_matches('}');
        let mut out = Vec::new();
        for part in inner.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            match part.strip_prefix("0x").and_then(|h| u8::from_str_radix(h, 16).ok()) {
                Some(b) => out.push(b),
                None => return Ok(Value::Other(t.to_string())),
            }
        }
        return Ok(Value::Buf(out));
    }
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        return Ok(Value::Str(t[1..t.len() - 1].to_string()));
    }
    Ok(Value::Other(t.to_string()))
}

/// Load acpi_call if /proc/acpi/call is missing.
pub fn ensure_module() -> Result<()> {
    if Path::new(PROC_CALL).exists() {
        return Ok(());
    }
    let _ = Command::new("modprobe").arg("acpi_call").status();
    if Path::new(PROC_CALL).exists() {
        Ok(())
    } else {
        Err(Error::Acpi("/proc/acpi/call is missing: install acpi-call-dkms and run 'sudo modprobe acpi_call'".into()))
    }
}

struct Flock(File);

impl Flock {
    fn acquire(path: &str) -> Result<Flock> {
        let f = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;
        // SAFETY: flock on a valid, owned fd.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(Flock(f))
    }
}

impl Drop for Flock {
    fn drop(&mut self) {
        // SAFETY: fd is still open; unlocking cannot invalidate memory.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

/// Real firmware access through /proc/acpi/call. Needs root.
#[derive(Debug, Default)]
pub struct AcpiCall;

impl Firmware for AcpiCall {
    fn call(&self, method: &str, args: &[Arg]) -> Result<Value> {
        let mut line = method.to_string();
        for a in args {
            line.push(' ');
            line.push_str(&encode_arg(a));
        }
        let _lock = Flock::acquire(LOCK_PATH)?;
        OpenOptions::new().write(true).open(PROC_CALL)?.write_all(line.as_bytes())?;
        let mut raw = String::new();
        File::open(PROC_CALL)?.read_to_string(&mut raw)?;
        parse_reply(&raw)
    }
}

/// `WMBB(0, cmd, <4-byte buffer>)`: the Clevo GCMD/SCMD dispatcher. The
/// firmware runs `SizeOf(Arg2)`, so the argument must be a buffer.
pub fn wmbb(fw: &dyn Firmware, path: &str, cmd: u32, arg: u32) -> Result<Value> {
    fw.call(path, &[Arg::Int(0), Arg::Int(cmd as u64), Arg::Buf(arg.to_le_bytes().to_vec())])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies() {
        assert_eq!(parse_reply("0x1f\0").unwrap(), Value::Int(0x1F));
        assert_eq!(parse_reply("{0x01, 0xff, 0x00, 0x10}").unwrap(), Value::Buf(vec![1, 0xFF, 0, 0x10]));
        assert_eq!(parse_reply("{0x01, 0x02,").unwrap(), Value::Buf(vec![1, 2]));
        assert_eq!(parse_reply("{}").unwrap(), Value::Buf(vec![]));
        assert_eq!(parse_reply("not called").unwrap(), Value::None);
        assert_eq!(parse_reply("\"abc\"").unwrap(), Value::Str("abc".into()));
        assert!(parse_reply("Error: AE_NOT_FOUND").is_err());
    }

    #[test]
    fn encoding() {
        assert_eq!(encode_arg(&Arg::Buf(0x8040u32.to_le_bytes().to_vec())), "b40800000");
        assert_eq!(encode_arg(&Arg::Int(0x68)), "0x68");
    }

    #[test]
    fn buffer_as_int_is_little_endian() {
        assert_eq!(Value::Buf(vec![0x40, 0x80]).as_int().unwrap(), 0x8040);
        assert!(Value::Str("x".into()).as_int().is_err());
    }
}
