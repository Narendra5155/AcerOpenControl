//! Hardware probes (root) and draft-profile generation for new models.
//! Results are saved as JSON in the output directory; `aocctl profile generate`
//! turns them into a model profile.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::sleep;
use std::time::{Duration, Instant};

use aoc_core::acpi::{self, AcpiCall};
use aoc_core::ec::{decode_rpm, Clevo};
use aoc_core::features::{camera_present, ChargeLimit, Features, Kbd};
use aoc_core::hotkeys::{self, netlink::Listener};
use aoc_core::profile::{self, Endian, KeyboardKind, Machine, Profile};
use aoc_core::sensors::cpu_temp;

/// Template used to probe machines that have no profile yet: the generic
/// Clevo command set, as verified on the A715-79G.
pub const TEMPLATE: &str = include_str!("../../../models/acer-aspire-a715-79g.toml");

pub const FANS_FILE: &str = "aoc-probe-fans.json";
pub const FEATURES_FILE: &str = "aoc-probe-features.json";
pub const KEYS_FILE: &str = "aoc-probe-keys.json";

pub fn require_root() -> Result<()> {
    // SAFETY: no preconditions.
    if unsafe { libc::geteuid() } != 0 {
        bail!("this command needs root: run it with sudo");
    }
    Ok(())
}

pub fn daemon_running() -> bool {
    Path::new(crate::client::SOCKET_PATH).exists()
}

/// The profile to probe with: an explicit file, the matching profile, or the
/// generic Clevo template.
pub fn base_profile(explicit: Option<&Path>) -> Result<Profile> {
    if let Some(p) = explicit {
        return Ok(Profile::load(p)?);
    }
    let machine = Machine::detect();
    let (loaded, _) = profile::load_default_dirs();
    if let Some(l) = profile::select(&loaded, &machine, None) {
        eprintln!("Using profile {} ({})", l.profile.model.id, l.path.display());
        return Ok(l.profile.clone());
    }
    let p = Profile::from_toml(TEMPLATE)?;
    if !machine.has_wmi_guid(&p.matching.wmi_guid) {
        bail!(
            "this machine does not expose the Clevo WMI interface ({}).\n\
             AcerOpenControl only supports laptops with that interface.",
            p.matching.wmi_guid
        );
    }
    eprintln!("No profile matches {:?}: probing with generic Clevo defaults.", machine.product_name);
    Ok(p)
}

fn connect(p: Profile) -> Result<Clevo> {
    acpi::ensure_module()?;
    Ok(Clevo::new(Arc::new(AcpiCall), Arc::new(p)))
}

fn ask(q: &str) -> bool {
    loop {
        print!("   ? {q} [y/n] ");
        io::stdout().flush().ok();
        let mut a = String::new();
        if io::stdin().lock().read_line(&mut a).is_err() || a.is_empty() {
            return false;
        }
        match a.trim().to_lowercase().as_str() {
            "y" | "yes" => return true,
            "n" | "no" => return false,
            _ => {}
        }
    }
}

fn save(dir: &Path, name: &str, v: &Value) -> Result<PathBuf> {
    let path = dir.join(name);
    std::fs::write(&path, serde_json::to_string_pretty(v)?).with_context(|| format!("writing {}", path.display()))?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).ok();
    println!("\nSaved {}", path.display());
    Ok(path)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// =================================================================== fans
fn fan_sample(ec: &Clevo, label: &str) -> Result<Value> {
    let f = ec.fans_cfg()?;
    let mut fans = Vec::new();
    for i in 0..f.count {
        let duty = ec.read_ec(f.duty_offsets[i], 1)?[0];
        let r = ec.read_ec(f.rpm_offsets[i], 2)?;
        let two = [r[0], r[1]];
        fans.push(json!({
            "duty_raw": duty, "rpm_hex": hex(&r),
            "rpm_be": decode_rpm(two, Endian::Big, f.rpm_dividend),
            "rpm_le": decode_rpm(two, Endian::Little, f.rpm_dividend),
        }));
    }
    let line: Vec<String> = fans
        .iter()
        .map(|f| format!("duty={:3} rpm(be/le)={:5}/{:5}", f["duty_raw"], f["rpm_be"], f["rpm_le"]))
        .collect();
    println!("  [{label:>8}] cpu={:?}C  {}", cpu_temp().map(|t| t.round()), line.join("  |  "));
    Ok(json!({"label": label, "cpu": cpu_temp(), "fans": fans}))
}

fn phase(ec: &Clevo, label: &str, secs: u64, out: &mut Vec<Value>) -> Result<()> {
    for _ in 0..secs {
        sleep(Duration::from_secs(1));
        out.push(fan_sample(ec, label)?);
    }
    Ok(())
}

/// Pick the RPM byte order that gives plausible speeds at 100% duty and
/// a higher speed at 100% than at 30%.
pub fn decide_endian(samples: &[Value]) -> Option<Endian> {
    let avg = |label: &str, key: &str| -> f64 {
        let v: Vec<f64> = samples
            .iter()
            .filter(|s| s["label"] == label)
            .skip(3) // let the fan settle
            .filter_map(|s| s["fans"][0][key].as_f64())
            .collect();
        if v.is_empty() {
            0.0
        } else {
            v.iter().sum::<f64>() / v.len() as f64
        }
    };
    let ok = |key: &str| {
        let hi = avg("set100", key);
        let lo = avg("set30", key);
        (1500.0..=9000.0).contains(&hi) && hi > lo * 1.3
    };
    match (ok("rpm_be"), ok("rpm_le")) {
        (true, false) => Some(Endian::Big),
        (false, true) => Some(Endian::Little),
        _ => None,
    }
}

pub fn fans(base: Option<&Path>, out_dir: &Path) -> Result<()> {
    require_root()?;
    if daemon_running() {
        bail!("aocd is running and would fight the probe: stop it first with 'sudo systemctl stop aocd'");
    }
    let p = base_profile(base)?;
    let ec = connect(p)?;
    let mut samples = Vec::new();

    println!("== EC RAM 0xC0..0xFF");
    let mut dump = Vec::new();
    for off in 0xC0u8..=0xFF {
        dump.push(ec.read_ec(off, 1)?[0]);
    }
    for (row, chunk) in dump.chunks(16).enumerate() {
        println!("  {:02X}: {}", 0xC0 + row * 16, chunk.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "));
    }

    let result = (|| -> Result<()> {
        println!("== Baseline (BIOS automatic), 4 s");
        phase(&ec, "auto", 4, &mut samples)?;
        println!("== Manual 100% for 10 s  (fans should get LOUD)");
        ec.set_duty(&[100.0])?;
        phase(&ec, "set100", 10, &mut samples)?;
        println!("== Manual 30% for 20 s  (does the EC override it?)");
        ec.set_duty(&[30.0])?;
        phase(&ec, "set30", 20, &mut samples)?;
        Ok(())
    })();
    println!("== Restoring BIOS automatic fan control");
    if let Err(e) = ec.set_auto() {
        eprintln!("!! FAILED to restore automatic control: {e}\n   Run: sudo aocctl restore-auto");
    }
    result?;
    phase(&ec, "restored", 5, &mut samples)?;

    let endian = decide_endian(&samples);
    match endian {
        Some(e) => println!("\nRPM byte order looks like: {e:?}"),
        None => println!("\n!! Could not decide the RPM byte order automatically; check the samples by hand."),
    }
    let audible = ask("Did the fans get clearly LOUDER during the 100% phase?");
    save(
        out_dir,
        FANS_FILE,
        &json!({"machine": Machine::detect(), "ec_c0_ff": hex(&dump), "samples": samples,
                "rpm_endian": endian, "audible": audible}),
    )?;
    Ok(())
}

// =================================================================== features
fn rapl() -> Value {
    let read = |c: &str| {
        std::fs::read_to_string(format!("/sys/class/powercap/intel-rapl:0/{c}_power_limit_uw"))
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .map(|v| v / 1_000_000)
    };
    let gpu = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=enforced.power.limit", "--format=csv,noheader"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string());
    json!({"pl1_w": read("constraint_0"), "pl2_w": read("constraint_1"), "gpu_limit": gpu})
}

/// Toggle a switch both ways; report which value turns the device off.
/// Always ends with the observed state equal to the starting state.
fn toggle_test(ec: &Clevo, cmd: u32, observe: fn() -> bool, settle: u64) -> Value {
    let before = observe();
    for (off, on) in [(0u32, 1u32), (1, 0)] {
        let _ = ec.set(cmd, off);
        sleep(Duration::from_secs(settle));
        let changed = observe();
        let _ = ec.set(cmd, on);
        sleep(Duration::from_secs(settle));
        let back = observe();
        if changed != before {
            return json!({"works": true, "off_value": off, "restored": back == before});
        }
    }
    json!({"works": false, "restored": observe() == before})
}

pub fn features(base: Option<&Path>, out_dir: &Path) -> Result<()> {
    require_root()?;
    let p = base_profile(base)?;
    let ec = connect(p.clone())?;
    let f = Features::new(ec.clone());
    let mut res = json!({"machine": Machine::detect()});

    println!("== Keyboard backlight (look at the keyboard)");
    let k = p.keyboard.clone().context("template has no keyboard section")?;
    let white = Kbd { r: 255, g: 255, b: 255, brightness: k.max_brightness };
    let rgb = f.set_kbd(white).is_ok() && ask("Is the keyboard now WHITE at FULL brightness?");
    res["keyboard_rgb"] = json!(rgb);
    if rgb {
        let mut shift = serde_json::Map::new();
        for s in [16u32, 8, 0] {
            let _ = ec.set(k.cmd, k.colour_prefix.unwrap_or(0xF000_0000) | 0xFF << s);
            for colour in ["red", "green", "blue"] {
                if ask(&format!("Is it {} now?", colour.to_uppercase())) {
                    shift.insert(colour.into(), json!(s));
                    break;
                }
            }
        }
        res["channel_shift"] = Value::Object(shift);
        let _ = f.set_kbd_brightness(0);
        res["kbd_off_works"] = json!(ask("Brightness set to 0: is the backlight OFF?"));
        let _ = f.set_kbd(Kbd { brightness: 160.min(k.max_brightness), ..white });
        println!("   (keyboard left white at medium brightness)");
    }

    println!("\n== Performance profiles");
    let perf_cfg = p.performance.clone().context("template has no performance section")?;
    let before = f.get_perf_profile();
    let mut perf = serde_json::Map::new();
    perf.insert("before".into(), rapl());
    for idx in 0..4u8 {
        if ec.set(perf_cfg.cmd, (perf_cfg.selector as u32) << 24 | idx as u32).is_err() {
            break;
        }
        sleep(Duration::from_secs(3));
        let r = rapl();
        println!("   profile {idx}: {r}");
        perf.insert(idx.to_string(), r);
    }
    let restore = before.unwrap_or(perf_cfg.profiles[0].index);
    let _ = ec.set(perf_cfg.cmd, (perf_cfg.selector as u32) << 24 | restore as u32);
    println!("   restored profile {restore}");
    res["perf"] = json!({"current_before": before, "limits": perf});

    println!("\n== Webcam switch");
    let w = p.webcam.clone().context("template has no webcam section")?;
    let r = toggle_test(&ec, w.cmd, camera_present, 3);
    println!("   {r}");
    res["webcam"] = r;

    println!("\n== Battery charge limit (read only)");
    match f.get_charge() {
        Ok(ChargeLimit { enabled, start, stop }) => {
            println!("   enabled={enabled} start={start}% stop={stop}%");
            res["battery"] = json!({"readable": true, "enabled": enabled, "start": start, "stop": stop});
        }
        Err(e) => res["battery"] = json!({"readable": false, "error": e.to_string()}),
    }
    save(out_dir, FEATURES_FILE, &res)?;
    Ok(())
}

// =================================================================== keys
const KEY_PROMPTS: &[(&str, &str)] = &[
    ("touchpad-toggle", "the TOUCHPAD on/off key"),
    ("webcam-toggle", "the CAMERA on/off key"),
    ("kbd-down", "KEYBOARD BACKLIGHT DOWN"),
    ("kbd-up", "KEYBOARD BACKLIGHT UP"),
    ("kbd-toggle", "KEYBOARD BACKLIGHT on/off"),
    ("kbd-colour-cycle", "the keyboard-light MODE/COLOUR key"),
    ("profile-cycle", "the PERFORMANCE MODE key"),
];

fn wait_code(l: &Listener, ec: &Clevo, cfg: &profile::HotkeysCfg, timeout: Duration) -> Result<Option<u8>> {
    let end = Instant::now() + timeout;
    while Instant::now() < end {
        for ev in l.next_events()? {
            if hotkeys::is_hotkey_event(cfg, &ev) {
                return Ok(Some(hotkeys::read_code(ec, cfg)?));
            }
        }
    }
    Ok(None)
}

pub fn keys(base: Option<&Path>, out_dir: &Path, raw_secs: Option<u64>) -> Result<()> {
    require_root()?;
    let p = base_profile(base)?;
    let cfg = p.hotkeys.clone().context("template has no hotkeys section")?;
    let ec = connect(p)?;
    let l = Listener::subscribe().context("subscribing to ACPI netlink events")?;
    l.set_timeout(Some(Duration::from_millis(500)))?;
    hotkeys::enable(&ec, &cfg)?;

    if let Some(secs) = raw_secs {
        println!("Hotkeys enabled. Press Fn keys now ({secs} s, Ctrl+C to stop).\n");
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end {
            for ev in l.next_events()? {
                if hotkeys::is_hotkey_event(&cfg, &ev) {
                    let code = hotkeys::read_code(&ec, &cfg)?;
                    let known = cfg.keymap().get(&code).cloned().unwrap_or_else(|| "UNKNOWN".into());
                    println!("  HOTKEY code=0x{code:02x}  {known}");
                } else {
                    println!("  (acpi event: class={:?} bus={:?} type={:#x})", ev.device_class, ev.bus_id, ev.kind);
                }
            }
        }
        return Ok(());
    }

    println!("Hotkeys enabled. For each prompt, press the key ONCE (with Fn if needed).");
    println!("Wait 10 seconds or press nothing to skip a key your laptop doesn't have.\n");
    let mut map = serde_json::Map::new();
    for (action, desc) in KEY_PROMPTS {
        println!("-> Press {desc} ...");
        match wait_code(&l, &ec, &cfg, Duration::from_secs(10))? {
            Some(code) => {
                println!("   got code 0x{code:02x}");
                map.insert(format!("0x{code:02X}"), json!(action));
                sleep(Duration::from_millis(700));
                // Swallow a possible second event (some keys send press + release codes).
                let _ = wait_code(&l, &ec, &cfg, Duration::from_millis(300));
            }
            None => println!("   skipped"),
        }
    }
    save(out_dir, KEYS_FILE, &json!({"machine": Machine::detect(), "map": map}))?;
    println!("Hotkeys stay enabled until the next sleep or reboot.");
    Ok(())
}

// =================================================================== generate
fn load_json(dir: &Path, name: &str) -> Option<Value> {
    let text = std::fs::read_to_string(dir.join(name)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

/// Build a draft profile from the template, this machine's identity and any
/// probe results found in `dir`.
pub fn generate(dir: &Path, machine: &Machine) -> Result<(Profile, Vec<String>)> {
    let mut p = Profile::from_toml(TEMPLATE)?;
    let mut notes = Vec::new();
    let name = if machine.product_name.is_empty() { "unknown".to_string() } else { machine.product_name.clone() };
    p.model.id = format!("acer-{}", slug(&name));
    p.model.name = format!("Acer {name}");
    p.model.platform = "clevo".into();
    p.model.status = profile::Status::Experimental;
    p.model.notes = format!("Generated by aocctl {} on BIOS {}", aoc_core::VERSION, machine.bios_version);
    p.matching.dmi_product_name = vec![machine.product_name.clone()];
    if let Some(perf) = &mut p.performance {
        for pr in &mut perf.profiles {
            pr.description.clear();
        }
    }

    match load_json(dir, FANS_FILE) {
        Some(f) => {
            if let (Some(fans), Some(e)) = (&mut p.fans, f["rpm_endian"].as_str()) {
                fans.rpm_endian = if e == "Little" { Endian::Little } else { Endian::Big };
            }
            if f["audible"] != json!(true) {
                notes.push("fan probe: fans did not audibly change; fan control may not work".into());
            }
        }
        None => notes.push(format!("no {FANS_FILE}: fan settings are template defaults (run 'sudo aocctl probe fans')")),
    }

    match load_json(dir, FEATURES_FILE) {
        Some(f) => {
            if f["keyboard_rgb"] == json!(true) {
                if let (Some(k), Some(s)) = (&mut p.keyboard, f["channel_shift"].as_object()) {
                    let get = |c: &str| s.get(c).and_then(Value::as_u64).map(|v| v as u8);
                    if let (Some(r), Some(g), Some(b)) = (get("red"), get("green"), get("blue")) {
                        k.channel_shift = Some(profile::ChannelShift { red: r, green: g, blue: b });
                    } else {
                        notes.push("keyboard: colour order incomplete; check channel_shift".into());
                    }
                    k.kind = KeyboardKind::RgbSingleZone;
                }
            } else {
                p.keyboard = None;
                notes.push("keyboard: RGB test failed, so the keyboard section was removed".into());
            }
            if let Some(perf) = &mut p.performance {
                let lim = &f["perf"]["limits"];
                for pr in &mut perf.profiles {
                    let l = &lim[pr.index.to_string()];
                    if let (Some(a), Some(b)) = (l["pl1_w"].as_u64(), l["pl2_w"].as_u64()) {
                        pr.description = format!("CPU {a} W / {b} W");
                    }
                }
                notes.push("performance: check that the labels match the measured limits".into());
            }
            if f["webcam"]["works"] == json!(true) {
                if let Some(w) = &mut p.webcam {
                    w.off_value = f["webcam"]["off_value"].as_u64().unwrap_or(0) as u8;
                }
            } else {
                p.webcam = None;
            }
            if f["battery"]["readable"] != json!(true) {
                p.battery = None;
            }
        }
        None => notes.push(format!("no {FEATURES_FILE}: feature settings are template defaults")),
    }

    match load_json(dir, KEYS_FILE) {
        Some(k) => {
            if let (Some(h), Some(m)) = (&mut p.hotkeys, k["map"].as_object()) {
                h.map = m.iter().filter_map(|(c, a)| a.as_str().map(|a| (c.clone(), a.to_string()))).collect();
            }
        }
        None => notes.push(format!("no {KEYS_FILE}: hotkey map is the template's")),
    }
    p.validate()?;
    Ok((p, notes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slug("Aspire A715-79G"), "aspire-a715-79g");
        assert_eq!(slug("  Nitro AN515_58 "), "nitro-an515-58");
    }

    #[test]
    fn endian_decision() {
        let mk = |label: &str, be: u32, le: u32| json!({"label": label, "fans": [{"rpm_be": be, "rpm_le": le}]});
        let mut s = Vec::new();
        for _ in 0..6 {
            s.push(mk("set100", 6300, 40));
            s.push(mk("set30", 2500, 90));
        }
        assert_eq!(decide_endian(&s), Some(Endian::Big));
    }

    #[test]
    fn generate_from_probe_files() {
        let dir = std::env::temp_dir().join(format!("aoc-gen-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FANS_FILE), r#"{"rpm_endian": "Big", "audible": true}"#).unwrap();
        std::fs::write(
            dir.join(FEATURES_FILE),
            r#"{"keyboard_rgb": true, "channel_shift": {"red": 8, "green": 0, "blue": 16},
                "perf": {"limits": {"2": {"pl1_w": 90, "pl2_w": 115}}},
                "webcam": {"works": true, "off_value": 0}, "battery": {"readable": true}}"#,
        )
        .unwrap();
        std::fs::write(dir.join(KEYS_FILE), r#"{"map": {"0xFD": "touchpad-toggle", "0x81": "kbd-down"}}"#).unwrap();
        let m = Machine { product_name: "Aspire A715-99X".into(), ..Machine::default() };
        let (p, _notes) = generate(&dir, &m).unwrap();
        assert_eq!(p.model.id, "acer-aspire-a715-99x");
        assert_eq!(p.model.status, profile::Status::Experimental);
        assert_eq!(p.hotkeys.as_ref().unwrap().map.len(), 2);
        assert_eq!(p.performance.as_ref().unwrap().by_index(2).unwrap().description, "CPU 90 W / 115 W");
        // Output must round-trip through TOML and validate.
        let text = toml::to_string_pretty(&p).unwrap();
        Profile::from_toml(&text).unwrap();
        std::fs::remove_dir_all(dir).ok();
    }
}
