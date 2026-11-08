//! Clevo `WMBB` access: generic get/set, EC RAM reads, and fan control.

use serde::Serialize;
use std::sync::Arc;

use crate::acpi::{self, Firmware, Value};
use crate::profile::{Endian, FansCfg, Profile};
use crate::{Error, Result};

#[derive(Clone)]
pub struct Clevo {
    pub fw: Arc<dyn Firmware>,
    pub profile: Arc<Profile>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Fan {
    pub label: String,
    /// Current duty in percent, read back from the EC.
    pub duty: u8,
    pub rpm: u32,
}

pub fn pct_to_raw(pct: f64) -> u8 {
    (pct.clamp(0.0, 100.0) * 255.0 / 100.0).round() as u8
}

pub fn raw_to_pct(raw: u8) -> u8 {
    ((raw as f64) * 100.0 / 255.0).round() as u8
}

pub fn decode_rpm(two: [u8; 2], endian: Endian, dividend: u32) -> u32 {
    let raw = match endian {
        Endian::Big => u16::from_be_bytes(two),
        Endian::Little => u16::from_le_bytes(two),
    };
    if raw == 0 || raw == 0xFFFF {
        0
    } else {
        dividend / raw as u32
    }
}

impl Clevo {
    pub fn new(fw: Arc<dyn Firmware>, profile: Arc<Profile>) -> Clevo {
        Clevo { fw, profile }
    }

    fn path(&self) -> &str {
        &self.profile.firmware.wmbb_path
    }

    pub fn raw(&self, cmd: u32, arg: u32) -> Result<Value> {
        acpi::wmbb(self.fw.as_ref(), self.path(), cmd, arg)
    }

    /// GCMD-style read returning an integer.
    pub fn get(&self, cmd: u32, arg: u32) -> Result<u64> {
        self.raw(cmd, arg)?.as_int()
    }

    /// SCMD-style write. Success means the firmware echoes the command id;
    /// `Ones` (0xffffffff) means it could not take the EC mutex.
    pub fn set(&self, cmd: u32, arg: u32) -> Result<()> {
        let ret = self.raw(cmd, arg)?.as_int()?;
        if ret != cmd as u64 {
            return Err(Error::Firmware { cmd, ret });
        }
        Ok(())
    }

    /// Evaluate an arbitrary ACPI object (e.g. a Name like `\_SB...CPCM`).
    pub fn call_path(&self, path: &str, args: &[acpi::Arg]) -> Result<Value> {
        self.fw.call(path, args)
    }

    /// Read 1, 2 or 4 bytes of EC RAM through the firmware.
    pub fn read_ec(&self, offset: u8, size: usize) -> Result<Vec<u8>> {
        let code = match size {
            1 => 0,
            2 => 1,
            4 => 2,
            _ => return Err(Error::Invalid("EC read size must be 1, 2 or 4".into())),
        };
        if offset as usize + size > 0x100 {
            return Err(Error::Invalid("EC offset out of range".into()));
        }
        let fwc = &self.profile.firmware;
        self.set(fwc.set_index_cmd, (code << 16) | offset as u32)?;
        let bytes = match self.raw(fwc.read_ec_cmd, 0)? {
            Value::Buf(b) => b,
            v => v.as_int()?.to_le_bytes().to_vec(),
        };
        if bytes.len() < size {
            return Err(Error::Acpi("short EC read".into()));
        }
        Ok(bytes[..size].to_vec())
    }

    pub fn fans_cfg(&self) -> Result<&FansCfg> {
        self.profile.fans.as_ref().ok_or(Error::Unsupported("fan control"))
    }

    /// Set every fan's duty (percent). Missing entries reuse the first value.
    pub fn set_duty(&self, pct: &[f64]) -> Result<()> {
        let f = self.fans_cfg()?;
        let first = pct.first().copied().unwrap_or(0.0);
        let mut arg = 0u32;
        for i in 0..f.count {
            arg |= (pct_to_raw(pct.get(i).copied().unwrap_or(first)) as u32) << (8 * i);
        }
        self.set(f.set_duty_cmd, arg)
    }

    /// Hand every fan back to the EC's own automatic control.
    pub fn set_auto(&self) -> Result<()> {
        let f = self.fans_cfg()?;
        self.set(f.set_auto_cmd, f.auto_mask)
    }

    pub fn read_fans(&self) -> Result<Vec<Fan>> {
        let f = self.fans_cfg()?;
        let mut out = Vec::with_capacity(f.count);
        for i in 0..f.count {
            let duty = self.read_ec(f.duty_offsets[i], 1)?[0];
            let r = self.read_ec(f.rpm_offsets[i], 2)?;
            out.push(Fan {
                label: f.labels[i].clone(),
                duty: raw_to_pct(duty),
                rpm: decode_rpm([r[0], r[1]], f.rpm_endian, f.rpm_dividend),
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{a715, FakeFirmware};

    fn setup() -> (Arc<FakeFirmware>, Clevo) {
        let fw = Arc::new(FakeFirmware::default());
        let c = Clevo::new(fw.clone(), Arc::new(a715()));
        (fw, c)
    }

    #[test]
    fn percent_conversions() {
        assert_eq!(pct_to_raw(100.0), 255);
        assert_eq!(pct_to_raw(0.0), 0);
        assert_eq!(pct_to_raw(150.0), 255);
        assert_eq!(raw_to_pct(255), 100);
    }

    #[test]
    fn rpm() {
        assert_eq!(decode_rpm([0, 0], Endian::Big, 2156220), 0);
        assert_eq!(decode_rpm([0xFF, 0xFF], Endian::Big, 2156220), 0);
        assert_eq!(decode_rpm([0x01, 0x53], Endian::Big, 2156220), 6360);
        assert_eq!(decode_rpm([0x53, 0x01], Endian::Little, 2156220), 6360);
    }

    #[test]
    fn read_ec_sets_index_first() {
        let (fw, c) = setup();
        assert_eq!(c.read_ec(0xD0, 2).unwrap(), vec![0x03, 0x93]);
        assert_eq!(fw.calls()[0], (0x6B, (1 << 16) | 0xD0));
        assert!(c.read_ec(0xFF, 2).is_err());
        assert!(c.read_ec(0, 3).is_err());
    }

    #[test]
    fn duty_and_auto() {
        let (fw, c) = setup();
        c.set_duty(&[100.0, 0.0]).unwrap();
        assert_eq!(fw.last_call(), Some((0x68, 0x00FF)));
        c.set_duty(&[50.0]).unwrap();
        assert_eq!(fw.last_call(), Some((0x68, 0x8080)));
        c.set_auto().unwrap();
        assert_eq!(fw.last_call(), Some((0x69, 0x0F)));
    }

    #[test]
    fn fan_readback() {
        let (_fw, c) = setup();
        let fans = c.read_fans().unwrap();
        assert_eq!(fans[0], Fan { label: "CPU".into(), duty: 28, rpm: 2156220 / 0x0393 });
        assert_eq!(fans[1].label, "GPU");
    }

    #[test]
    fn set_detects_firmware_failure() {
        let (fw, c) = setup();
        fw.set_fail(true);
        assert!(c.set_auto().is_err());
    }
}
