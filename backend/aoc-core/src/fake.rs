//! In-memory emulation of the A715-79G's `WMBB` firmware, for tests.
//! Enabled for this crate's tests and, via the `testing` feature, for others.

use std::sync::Mutex;

use crate::acpi::{Arg, Firmware, Value};
use crate::profile::Profile;
use crate::{Error, Result};

pub const A715_TOML: &str = include_str!("../../../models/acer-aspire-a715-79g.toml");

pub fn a715() -> Profile {
    Profile::from_toml(A715_TOML).expect("shipped profile is valid")
}

#[derive(Debug)]
pub struct State {
    pub ram: [u8; 256],
    pub indx: u32,
    pub charge_stop: u8,
    pub charge_start: u8,
    pub charge_enabled: bool,
    pub cpcm: u8,
    pub webcam_on: bool,
    pub event: u8,
    pub fail: bool,
    /// (cmd, arg) for every WMBB call, plus ("path", 0) entries for other methods.
    pub calls: Vec<(u32, u32)>,
    pub other_calls: Vec<String>,
}

#[derive(Debug)]
pub struct FakeFirmware {
    pub wmbb_path: String,
    pub cpcm_path: String,
    pub state: Mutex<State>,
}

impl Default for FakeFirmware {
    fn default() -> Self {
        let mut ram = [0u8; 256];
        // duty 71/71, rpm raw 0x0393 / 0x03C8 (big-endian), as probed at idle.
        ram[0xCE] = 71;
        ram[0xCF] = 71;
        ram[0xD0..0xD4].copy_from_slice(&[0x03, 0x93, 0x03, 0xC8]);
        FakeFirmware {
            wmbb_path: r"\_SB.WMI.WMBB".into(),
            cpcm_path: r"\_SB.PC00.LPCB.EC.CPCM".into(),
            state: Mutex::new(State {
                ram,
                indx: 0,
                charge_stop: 100,
                charge_start: 95,
                charge_enabled: false,
                cpcm: 3,
                webcam_on: true,
                event: 0,
                fail: false,
                calls: Vec::new(),
                other_calls: Vec::new(),
            }),
        }
    }
}

impl FakeFirmware {
    pub fn calls(&self) -> Vec<(u32, u32)> {
        self.state.lock().unwrap().calls.clone()
    }
    pub fn last_call(&self) -> Option<(u32, u32)> {
        self.state.lock().unwrap().calls.last().copied()
    }
    pub fn clear(&self) {
        let mut s = self.state.lock().unwrap();
        s.calls.clear();
        s.other_calls.clear();
    }
    pub fn set_fail(&self, fail: bool) {
        self.state.lock().unwrap().fail = fail;
    }
}

impl Firmware for FakeFirmware {
    fn call(&self, method: &str, args: &[Arg]) -> Result<Value> {
        let mut s = self.state.lock().unwrap();
        if method == self.cpcm_path {
            return Ok(Value::Int(s.cpcm as u64));
        }
        if method != self.wmbb_path {
            s.other_calls.push(method.to_string());
            return Ok(Value::Int(0));
        }
        if s.fail {
            return Err(Error::Acpi("AE_ERROR (fake)".into()));
        }
        let cmd = match args.get(1) {
            Some(Arg::Int(c)) => *c as u32,
            _ => return Err(Error::Acpi("bad args".into())),
        };
        let arg = match args.get(2) {
            Some(Arg::Buf(b)) => {
                let mut x = [0u8; 4];
                x[..b.len().min(4)].copy_from_slice(&b[..b.len().min(4)]);
                u32::from_le_bytes(x)
            }
            _ => 0,
        };
        s.calls.push((cmd, arg));
        let echo = Value::Int(cmd as u64);
        Ok(match cmd {
            0x6B => {
                s.indx = arg;
                echo
            }
            0x73 => {
                let off = (s.indx & 0xFFFF) as usize;
                let size = 1usize << ((s.indx >> 16) & 0xFFFF);
                let mut out = vec![0u8; 4];
                out[..size].copy_from_slice(&s.ram[off..off + size]);
                Value::Buf(out)
            }
            0x68 => {
                s.ram[0xCE] = (arg & 0xFF) as u8;
                s.ram[0xCF] = ((arg >> 8) & 0xFF) as u8;
                echo
            }
            0x79 => {
                match arg >> 24 {
                    0x06 => {
                        s.charge_stop = ((arg >> 8) & 0xFF) as u8;
                        s.charge_start = (arg & 0xFF) as u8;
                    }
                    0x05 => s.charge_enabled = arg & 1 == 1,
                    0x19 => s.cpcm = (arg & 0xFF) as u8,
                    _ => {}
                }
                echo
            }
            0x77 => {
                Value::Int(((s.charge_stop as u64) << 16) | ((s.charge_start as u64) << 8) | s.charge_enabled as u64)
            }
            0x22 => {
                s.webcam_on = arg != 0;
                echo
            }
            0x46 => Value::Int(0x1234),
            0x01 => Value::Int(s.event as u64),
            _ => echo,
        })
    }
}
