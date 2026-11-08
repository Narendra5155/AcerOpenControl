//! Service tests against the emulated firmware (ported from the Python prototype).

use serde_json::json;
use std::sync::{Arc, Mutex};

use aoc_core::config::Config;
use aoc_core::fake::{a715, FakeFirmware};
use aoc_core::profile::{Loaded, Machine};
use aoc_core::sensors::Sensors;

use crate::server;
use crate::service::Service;

#[derive(Clone)]
struct Temps(Arc<Mutex<(Option<f64>, Option<f64>)>>);

impl Sensors for Temps {
    fn cpu(&self) -> Option<f64> {
        self.0.lock().unwrap().0
    }
    fn gpu_asleep(&self) -> bool {
        self.0.lock().unwrap().1.is_none()
    }
    fn gpu(&self) -> Option<f64> {
        self.0.lock().unwrap().1
    }
}

struct Rig {
    svc: Service,
    fw: Arc<FakeFirmware>,
    temps: Temps,
    clock: Arc<Mutex<f64>>,
}

impl Rig {
    fn set_cpu(&self, v: Option<f64>) {
        self.temps.0.lock().unwrap().0 = v;
    }
    fn set_time(&self, t: f64) {
        *self.clock.lock().unwrap() = t;
    }
    /// Last WMBB call with this command id (state refreshes add reads after writes).
    fn last_of(&self, cmd: u32) -> Option<(u32, u32)> {
        self.fw.calls().into_iter().rev().find(|(c, _)| *c == cmd)
    }
    /// Fan-control calls only (0x68 duty / 0x69 auto).
    fn fan_calls(&self) -> Vec<(u32, u32)> {
        self.fw.calls().into_iter().filter(|(c, _)| *c == 0x68 || *c == 0x69).collect()
    }
}

fn rig_with(mode: &str, cpu: Option<f64>, gpu: Option<f64>) -> Rig {
    let fw = Arc::new(FakeFirmware::default());
    let temps = Temps(Arc::new(Mutex::new((cpu, gpu))));
    let clock = Arc::new(Mutex::new(0.0));
    let c2 = clock.clone();
    let loaded = Loaded { profile: a715(), path: "a715.toml".into() };
    let cfg = Config { mode: mode.into(), ..Config::default() };
    let mut svc = Service::new(
        Machine::default(),
        Some(&loaded),
        Some(fw.clone()),
        cfg,
        Box::new(temps.clone()),
        Box::new(|_| {}),
        Box::new(move || *c2.lock().unwrap()),
    );
    svc.features.as_mut().unwrap().camera_probe = || true;
    Rig { svc, fw, temps, clock }
}

fn rig(cpu: f64) -> Rig {
    rig_with("balanced", Some(cpu), None)
}

const DUTY_40: (u32, u32) = (0x68, 102 | 102 << 8); // 40% = raw 102 on both fans

#[test]
fn curve_applied_once() {
    let mut r = rig(65.0);
    r.svc.tick();
    r.svc.tick();
    assert_eq!(r.fan_calls(), vec![DUTY_40]);
}

#[test]
fn refresh_resends() {
    let mut r = rig(65.0);
    r.svc.tick();
    r.set_time(31.0);
    r.svc.tick();
    assert_eq!(r.fan_calls().len(), 2);
}

#[test]
fn auto_mode_hands_to_bios() {
    let mut r = rig_with("auto", Some(60.0), None);
    r.svc.tick();
    r.svc.tick();
    assert_eq!(r.fan_calls(), vec![(0x69, 0x0F)]);
    assert_eq!(r.svc.status()["bios_control"], json!(true));
}

#[test]
fn gpu_drives_when_hotter() {
    let mut r = rig_with("balanced", Some(45.0), Some(75.0));
    r.svc.tick();
    assert_eq!(r.fan_calls().last(), Some(&(0x68, 153 | 153 << 8))); // 60%
}

#[test]
fn emergency_forces_full_speed() {
    let mut r = rig_with("silent", Some(93.0), None);
    r.svc.tick();
    assert_eq!(r.fan_calls().last(), Some(&(0x68, 0xFFFF)));
    assert_eq!(r.svc.status()["emergency"], json!(true));
}

#[test]
fn sensor_failure_falls_back_to_bios() {
    let mut r = rig(65.0);
    r.svc.tick();
    r.set_cpu(None);
    for _ in 0..3 {
        r.svc.tick();
    }
    assert_eq!(r.fan_calls().last(), Some(&(0x69, 0x0F)));
    assert!(r.svc.status()["fault"].is_string());
}

#[test]
fn hardware_errors_fall_back_to_bios() {
    let mut r = rig(65.0);
    r.fw.set_fail(true);
    for _ in 0..3 {
        r.svc.tick();
    }
    r.fw.set_fail(false);
    // The fallback attempt itself failed while the firmware was failing;
    // make sure it was attempted and the fault is reported.
    assert!(r.svc.status()["fault"].as_str().unwrap().contains("hardware error"));
    assert_eq!(r.svc.status()["bios_control"], json!(true));
}

#[test]
fn set_mode_and_curve() {
    let mut r = rig(65.0);
    assert_eq!(r.svc.handle(&json!({"cmd": "set_mode", "mode": "max"}))["ok"], json!(true));
    assert_eq!(r.fan_calls().last(), Some(&(0x68, 0xFFFF)));
    assert_eq!(r.svc.handle(&json!({"cmd": "set_mode", "mode": "turbo"}))["ok"], json!(false));
    let bad = r.svc.handle(&json!({"cmd": "set_curve", "points": [[50, 90], [60, 10]]}));
    assert_eq!(bad["ok"], json!(false));
    let ok = r.svc.handle(&json!({"cmd": "set_curve", "points": [[40, 0], [90, 100]]}));
    assert_eq!(ok["ok"], json!(true));
    assert_eq!(r.svc.cfg.mode, "custom");
    assert_eq!(r.fan_calls().last(), Some(&(0x68, 128 | 128 << 8))); // 50%
}

#[test]
fn shutdown_restores_bios() {
    let mut r = rig(65.0);
    r.svc.tick();
    r.svc.shutdown();
    assert_eq!(r.fan_calls().last(), Some(&(0x69, 0x0F)));
}

#[test]
fn feature_api() {
    let mut r = rig(50.0);
    let v = r.svc.handle(&json!({"cmd": "set_kbd", "r": 255, "g": 0, "b": 0, "brightness": 100}));
    assert_eq!(v["ok"], json!(true));
    assert_eq!(v["kbd"], json!({"r": 255, "g": 0, "b": 0, "brightness": 100}));
    r.svc.handle(&json!({"cmd": "set_kbd", "brightness": 10}));
    assert_eq!(r.last_of(0x67), Some((0x67, 0xF400_0000 | 10)), "brightness-only change");
    let n67 = r.fw.calls().iter().filter(|c| c.0 == 0x67).count();
    assert_eq!(n67, 3, "colour+brightness, then brightness only");
    assert_eq!(r.svc.handle(&json!({"cmd": "set_perf_profile", "profile": "performance"}))["ok"], json!(true));
    assert_eq!(r.fw.state.lock().unwrap().cpcm, 2);
    let v = r.svc.handle(&json!({"cmd": "set_charge_limit", "enabled": true, "start": 55, "stop": 60}));
    assert_eq!(v["charge"], json!({"enabled": true, "start": 55, "stop": 60}));
    assert_eq!(r.svc.handle(&json!({"cmd": "set_charge_limit", "start": 90, "stop": 80}))["ok"], json!(false));
    assert_eq!(r.svc.handle(&json!({"cmd": "set_kbd", "r": 300}))["ok"], json!(false));
    assert_eq!(r.svc.handle(&json!({"cmd": "get_features"}))["available"]["keyboard"], json!(true));
}

#[test]
fn reapply_saved_features() {
    let mut r = rig(50.0);
    r.svc.handle(&json!({"cmd": "set_kbd", "r": 1, "g": 2, "b": 3, "brightness": 50}));
    r.svc.handle(&json!({"cmd": "set_perf_profile", "profile": 3}));
    r.svc.handle(&json!({"cmd": "set_webcam", "on": false}));
    r.fw.clear();
    r.svc.apply_saved_features();
    let calls = r.fw.calls();
    assert!(calls.iter().any(|c| c.0 == 0x67));
    assert!(calls.contains(&(0x79, 0x19 << 24 | 3)));
    assert!(calls.contains(&(0x22, 0)));
}

#[test]
fn nothing_forced_by_default() {
    let mut r = rig(50.0);
    r.svc.apply_saved_features();
    let writes: Vec<_> = r.fw.calls().into_iter().filter(|(c, _)| [0x67, 0x22].contains(c) || (*c == 0x79)).collect();
    assert!(writes.is_empty(), "{writes:?}");
}

#[test]
fn hotkey_keyboard_toggle_and_steps() {
    let mut r = rig(50.0);
    let kb = |r: &Rig| r.svc.cfg.features.saved.kbd.unwrap().brightness;
    r.svc.hotkey(0x83); // toggle: default 160 -> off
    assert_eq!(kb(&r), 0);
    r.svc.hotkey(0x83);
    assert_eq!(kb(&r), 160);
    r.svc.hotkey(0x82);
    assert_eq!(kb(&r), 192);
    r.svc.hotkey(0x81);
    r.svc.hotkey(0x81);
    assert_eq!(kb(&r), 128);
    r.svc.hotkey(0x83);
    r.svc.hotkey(0x83);
    assert_eq!(kb(&r), 128, "toggle restores the last level");
    for _ in 0..10 {
        r.svc.hotkey(0x82);
    }
    assert_eq!(kb(&r), 255);
    assert_eq!(r.last_of(0x67), Some((0x67, 0xF400_0000 | 255)));
}

#[test]
fn hotkey_colour_cycle_webcam_profile() {
    let mut r = rig(50.0);
    r.svc.hotkey(0x9F);
    let k = r.svc.cfg.features.saved.kbd.unwrap();
    assert_eq!((k.r, k.g, k.b), (255, 0, 0));
    r.svc.hotkey(0x9F);
    assert_eq!(r.svc.cfg.features.saved.kbd.unwrap().g, 96);

    r.svc.hotkey(0xF7); // camera present -> turn off
    assert_eq!(r.last_of(0x22), Some((0x22, 0)));

    // order in the profile: quiet(0), power-saver(1), balanced(3), performance(2); current 3
    r.svc.hotkey(0x59);
    assert_eq!(r.fw.state.lock().unwrap().cpcm, 2);
    r.svc.hotkey(0x59);
    assert_eq!(r.fw.state.lock().unwrap().cpcm, 0);

    r.svc.hotkey(0xEE);
    assert_eq!(r.svc.last_hotkey.as_ref().unwrap()["action"], json!(null));
}

#[test]
fn charger_change_runs_adjp_only_with_hotkeys() {
    let dir = std::env::temp_dir().join(format!("aoc-ac-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let online = dir.join("online");
    let mut r = rig(50.0);
    r.svc.ac_path = Some(online.clone());

    std::fs::write(&online, "1").unwrap();
    r.svc.watch_ac();
    std::fs::write(&online, "0").unwrap();
    r.svc.watch_ac();
    assert!(r.fw.state.lock().unwrap().other_calls.is_empty(), "no ADJP without hotkeys");

    r.svc.hotkeys_enabled = true;
    std::fs::write(&online, "1").unwrap();
    r.svc.watch_ac();
    assert_eq!(r.fw.state.lock().unwrap().other_calls, vec![r"\_SB.AC.ADJP".to_string()]);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn read_only_without_profile() {
    let mut svc = Service::new(
        Machine::default(),
        None,
        None,
        Config::default(),
        Box::new(Temps(Arc::new(Mutex::new((Some(70.0), None))))),
        Box::new(|_| {}),
        Box::new(|| 0.0),
    );
    svc.tick();
    let st = svc.status();
    assert_eq!(st["read_only"], json!(true));
    assert_eq!(st["cpu"], json!(70.0));
    let v = svc.handle(&json!({"cmd": "set_mode", "mode": "max"}));
    assert!(v["error"].as_str().unwrap().contains("read-only"));
    assert_eq!(svc.handle(&json!({"cmd": "get_model"}))["model"], json!(null));
}

#[test]
fn permission_gate() {
    let r = rig(50.0);
    let svc = Mutex::new(r.svc);
    let denied = server::respond(&svc, r#"{"cmd": "set_mode", "mode": "max"}"#, false);
    assert_eq!(denied["code"], json!("forbidden"));
    assert_eq!(server::respond(&svc, r#"{"cmd": "status"}"#, false)["ok"], json!(true));
    assert_eq!(server::respond(&svc, r#"{"cmd": "set_mode", "mode": "max"}"#, true)["ok"], json!(true));
    assert_eq!(server::respond(&svc, "not json", true)["error"], json!("invalid JSON"));
    assert_eq!(server::respond(&svc, "[1]", true)["ok"], json!(false));
}

#[test]
fn model_and_config_info() {
    let mut r = rig(50.0);
    let m = r.svc.handle(&json!({"cmd": "get_model"}));
    assert_eq!(m["model"]["id"], json!("acer-aspire-a715-79g"));
    assert_eq!(m["capabilities"]["fan_labels"], json!(["CPU", "GPU"]));
    assert_eq!(m["perf_profiles"].as_array().unwrap().len(), 4);
    let c = r.svc.handle(&json!({"cmd": "get_config"}));
    assert!(c["presets"]["balanced"].is_array());
    assert_eq!(r.svc.status()["api_version"], json!(1));
}

#[test]
fn socket_roundtrip() {
    use std::io::{BufRead, BufReader, Write};
    let r = rig(50.0);
    let svc = Arc::new(Mutex::new(r.svc));
    let path = std::env::temp_dir().join(format!("aoc-test-{}.sock", std::process::id()));
    let l = server::bind(&path).unwrap();
    let s2 = svc.clone();
    std::thread::spawn(move || server::serve(l, s2));
    let mut c = std::os::unix::net::UnixStream::connect(&path).unwrap();
    c.write_all(b"{\"cmd\": \"status\"}\n").unwrap();
    let mut line = String::new();
    BufReader::new(c.try_clone().unwrap()).read_line(&mut line).unwrap();
    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["mode"], json!("balanced"));
    std::fs::remove_file(path).ok();
}
