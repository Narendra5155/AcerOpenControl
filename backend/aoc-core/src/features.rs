//! Non-fan firmware features: keyboard backlight, performance profile,
//! battery charge limit and webcam power. Everything is driven by the profile.

use serde::{Deserialize, Serialize};
use std::fs;

use crate::ec::Clevo;
use crate::profile::KeyboardKind;
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kbd {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub brightness: u8,
}

impl Default for Kbd {
    fn default() -> Self {
        Kbd { r: 255, g: 255, b: 255, brightness: 160 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChargeLimit {
    pub enabled: bool,
    pub start: u8,
    pub stop: u8,
}

pub fn validate_charge(enabled: bool, start: i64, stop: i64) -> Result<ChargeLimit> {
    if !(50..=100).contains(&stop) {
        return Err(Error::Invalid("stop must be 50..100 %".into()));
    }
    if !(40..stop).contains(&start) {
        return Err(Error::Invalid("start must be at least 40 % and below stop".into()));
    }
    Ok(ChargeLimit { enabled, start: start as u8, stop: stop as u8 })
}

/// True if a USB device whose product string mentions a camera is present.
pub fn camera_present() -> bool {
    fs::read_dir("/sys/bus/usb/devices")
        .map(|rd| {
            rd.filter_map(|e| e.ok()).any(|e| {
                fs::read_to_string(e.path().join("product")).map(|p| p.to_lowercase().contains("cam")).unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

pub struct Features {
    pub ec: Clevo,
    /// How to observe webcam state; replaceable in tests.
    pub camera_probe: fn() -> bool,
}

impl Features {
    pub fn new(ec: Clevo) -> Features {
        Features { ec, camera_probe: camera_present }
    }

    // -- keyboard -------------------------------------------------------------------
    pub fn kbd_colour_arg(&self, r: u8, g: u8, b: u8) -> Result<u32> {
        let k = self.ec.profile.keyboard.as_ref().ok_or(Error::Unsupported("keyboard backlight"))?;
        let (Some(prefix), Some(s)) = (k.colour_prefix, k.channel_shift) else {
            return Err(Error::Unsupported("keyboard colour"));
        };
        Ok(prefix | (r as u32) << s.red | (g as u32) << s.green | (b as u32) << s.blue)
    }

    pub fn set_kbd_brightness(&self, level: u8) -> Result<()> {
        let k = self.ec.profile.keyboard.as_ref().ok_or(Error::Unsupported("keyboard backlight"))?;
        if level > k.max_brightness {
            return Err(Error::Invalid(format!("brightness must be 0..{}", k.max_brightness)));
        }
        self.ec.set(k.cmd, k.brightness_prefix | level as u32)
    }

    pub fn set_kbd(&self, kbd: Kbd) -> Result<()> {
        let k = self.ec.profile.keyboard.as_ref().ok_or(Error::Unsupported("keyboard backlight"))?;
        if k.kind == KeyboardKind::RgbSingleZone {
            self.ec.set(k.cmd, self.kbd_colour_arg(kbd.r, kbd.g, kbd.b)?)?;
        }
        self.set_kbd_brightness(kbd.brightness)
    }

    // -- performance profile --------------------------------------------------------
    pub fn get_perf_profile(&self) -> Option<u8> {
        let p = self.ec.profile.performance.as_ref()?;
        let path = p.current_path.as_ref()?;
        let v = self.ec.call_path(path, &[]).ok()?.as_int().ok()?;
        let idx = u8::try_from(v).ok()?;
        p.by_index(idx).map(|_| idx)
    }

    pub fn set_perf_profile(&self, idx: u8) -> Result<()> {
        let p = self.ec.profile.performance.as_ref().ok_or(Error::Unsupported("performance profiles"))?;
        if p.by_index(idx).is_none() {
            return Err(Error::Invalid(format!("unknown performance profile {idx}")));
        }
        self.ec.set(p.cmd, (p.selector as u32) << 24 | idx as u32)
    }

    // -- battery ----------------------------------------------------------------------
    pub fn get_charge(&self) -> Result<ChargeLimit> {
        let b = self.ec.profile.battery.as_ref().ok_or(Error::Unsupported("battery charge limit"))?;
        let v = self.ec.get(b.read_cmd, 0)?;
        Ok(ChargeLimit { enabled: v & 0xFF != 0, start: ((v >> 8) & 0xFF) as u8, stop: ((v >> 16) & 0xFF) as u8 })
    }

    pub fn set_charge(&self, lim: ChargeLimit) -> Result<ChargeLimit> {
        let b = self.ec.profile.battery.as_ref().ok_or(Error::Unsupported("battery charge limit"))?;
        self.ec.set(b.cmd, (b.thresholds_selector as u32) << 24 | (lim.stop as u32) << 8 | lim.start as u32)?;
        self.ec.set(b.cmd, (b.enable_selector as u32) << 24 | lim.enabled as u32)?;
        self.get_charge()
    }

    // -- webcam ---------------------------------------------------------------------------
    pub fn set_webcam(&self, on: bool) -> Result<()> {
        let w = self.ec.profile.webcam.as_ref().ok_or(Error::Unsupported("webcam switch"))?;
        let off = w.off_value as u32;
        self.ec.set(w.cmd, if on { 1 - off.min(1) } else { off })
    }

    pub fn webcam_on(&self) -> bool {
        (self.camera_probe)()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{a715, FakeFirmware};
    use std::sync::Arc;

    fn setup() -> (Arc<FakeFirmware>, Features) {
        let fw = Arc::new(FakeFirmware::default());
        (fw.clone(), Features::new(Clevo::new(fw, Arc::new(a715()))))
    }

    #[test]
    fn kbd_brg_packing() {
        let (fw, f) = setup();
        f.set_kbd(Kbd { r: 0x11, g: 0x22, b: 0x33, brightness: 200 }).unwrap();
        assert_eq!(fw.calls(), vec![(0x67, 0xF000_0000 | 0x33 << 16 | 0x11 << 8 | 0x22), (0x67, 0xF400_0000 | 200)]);
    }

    #[test]
    fn charge_roundtrip() {
        let (fw, f) = setup();
        assert_eq!(f.get_charge().unwrap(), ChargeLimit { enabled: false, start: 95, stop: 100 });
        fw.clear();
        let got = f.set_charge(ChargeLimit { enabled: true, start: 75, stop: 80 }).unwrap();
        assert_eq!(got, ChargeLimit { enabled: true, start: 75, stop: 80 });
        assert_eq!(fw.calls()[0], (0x79, 0x06 << 24 | 80 << 8 | 75));
        assert_eq!(fw.calls()[1], (0x79, 0x05 << 24 | 1));
    }

    #[test]
    fn charge_validation() {
        assert!(validate_charge(true, 80, 80).is_err());
        assert!(validate_charge(true, 30, 80).is_err());
        assert!(validate_charge(true, 70, 120).is_err());
        assert!(validate_charge(true, 75, 80).is_ok());
    }

    #[test]
    fn perf() {
        let (fw, f) = setup();
        assert_eq!(f.get_perf_profile(), Some(3));
        f.set_perf_profile(2).unwrap();
        assert_eq!(fw.last_call(), Some((0x79, 0x19 << 24 | 2)));
        assert_eq!(f.get_perf_profile(), Some(2));
        assert!(f.set_perf_profile(7).is_err());
    }

    #[test]
    fn webcam() {
        let (fw, f) = setup();
        f.set_webcam(false).unwrap();
        f.set_webcam(true).unwrap();
        assert_eq!(fw.calls(), vec![(0x22, 0), (0x22, 1)]);
    }
}
