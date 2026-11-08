//! Persistent daemon settings (`/etc/acer-open-control/config.json`).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;

use crate::curve::{self, Point};
use crate::features::{ChargeLimit, Kbd};
use crate::{Error, Result};

pub const CONFIG_DIR: &str = "/etc/acer-open-control";
pub const CONFIG_PATH: &str = "/etc/acer-open-control/config.json";

pub const MODES: &[&str] = &["auto", "silent", "balanced", "performance", "max", "custom"];

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Saved {
    pub kbd: Option<Kbd>,
    pub kbd_last_on: Option<u8>,
    pub perf_profile: Option<u8>,
    pub charge: Option<ChargeLimit>,
    pub webcam: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct FeatureSettings {
    /// What the user chose; reapplied at boot and after resume.
    pub saved: Saved,
    pub hotkeys: bool,
    pub gnome_kbd_slider: bool,
    /// Extra or overriding hotkey mappings, e.g. {"0x9F": "kbd-colour-cycle"}.
    pub keymap: BTreeMap<String, String>,
}

impl Default for FeatureSettings {
    fn default() -> Self {
        FeatureSettings { saved: Saved::default(), hotkeys: true, gnome_kbd_slider: true, keymap: BTreeMap::new() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub mode: String,
    pub custom_curve: Vec<Point>,
    pub poll_interval: f64,
    pub refresh_interval: f64,
    pub hysteresis: f64,
    pub ramp_down: f64,
    /// Use this model profile id even if the machine doesn't match it.
    pub force_profile: Option<String>,
    pub features: FeatureSettings,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            mode: "balanced".into(),
            custom_curve: curve::preset("balanced").unwrap(),
            poll_interval: 1.5,
            refresh_interval: 30.0,
            hysteresis: 4.0,
            ramp_down: 5.0,
            force_profile: None,
            features: FeatureSettings::default(),
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        if !MODES.contains(&self.mode.as_str()) {
            return Err(Error::Invalid(format!("unknown mode {:?}", self.mode)));
        }
        curve::validate_points(&self.custom_curve)?;
        if !(0.5..=10.0).contains(&self.poll_interval) {
            return Err(Error::Invalid("poll_interval must be 0.5..10 s".into()));
        }
        if !(5.0..=600.0).contains(&self.refresh_interval) {
            return Err(Error::Invalid("refresh_interval must be 5..600 s".into()));
        }
        Ok(())
    }

    pub fn curve_points(&self) -> Option<Vec<Point>> {
        if self.mode == "custom" {
            Some(self.custom_curve.clone())
        } else {
            curve::preset(&self.mode)
        }
    }

    /// Load; a missing or invalid file yields safe defaults instead of failing.
    pub fn load(path: &Path) -> Config {
        let Ok(text) = fs::read_to_string(path) else { return Config::default() };
        match serde_json::from_str::<Config>(&text) {
            Ok(c) if c.validate().is_ok() => c,
            Ok(_) | Err(_) => {
                log::warn!("{}: invalid settings, using defaults", path.display());
                Config::default()
            }
        }
    }

    /// Atomic write (temp file + rename), mode 0644.
    pub fn save(&self, path: &Path) -> Result<()> {
        let dir = path.parent().unwrap_or(Path::new("."));
        fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(".config.{}.tmp", std::process::id()));
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(serde_json::to_string_pretty(self).map_err(|e| Error::Invalid(e.to_string()))?.as_bytes())?;
            f.sync_all()?;
        }
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o644))?;
        fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_fallback() {
        let dir = std::env::temp_dir().join(format!("aoc-cfg-{}", std::process::id()));
        let p = dir.join("config.json");
        let c = Config { mode: "silent".into(), ..Config::default() };
        c.save(&p).unwrap();
        assert_eq!(Config::load(&p).mode, "silent");
        fs::write(&p, r#"{"mode": "bogus"}"#).unwrap();
        assert_eq!(Config::load(&p).mode, "balanced");
        fs::remove_dir_all(dir).ok();
    }
}
