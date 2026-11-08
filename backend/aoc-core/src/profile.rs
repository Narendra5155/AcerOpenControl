//! Model profiles: one TOML file per laptop model describing how its Clevo
//! `WMBB` firmware interface behaves. See `docs/ADDING_A_MODEL.md`.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::Error;

pub const SYSTEM_MODELS_DIR: &str = "/usr/share/acer-open-control/models";
pub const USER_MODELS_DIR: &str = "/etc/acer-open-control/models.d";

/// Hotkey actions a profile may map firmware event codes to.
pub const HOTKEY_ACTIONS: &[&str] = &[
    "kbd-up",
    "kbd-down",
    "kbd-toggle",
    "kbd-colour-cycle",
    "touchpad-toggle",
    "touchpad-on",
    "touchpad-off",
    "webcam-toggle",
    "profile-cycle",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub model: ModelInfo,
    #[serde(rename = "match")]
    pub matching: Match,
    pub firmware: FirmwareCfg,
    pub fans: Option<FansCfg>,
    pub keyboard: Option<KeyboardCfg>,
    pub performance: Option<PerfCfg>,
    pub battery: Option<BatteryCfg>,
    pub webcam: Option<WebcamCfg>,
    pub hotkeys: Option<HotkeysCfg>,
    #[serde(default)]
    pub safety: SafetyCfg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Verified,
    Community,
    Experimental,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub platform: String,
    pub status: Status,
    #[serde(default)]
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Match {
    pub dmi_product_name: Vec<String>,
    #[serde(default)]
    pub dmi_board_name: Vec<String>,
    #[serde(default)]
    pub bios_version_prefix: Vec<String>,
    pub wmi_guid: String,
}

fn default_set_index() -> u32 {
    0x6B
}
fn default_read_ec() -> u32 {
    0x73
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirmwareCfg {
    pub wmbb_path: String,
    #[serde(default = "default_set_index")]
    pub set_index_cmd: u32,
    #[serde(default = "default_read_ec")]
    pub read_ec_cmd: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Endian {
    Big,
    Little,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FansCfg {
    pub count: usize,
    pub labels: Vec<String>,
    pub set_duty_cmd: u32,
    pub set_auto_cmd: u32,
    pub auto_mask: u32,
    pub duty_offsets: Vec<u8>,
    pub rpm_offsets: Vec<u8>,
    pub rpm_endian: Endian,
    pub rpm_dividend: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KeyboardKind {
    White,
    RgbSingleZone,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelShift {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyboardCfg {
    pub kind: KeyboardKind,
    pub cmd: u32,
    #[serde(default)]
    pub colour_prefix: Option<u32>,
    pub brightness_prefix: u32,
    #[serde(default)]
    pub channel_shift: Option<ChannelShift>,
    pub max_brightness: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerfProfile {
    pub index: u8,
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerfCfg {
    pub cmd: u32,
    pub selector: u8,
    #[serde(default)]
    pub current_path: Option<String>,
    #[serde(rename = "profile")]
    pub profiles: Vec<PerfProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatteryCfg {
    pub cmd: u32,
    pub thresholds_selector: u8,
    pub enable_selector: u8,
    pub read_cmd: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebcamCfg {
    pub cmd: u32,
    pub off_value: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HotkeysCfg {
    pub enable_cmd: u32,
    pub read_cmd: u32,
    pub notify: u32,
    pub event_guid_prefix: String,
    #[serde(default)]
    pub ac_adjust_method: Option<String>,
    #[serde(default)]
    pub map: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SafetyCfg {
    pub cpu_emergency: f64,
    pub gpu_emergency: f64,
    pub floor_pct: f64,
    pub floor_temp: f64,
}

impl Default for SafetyCfg {
    fn default() -> Self {
        SafetyCfg { cpu_emergency: 92.0, gpu_emergency: 85.0, floor_pct: 20.0, floor_temp: 60.0 }
    }
}

/// Parse a hotkey map key such as "0x9F" or "159".
pub fn parse_code(s: &str) -> Option<u8> {
    let s = s.trim();
    let v = if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u32::from_str_radix(h, 16).ok()?
    } else {
        s.parse::<u32>().ok()?
    };
    u8::try_from(v).ok()
}

impl HotkeysCfg {
    pub fn keymap(&self) -> BTreeMap<u8, String> {
        self.map.iter().filter_map(|(k, v)| parse_code(k).map(|c| (c, v.clone()))).collect()
    }
}

impl PerfCfg {
    pub fn by_index(&self, idx: u8) -> Option<&PerfProfile> {
        self.profiles.iter().find(|p| p.index == idx)
    }
    pub fn by_key(&self, key: &str) -> Option<&PerfProfile> {
        self.profiles.iter().find(|p| p.key == key)
    }
}

fn bad(msg: impl Into<String>) -> Error {
    Error::Profile(msg.into())
}

impl Profile {
    pub fn from_toml(text: &str) -> Result<Profile, Error> {
        let p: Profile = toml::from_str(text).map_err(|e| bad(e.to_string()))?;
        p.validate()?;
        Ok(p)
    }

    pub fn load(path: &Path) -> Result<Profile, Error> {
        let text = fs::read_to_string(path).map_err(|e| bad(format!("{}: {e}", path.display())))?;
        Profile::from_toml(&text).map_err(|e| bad(format!("{}: {e}", path.display())))
    }

    pub fn validate(&self) -> Result<(), Error> {
        let id = &self.model.id;
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return Err(bad("model.id must be non-empty lowercase letters, digits and '-'"));
        }
        if self.matching.dmi_product_name.is_empty() {
            return Err(bad("match.dmi_product_name must list at least one product name"));
        }
        if self.matching.wmi_guid.len() != 36 {
            return Err(bad("match.wmi_guid must be a 36-character GUID"));
        }
        if !self.firmware.wmbb_path.starts_with('\\') {
            return Err(bad("firmware.wmbb_path must be an absolute ACPI path like '\\_SB.WMI.WMBB'"));
        }
        if let Some(f) = &self.fans {
            if !(1..=4).contains(&f.count) {
                return Err(bad("fans.count must be 1..4"));
            }
            if f.labels.len() != f.count || f.duty_offsets.len() != f.count || f.rpm_offsets.len() != f.count {
                return Err(bad("fans.labels, duty_offsets and rpm_offsets must each have fans.count entries"));
            }
            if f.rpm_dividend == 0 {
                return Err(bad("fans.rpm_dividend must be > 0"));
            }
            if f.rpm_offsets.iter().any(|&o| o == 0xFF) {
                return Err(bad("fans.rpm_offsets must leave room for a 2-byte read"));
            }
        }
        if let Some(k) = &self.keyboard {
            if k.kind == KeyboardKind::RgbSingleZone {
                let s = k.channel_shift.ok_or_else(|| bad("keyboard.channel_shift is required for rgb keyboards"))?;
                if k.colour_prefix.is_none() {
                    return Err(bad("keyboard.colour_prefix is required for rgb keyboards"));
                }
                let set: HashSet<u8> = [s.red, s.green, s.blue].into_iter().collect();
                if set.len() != 3 || set.iter().any(|v| ![0, 8, 16].contains(v)) {
                    return Err(bad("keyboard.channel_shift must use 0, 8 and 16 once each"));
                }
            }
            if k.max_brightness == 0 {
                return Err(bad("keyboard.max_brightness must be > 0"));
            }
        }
        if let Some(p) = &self.performance {
            if p.profiles.is_empty() {
                return Err(bad("performance needs at least one [[performance.profile]]"));
            }
            let idx: HashSet<u8> = p.profiles.iter().map(|x| x.index).collect();
            let keys: HashSet<&str> = p.profiles.iter().map(|x| x.key.as_str()).collect();
            if idx.len() != p.profiles.len() || keys.len() != p.profiles.len() {
                return Err(bad("performance profile index and key values must be unique"));
            }
        }
        if let Some(h) = &self.hotkeys {
            for (code, action) in &h.map {
                if parse_code(code).is_none() {
                    return Err(bad(format!("hotkeys.map key {code:?} is not a byte value like \"0x9F\"")));
                }
                if !HOTKEY_ACTIONS.contains(&action.as_str()) {
                    return Err(bad(format!(
                        "hotkeys.map: unknown action {action:?} (allowed: {})",
                        HOTKEY_ACTIONS.join(", ")
                    )));
                }
            }
        }
        let s = &self.safety;
        if !(70.0..=105.0).contains(&s.cpu_emergency) || !(60.0..=100.0).contains(&s.gpu_emergency) {
            return Err(bad("safety emergency temperatures look wrong (cpu 70..105, gpu 60..100)"));
        }
        if !(0.0..=60.0).contains(&s.floor_pct) {
            return Err(bad("safety.floor_pct must be 0..60"));
        }
        Ok(())
    }

    /// Feature availability summary sent to clients.
    pub fn capabilities(&self) -> serde_json::Value {
        serde_json::json!({
            "fans": self.fans.as_ref().map(|f| f.count).unwrap_or(0),
            "fan_labels": self.fans.as_ref().map(|f| f.labels.clone()).unwrap_or_default(),
            "keyboard": self.keyboard.as_ref().map(|k| k.kind),
            "keyboard_max_brightness": self.keyboard.as_ref().map(|k| k.max_brightness),
            "perf_profile": self.performance.is_some(),
            "battery_limit": self.battery.is_some(),
            "webcam": self.webcam.is_some(),
            "hotkeys": self.hotkeys.is_some(),
        })
    }
}

/// Machine identity used for matching profiles.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Machine {
    pub product_name: String,
    pub board_name: String,
    pub bios_version: String,
    pub wmi_guids: Vec<String>,
}

fn read_trim(p: &str) -> String {
    fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_default()
}

impl Machine {
    pub fn detect() -> Machine {
        let mut wmi_guids: Vec<String> = fs::read_dir("/sys/bus/wmi/devices")
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().to_uppercase())
                    .collect()
            })
            .unwrap_or_default();
        wmi_guids.sort();
        Machine {
            product_name: read_trim("/sys/class/dmi/id/product_name"),
            board_name: read_trim("/sys/class/dmi/id/board_name"),
            bios_version: read_trim("/sys/class/dmi/id/bios_version"),
            wmi_guids,
        }
    }

    pub fn has_wmi_guid(&self, guid: &str) -> bool {
        let g = guid.to_uppercase();
        // Newer kernels may suffix duplicate GUIDs with "-N".
        self.wmi_guids.iter().any(|d| d.starts_with(&g))
    }
}

impl Profile {
    pub fn matches(&self, m: &Machine) -> bool {
        let mm = &self.matching;
        mm.dmi_product_name.iter().any(|n| n == &m.product_name)
            && (mm.dmi_board_name.is_empty() || mm.dmi_board_name.iter().any(|n| n == &m.board_name))
            && (mm.bios_version_prefix.is_empty()
                || mm.bios_version_prefix.iter().any(|p| m.bios_version.starts_with(p.as_str())))
            && m.has_wmi_guid(&mm.wmi_guid)
    }
}

/// A profile together with where it came from.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub profile: Profile,
    pub path: PathBuf,
}

/// Load every `*.toml` from the given directories. Later directories override
/// earlier ones by `model.id`. Invalid files are returned as errors, not fatal.
pub fn load_dirs(dirs: &[&Path]) -> (Vec<Loaded>, Vec<Error>) {
    let mut by_id: BTreeMap<String, Loaded> = BTreeMap::new();
    let mut errors = Vec::new();
    for dir in dirs {
        let Ok(rd) = fs::read_dir(dir) else { continue };
        let mut paths: Vec<PathBuf> = rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect();
        paths.sort();
        for path in paths {
            match Profile::load(&path) {
                Ok(profile) => {
                    by_id.insert(profile.model.id.clone(), Loaded { profile, path });
                }
                Err(e) => errors.push(e),
            }
        }
    }
    (by_id.into_values().collect(), errors)
}

pub fn load_default_dirs() -> (Vec<Loaded>, Vec<Error>) {
    load_dirs(&[Path::new(SYSTEM_MODELS_DIR), Path::new(USER_MODELS_DIR)])
}

/// Choose the profile for this machine. `force` (a model id from the config)
/// overrides matching and is the only way to use a non-matching profile.
pub fn select<'a>(loaded: &'a [Loaded], machine: &Machine, force: Option<&str>) -> Option<&'a Loaded> {
    if let Some(id) = force {
        return loaded.iter().find(|l| l.profile.model.id == id);
    }
    loaded.iter().find(|l| l.profile.matches(machine))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub const A715: &str = include_str!("../../../models/acer-aspire-a715-79g.toml");

    fn machine() -> Machine {
        Machine {
            product_name: "Aspire A715-79G".into(),
            board_name: "x".into(),
            bios_version: "1.07.01RAC5".into(),
            wmi_guids: vec!["ABBC0F6B-8EA1-11D1-00A0-C90629100000".into(),
                            "ABBC0F6D-8EA1-11D1-00A0-C90629100000".into()],
        }
    }

    #[test]
    fn shipped_profile_is_valid() {
        let p = Profile::from_toml(A715).unwrap();
        assert_eq!(p.fans.as_ref().unwrap().count, 2);
        assert_eq!(p.hotkeys.as_ref().unwrap().keymap()[&0x9F], "kbd-colour-cycle");
        assert_eq!(p.performance.as_ref().unwrap().by_key("performance").unwrap().index, 2);
    }

    #[test]
    fn matching() {
        let p = Profile::from_toml(A715).unwrap();
        assert!(p.matches(&machine()));
        let mut m = machine();
        m.product_name = "Aspire A715-76G".into();
        assert!(!p.matches(&m));
        let mut m = machine();
        m.wmi_guids.clear();
        assert!(!p.matches(&m), "must not match without the Clevo WMI GUID");
    }

    #[test]
    fn select_and_force() {
        let p = Profile::from_toml(A715).unwrap();
        let loaded = vec![Loaded { profile: p, path: "x.toml".into() }];
        let mut other = machine();
        other.product_name = "Something else".into();
        assert!(select(&loaded, &other, None).is_none());
        assert!(select(&loaded, &other, Some("acer-aspire-a715-79g")).is_some());
        assert!(select(&loaded, &machine(), None).is_some());
    }

    fn broken(from: &str, to: &str) -> Result<Profile, Error> {
        assert!(A715.contains(from), "{from}");
        Profile::from_toml(&A715.replacen(from, to, 1))
    }

    #[test]
    fn validation_errors() {
        assert!(broken("count = 2", "count = 3").is_err());
        assert!(broken("\"0x81\" = \"kbd-down\"", "\"0x81\" = \"self-destruct\"").is_err());
        assert!(broken("\"0x81\"", "\"0x181\"").is_err());
        assert!(broken("blue = 16", "blue = 8").is_err());
        assert!(broken("id = \"acer-aspire-a715-79g\"", "id = \"Bad Id\"").is_err());
        assert!(broken("cpu_emergency = 92.0", "cpu_emergency = 150.0").is_err());
        assert!(broken("[webcam]", "[webcam]\nbogus = 1").is_err(), "unknown keys are rejected");
    }

    #[test]
    fn optional_sections() {
        let start = A715.find("[keyboard]").unwrap();
        let end = A715.find("[performance]").unwrap();
        let text = format!("{}{}", &A715[..start], &A715[end..]);
        let p = Profile::from_toml(&text).unwrap();
        assert!(p.keyboard.is_none());
        assert_eq!(p.capabilities()["keyboard"], serde_json::Value::Null);
    }

    #[test]
    fn code_parsing() {
        assert_eq!(parse_code("0x9F"), Some(0x9F));
        assert_eq!(parse_code("159"), Some(159));
        assert_eq!(parse_code("0x100"), None);
        assert_eq!(parse_code("zz"), None);
    }
}
