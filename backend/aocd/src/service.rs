//! The daemon's state machine: fan control loop, fail-safes, firmware
//! features, hotkey actions and the request handler.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use aoc_core::config::{Config, MODES};
use aoc_core::curve::{self, Controller, Curve, Point};
use aoc_core::ec::{Clevo, Fan};
use aoc_core::features::{validate_charge, Features, Kbd};
use aoc_core::hotkeys::uinput::{self, VirtualKeyboard};
use aoc_core::kbd_led::KbdLed;
use aoc_core::profile::{Loaded, Machine, Profile};
use aoc_core::sensors::Sensors;
use aoc_core::{Error, API_VERSION, VERSION};

const SENSOR_FAIL_LIMIT: u32 = 3;
const HW_FAIL_LIMIT: u32 = 3;
pub const KBD_LEVELS: &[u8] = &[0, 32, 64, 96, 128, 160, 192, 224, 255];
pub const KBD_CYCLE: &[(u8, u8, u8)] = &[
    (255, 255, 255),
    (255, 0, 0),
    (255, 96, 0),
    (255, 204, 0),
    (0, 255, 0),
    (0, 255, 255),
    (0, 0, 255),
    (128, 0, 255),
    (255, 32, 128),
];

pub type SaveFn = Box<dyn Fn(&Config) + Send>;
pub type Clock = Box<dyn Fn() -> f64 + Send>;

pub struct Service {
    pub machine: Machine,
    pub profile: Option<Arc<Profile>>,
    pub profile_path: Option<PathBuf>,
    pub ec: Option<Clevo>,
    pub features: Option<Features>,
    pub cfg: Config,
    sensors: Box<dyn Sensors>,
    save: SaveFn,
    clock: Clock,

    controller: Option<Controller>,
    applied_duty: Option<u8>,
    last_sent: f64,
    force: bool,
    sensor_fails: u32,
    hw_fails: u32,
    fault: Option<String>,
    emergency: bool,
    ticks: u64,
    cpu: Option<f64>,
    gpu: Option<f64>,
    gpu_asleep: bool,
    fans: Vec<Fan>,
    target: Option<u8>,
    feature_state: Value,

    pub kbd_led: Option<KbdLed>,
    pub vkbd: Option<VirtualKeyboard>,
    pub hotkeys_enabled: bool,
    pub last_hotkey: Option<Value>,
    keymap: BTreeMap<u8, String>,
    ac_online: Option<String>,
    pub ac_path: Option<PathBuf>,
}

fn err(msg: impl std::fmt::Display) -> Value {
    json!({"ok": false, "error": msg.to_string()})
}

impl Service {
    pub fn new(
        machine: Machine,
        selected: Option<&Loaded>,
        fw: Option<Arc<dyn aoc_core::acpi::Firmware>>,
        cfg: Config,
        sensors: Box<dyn Sensors>,
        save: SaveFn,
        clock: Clock,
    ) -> Service {
        let profile = selected.map(|l| Arc::new(l.profile.clone()));
        let ec = match (&profile, fw) {
            (Some(p), Some(fw)) => Some(Clevo::new(fw, p.clone())),
            _ => None,
        };
        let features = ec.clone().map(Features::new);
        let mut keymap = profile.as_ref().and_then(|p| p.hotkeys.as_ref()).map(|h| h.keymap()).unwrap_or_default();
        for (k, v) in &cfg.features.keymap {
            if let Some(c) = aoc_core::profile::parse_code(k) {
                keymap.insert(c, v.clone());
            }
        }
        let mut s = Service {
            machine,
            profile_path: selected.map(|l| l.path.clone()),
            profile,
            ec,
            features,
            cfg,
            sensors,
            save,
            clock,
            controller: None,
            applied_duty: None,
            last_sent: 0.0,
            force: true,
            sensor_fails: 0,
            hw_fails: 0,
            fault: None,
            emergency: false,
            ticks: 0,
            cpu: None,
            gpu: None,
            gpu_asleep: true,
            fans: Vec::new(),
            target: None,
            feature_state: json!({}),
            kbd_led: None,
            vkbd: None,
            hotkeys_enabled: false,
            last_hotkey: None,
            keymap,
            ac_online: None,
            ac_path: None,
        };
        s.rebuild_controller();
        s
    }

    pub fn read_only(&self) -> bool {
        self.ec.is_none()
    }

    fn safety(&self) -> aoc_core::profile::SafetyCfg {
        self.profile.as_ref().map(|p| p.safety.clone()).unwrap_or_default()
    }

    fn rebuild_controller(&mut self) {
        let s = self.safety();
        self.controller = self
            .cfg
            .curve_points()
            .and_then(|pts| Curve::new(&pts).ok())
            .map(|c| Controller::new(c, self.cfg.hysteresis, self.cfg.ramp_down, s.floor_pct, s.floor_temp));
        self.force = true;
    }

    fn persist(&self) {
        (self.save)(&self.cfg);
    }

    // -- fan control -------------------------------------------------------------------
    fn hand_to_bios(&mut self, reason: Option<&str>) -> aoc_core::Result<()> {
        if let Some(ec) = &self.ec {
            if ec.profile.fans.is_some() {
                ec.set_auto()?;
            }
        }
        self.applied_duty = None;
        self.last_sent = (self.clock)();
        if let Some(r) = reason {
            log::warn!("fans handed to BIOS: {r}");
        }
        Ok(())
    }

    fn send(&mut self, duty: u8) -> aoc_core::Result<()> {
        if let Some(ec) = &self.ec {
            ec.set_duty(&[duty as f64])?;
        }
        self.applied_duty = Some(duty);
        self.last_sent = (self.clock)();
        Ok(())
    }

    pub fn tick(&mut self) {
        match self.tick_inner() {
            Ok(()) => self.hw_fails = 0,
            Err(e) => {
                self.hw_fails += 1;
                log::error!("tick failed ({}): {e}", self.hw_fails);
                if self.hw_fails >= HW_FAIL_LIMIT {
                    self.fault = Some(format!("hardware error: {e}"));
                    let _ = self.hand_to_bios(Some("repeated hardware errors"));
                }
            }
        }
    }

    fn tick_safely(&mut self) {
        if let Err(e) = self.tick_inner() {
            log::error!("apply failed: {e}");
        }
    }

    fn tick_inner(&mut self) -> aoc_core::Result<()> {
        self.watch_ac();
        self.cpu = self.sensors.cpu();
        self.gpu_asleep = self.sensors.gpu_asleep();
        self.gpu = if self.gpu_asleep { None } else { self.sensors.gpu() };
        if let Some(ec) = &self.ec {
            if ec.profile.fans.is_some() {
                match ec.read_fans() {
                    Ok(f) => self.fans = f,
                    Err(e) => log::debug!("fan status read failed: {e}"),
                }
            }
        }
        if self.features.is_some() && self.ticks % 5 == 0 {
            self.refresh_feature_state();
        }
        self.ticks += 1;

        let has_fans = self.ec.as_ref().is_some_and(|e| e.profile.fans.is_some());
        if !has_fans {
            return Ok(());
        }
        let now = (self.clock)();
        let refresh_due = now - self.last_sent >= self.cfg.refresh_interval;

        if self.cfg.mode == "auto" {
            self.emergency = false;
            self.target = None;
            if self.applied_duty.is_some() || self.force {
                self.hand_to_bios(None)?;
            }
            self.force = false;
            return Ok(());
        }

        let Some(cpu) = self.cpu else {
            self.sensor_fails += 1;
            if self.sensor_fails >= SENSOR_FAIL_LIMIT {
                self.fault = Some("CPU temperature unavailable".into());
                if self.applied_duty.is_some() || self.force {
                    self.hand_to_bios(Some("CPU temperature unavailable"))?;
                }
                self.force = false;
            }
            return Ok(());
        };
        self.sensor_fails = 0;
        self.fault = None;

        let hottest = self.gpu.map_or(cpu, |g| g.max(cpu));
        let s = self.safety();
        self.emergency = cpu >= s.cpu_emergency || self.gpu.is_some_and(|g| g >= s.gpu_emergency);
        let curve_duty = self.controller.as_mut().map(|c| c.step(hottest));
        let target = if self.cfg.mode == "max" || self.emergency { 100 } else { curve_duty.unwrap_or(100) };
        self.target = Some(target);

        if self.force || Some(target) != self.applied_duty || refresh_due {
            self.send(target)?;
            self.force = false;
        }
        Ok(())
    }

    // -- status & config ------------------------------------------------------------------
    pub fn status(&self) -> Value {
        json!({
            "ok": true,
            "api_version": API_VERSION,
            "version": VERSION,
            "model": self.profile.as_ref().map(|p| p.model.id.clone()),
            "read_only": self.read_only(),
            "mode": self.cfg.mode,
            "cpu": self.cpu,
            "gpu": self.gpu,
            "gpu_asleep": self.gpu_asleep,
            "target": self.target,
            "bios_control": self.applied_duty.is_none(),
            "fans": self.fans,
            "emergency": self.emergency,
            "fault": self.fault,
            "features": self.feature_state,
            "hotkey": self.last_hotkey,
        })
    }

    fn model_info(&self) -> Value {
        let m = &self.machine;
        let machine =
            json!({"product_name": m.product_name, "board_name": m.board_name, "bios_version": m.bios_version});
        match &self.profile {
            None => json!({"ok": true, "read_only": true, "model": null, "machine": machine,
                           "capabilities": {}, "perf_profiles": []}),
            Some(p) => json!({
                "ok": true,
                "read_only": self.read_only(),
                "model": p.model,
                "path": self.profile_path,
                "machine": machine,
                "capabilities": p.capabilities(),
                "perf_profiles": p.performance.as_ref().map(|x| x.profiles.clone()).unwrap_or_default(),
                "hotkeys_enabled": self.hotkeys_enabled,
            }),
        }
    }

    fn config_info(&self) -> Value {
        let presets: BTreeMap<&str, Vec<Point>> = curve::PRESETS.iter().map(|(k, p)| (*k, p.to_vec())).collect();
        json!({"ok": true, "config": self.cfg, "modes": MODES, "presets": presets})
    }

    // -- request handling ----------------------------------------------------------------
    pub fn handle(&mut self, req: &Value) -> Value {
        let cmd = req.get("cmd").and_then(Value::as_str).unwrap_or("");
        match cmd {
            "status" => return self.status(),
            "get_model" => return self.model_info(),
            "get_config" => return self.config_info(),
            _ => {}
        }
        if self.read_only() && cmd != "get_features" {
            if crate::auth::is_mutating(cmd) {
                return err("read-only: no model profile matches this machine (see `aocctl profile list`)");
            }
            return err(format!("unknown cmd {cmd:?}"));
        }
        match cmd {
            "set_mode" => {
                let mode = req.get("mode").and_then(Value::as_str).unwrap_or("");
                if !MODES.contains(&mode) {
                    return err(format!("unknown mode {mode:?}"));
                }
                self.cfg.mode = mode.to_string();
                self.rebuild_controller();
                self.persist();
                self.tick_safely();
                json!({"ok": true, "mode": mode})
            }
            "set_curve" => {
                let pts: Vec<Point> = match req.get("points").cloned().map(serde_json::from_value) {
                    Some(Ok(p)) => p,
                    _ => return err("points must be [[temp, duty], ...]"),
                };
                if let Err(e) = curve::validate_points(&pts) {
                    return err(e);
                }
                self.cfg.custom_curve = pts;
                if req.get("activate").and_then(Value::as_bool).unwrap_or(true) {
                    self.cfg.mode = "custom".into();
                }
                self.rebuild_controller();
                self.persist();
                self.tick_safely();
                json!({"ok": true})
            }
            "reapply" => {
                self.force = true;
                if self.hotkeys_enabled {
                    self.enable_hotkeys();
                }
                self.apply_saved_features();
                self.tick_safely();
                json!({"ok": true})
            }
            "get_features" | "set_kbd" | "set_perf_profile" | "set_charge_limit" | "set_webcam" => {
                match self.handle_feature(cmd, req) {
                    Ok(v) => v,
                    Err(e) => err(e),
                }
            }
            _ => err(format!("unknown cmd {cmd:?}")),
        }
    }

    // -- firmware features ----------------------------------------------------------------
    fn available(&self) -> Value {
        let p = self.profile.as_deref();
        json!({
            "keyboard": p.is_some_and(|p| p.keyboard.is_some()),
            "keyboard_rgb": p.and_then(|p| p.keyboard.as_ref())
                .is_some_and(|k| k.kind == aoc_core::profile::KeyboardKind::RgbSingleZone),
            "perf_profile": p.is_some_and(|p| p.performance.is_some()),
            "battery_limit": p.is_some_and(|p| p.battery.is_some()),
            "webcam": p.is_some_and(|p| p.webcam.is_some()),
        })
    }

    pub fn refresh_feature_state(&mut self) {
        let Some(f) = &self.features else { return };
        let p = &f.ec.profile;
        let mut st = json!({"available": self.available(), "kbd": self.cfg.features.saved.kbd});
        if p.performance.is_some() {
            st["perf_profile"] = json!(f.get_perf_profile());
        }
        if p.battery.is_some() {
            match f.get_charge() {
                Ok(c) => st["charge"] = json!(c),
                Err(e) => log::debug!("charge read failed: {e}"),
            }
        }
        if p.webcam.is_some() {
            st["webcam"] = json!(f.webcam_on());
        }
        self.feature_state = st;
    }

    /// Reapply what the user chose (boot, resume). Never forces defaults.
    pub fn apply_saved_features(&mut self) {
        let Some(f) = &self.features else { return };
        let saved = self.cfg.features.saved.clone();
        let p = f.ec.profile.clone();
        let run = |name: &str, r: aoc_core::Result<()>| {
            if let Err(e) = r {
                log::error!("reapplying {name} failed: {e}");
            }
        };
        if let (Some(k), Some(_)) = (saved.kbd, &p.keyboard) {
            run("keyboard", f.set_kbd(k));
        }
        if let (Some(i), Some(_)) = (saved.perf_profile, &p.performance) {
            run("profile", f.set_perf_profile(i));
        }
        if let (Some(c), Some(_)) = (saved.charge, &p.battery) {
            run("charge limit", f.set_charge(c).map(|_| ()));
        }
        if let (Some(false), Some(_)) = (saved.webcam, &p.webcam) {
            run("webcam", f.set_webcam(false));
        }
        self.refresh_feature_state();
    }

    fn handle_feature(&mut self, cmd: &str, req: &Value) -> Result<Value, Error> {
        let Some(f) = &self.features else { return Err(Error::Unsupported("firmware features")) };
        let p = f.ec.profile.clone();
        let int = |k: &str| req.get(k).and_then(Value::as_i64);
        match cmd {
            "get_features" => {}
            "set_kbd" => {
                let kcfg = p.keyboard.as_ref().ok_or(Error::Unsupported("keyboard backlight"))?;
                let mut cur = self.cfg.features.saved.kbd.unwrap_or_default();
                let byte = |k: &str, v: &mut u8| -> Result<(), Error> {
                    if let Some(x) = int(k) {
                        *v = u8::try_from(x).map_err(|_| Error::Invalid(format!("{k} must be 0..255")))?;
                    }
                    Ok(())
                };
                byte("r", &mut cur.r)?;
                byte("g", &mut cur.g)?;
                byte("b", &mut cur.b)?;
                byte("brightness", &mut cur.brightness)?;
                if cur.brightness > kcfg.max_brightness {
                    return Err(Error::Invalid(format!("brightness must be 0..{}", kcfg.max_brightness)));
                }
                let colour_change = ["r", "g", "b"].iter().any(|k| req.get(*k).is_some());
                if colour_change || self.cfg.features.saved.kbd.is_none() {
                    f.set_kbd(cur)?;
                } else {
                    f.set_kbd_brightness(cur.brightness)?;
                }
                if let Some(prev) = self.cfg.features.saved.kbd {
                    if prev.brightness > 0 {
                        self.cfg.features.saved.kbd_last_on = Some(prev.brightness);
                    }
                }
                self.cfg.features.saved.kbd = Some(cur);
                let from_led = req.get("from_led").and_then(Value::as_bool).unwrap_or(false);
                if let (Some(led), false) = (&self.kbd_led, from_led) {
                    led.sync(cur.brightness);
                }
            }
            "set_perf_profile" => {
                let perf = p.performance.as_ref().ok_or(Error::Unsupported("performance profiles"))?;
                match req.get("profile") {
                    None | Some(Value::Null) => self.cfg.features.saved.perf_profile = None,
                    Some(Value::String(s)) if s == "bios" => self.cfg.features.saved.perf_profile = None,
                    Some(v) => {
                        let idx = match v {
                            Value::String(s) => perf
                                .by_key(s)
                                .map(|x| x.index)
                                .ok_or_else(|| Error::Invalid(format!("unknown profile {s:?}")))?,
                            Value::Number(n) => n
                                .as_u64()
                                .and_then(|x| u8::try_from(x).ok())
                                .ok_or_else(|| Error::Invalid("bad profile index".into()))?,
                            _ => return Err(Error::Invalid("profile must be a key or index".into())),
                        };
                        f.set_perf_profile(idx)?;
                        self.cfg.features.saved.perf_profile = Some(idx);
                    }
                }
            }
            "set_charge_limit" => {
                let enabled = req.get("enabled").and_then(Value::as_bool).unwrap_or(true);
                let lim = validate_charge(enabled, int("start").unwrap_or(75), int("stop").unwrap_or(80))?;
                let got = f.set_charge(lim)?;
                if got != lim {
                    log::warn!("charge limit readback {got:?} != requested {lim:?}");
                }
                self.cfg.features.saved.charge = Some(lim);
            }
            "set_webcam" => {
                let on = req.get("on").and_then(Value::as_bool).unwrap_or(true);
                f.set_webcam(on)?;
                self.cfg.features.saved.webcam = Some(on);
            }
            _ => return Err(Error::Invalid(format!("unknown cmd {cmd:?}"))),
        }
        if cmd != "get_features" {
            self.persist();
        }
        self.refresh_feature_state();
        let mut out = self.feature_state.clone();
        out["ok"] = json!(true);
        Ok(out)
    }

    // -- hotkeys ----------------------------------------------------------------------------
    pub fn enable_hotkeys(&mut self) {
        let (Some(ec), Some(h)) = (&self.ec, self.profile.as_ref().and_then(|p| p.hotkeys.clone())) else { return };
        match aoc_core::hotkeys::enable(ec, &h) {
            Ok(()) => self.hotkeys_enabled = true,
            Err(e) => log::error!("enabling hotkeys failed: {e}"),
        }
    }

    /// Handle one hotkey code (already read from the firmware).
    pub fn hotkey(&mut self, code: u8) {
        let action = self.keymap.get(&code).cloned();
        log::info!("hotkey {code:#04x} -> {}", action.as_deref().unwrap_or("unmapped"));
        self.last_hotkey = Some(json!({"code": code, "action": action}));
        let Some(action) = action else { return };
        match action.as_str() {
            "kbd-up" => self.kbd_step(1),
            "kbd-down" => self.kbd_step(-1),
            "kbd-toggle" => self.kbd_toggle(),
            "kbd-colour-cycle" => self.kbd_colour_cycle(),
            "webcam-toggle" => self.webcam_toggle(),
            "profile-cycle" => self.profile_cycle(),
            "touchpad-toggle" => self.tap(uinput::KEY_TOUCHPAD_TOGGLE),
            "touchpad-on" => self.tap(uinput::KEY_TOUCHPAD_ON),
            "touchpad-off" => self.tap(uinput::KEY_TOUCHPAD_OFF),
            other => log::warn!("unknown hotkey action {other}"),
        }
    }

    fn tap(&mut self, key: u16) {
        if let Some(k) = &mut self.vkbd {
            if let Err(e) = k.tap(key) {
                log::error!("virtual key {key} failed: {e}");
            }
        }
    }

    fn kbd_current(&self) -> Kbd {
        self.cfg.features.saved.kbd.unwrap_or_default()
    }

    fn kbd_set_brightness(&mut self, v: u8) {
        if self.profile.as_ref().and_then(|p| p.keyboard.as_ref()).is_none() {
            return;
        }
        let r = self.handle(&json!({"cmd": "set_kbd", "brightness": v}));
        if r["ok"] != json!(true) {
            log::error!("keyboard brightness: {}", r["error"]);
        }
    }

    pub fn kbd_step(&mut self, dir: i32) {
        let max = self.profile.as_ref().and_then(|p| p.keyboard.as_ref()).map_or(255, |k| k.max_brightness);
        let levels: Vec<u8> = KBD_LEVELS.iter().map(|&l| ((l as u32 * max as u32) / 255) as u8).collect();
        let cur = self.kbd_current().brightness;
        let next = if dir > 0 {
            levels.iter().copied().find(|&l| l > cur).unwrap_or(max)
        } else {
            levels.iter().rev().copied().find(|&l| l < cur).unwrap_or(0)
        };
        self.kbd_set_brightness(next);
    }

    pub fn kbd_toggle(&mut self) {
        let cur = self.kbd_current().brightness;
        if cur > 0 {
            self.kbd_set_brightness(0);
        } else {
            self.kbd_set_brightness(self.cfg.features.saved.kbd_last_on.unwrap_or(160));
        }
    }

    pub fn kbd_colour_cycle(&mut self) {
        let cur = self.kbd_current();
        let idx = KBD_CYCLE.iter().position(|&c| c == (cur.r, cur.g, cur.b));
        let (r, g, b) = KBD_CYCLE[idx.map_or(0, |i| (i + 1) % KBD_CYCLE.len())];
        let brightness = if cur.brightness > 0 { cur.brightness } else { 160 };
        let _ = self.handle(&json!({"cmd": "set_kbd", "r": r, "g": g, "b": b, "brightness": brightness}));
    }

    pub fn webcam_toggle(&mut self) {
        let Some(f) = &self.features else { return };
        if f.ec.profile.webcam.is_none() {
            return;
        }
        let on = f.webcam_on();
        let _ = self.handle(&json!({"cmd": "set_webcam", "on": !on}));
        log::info!("webcam {} by hotkey", if on { "off" } else { "on" });
    }

    pub fn profile_cycle(&mut self) {
        let Some(f) = &self.features else { return };
        let Some(perf) = f.ec.profile.performance.clone() else { return };
        let order: Vec<u8> = perf.profiles.iter().map(|p| p.index).collect();
        let next = match f.get_perf_profile().and_then(|c| order.iter().position(|&x| x == c)) {
            Some(i) => order[(i + 1) % order.len()],
            None => order[0],
        };
        let _ = self.handle(&json!({"cmd": "set_perf_profile", "profile": next}));
    }

    /// With hotkeys registered, the firmware's `_PSR` no longer runs its
    /// charger-change power adjustment, so the daemon calls it instead.
    pub fn watch_ac(&mut self) {
        let Some(path) = &self.ac_path else { return };
        let Ok(online) = std::fs::read_to_string(path).map(|s| s.trim().to_string()) else { return };
        let changed = self.ac_online.as_ref().is_some_and(|prev| *prev != online);
        if changed && self.hotkeys_enabled {
            let method =
                self.profile.as_ref().and_then(|p| p.hotkeys.as_ref()).and_then(|h| h.ac_adjust_method.clone());
            if let (Some(ec), Some(m)) = (&self.ec, method) {
                match ec.call_path(&m, &[aoc_core::acpi::Arg::Int(0)]) {
                    Ok(_) => log::info!("charger {}: ran {m}", if online == "1" { "connected" } else { "removed" }),
                    Err(e) => log::error!("{m} failed: {e}"),
                }
            }
        }
        self.ac_online = Some(online);
    }

    pub fn shutdown(&mut self) {
        if let Err(e) = self.hand_to_bios(Some("daemon stopping")) {
            log::error!("could not restore BIOS fan control: {e}");
        }
    }
}

/// Find the mains power-supply `online` file (named AC, ACAD, ADP1, ... by vendor).
pub fn find_mains_online() -> Option<PathBuf> {
    let rd = std::fs::read_dir("/sys/class/power_supply").ok()?;
    rd.filter_map(|e| e.ok()).map(|e| e.path()).find_map(|p| {
        let t = std::fs::read_to_string(p.join("type")).ok()?;
        (t.trim() == "Mains").then(|| p.join("online"))
    })
}
