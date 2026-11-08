//! aocctl: command-line control for AcerOpenControl.

mod client;
mod probe;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

use aoc_core::profile::{self, Machine, Profile};

#[derive(Parser)]
#[command(name = "aocctl", version, about = "Control fans, keyboard light and firmware features (AcerOpenControl)")]
struct Cli {
    /// aocd socket path
    #[arg(long, global = true, default_value = client::SOCKET_PATH)]
    socket: String,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show temperatures, fan speeds, mode and feature state
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Set the fan mode: auto (BIOS), silent, balanced, performance, max, custom
    Mode { mode: String },
    /// Show the custom curve, or set it from TEMP:DUTY pairs (e.g. 50:0 70:40 90:100)
    Curve { points: Vec<String> },
    /// Keyboard backlight
    Kbd {
        /// RRGGBB hex, or white/red/green/blue/orange/yellow/purple/cyan/pink
        #[arg(long, short)]
        color: Option<String>,
        /// 0..255, or "off"
        #[arg(long, short)]
        brightness: Option<String>,
    },
    /// Firmware performance profile (a key from `aocctl model`), or "bios" to stop managing it
    Profile { profile: String },
    /// Battery charge limit: a stop percentage (50..100), or "off"
    Charge {
        stop: String,
        /// Resume charging below this percentage (default: stop - 5)
        #[arg(long)]
        start: Option<i64>,
    },
    /// Turn the webcam on or off at the firmware level
    Webcam { state: OnOff },
    /// Show the detected model profile and its capabilities
    Model {
        #[arg(long)]
        json: bool,
    },
    /// Ask the daemon to resend all settings (used after resume)
    Reapply,
    /// Hand the fans back to the BIOS directly, without the daemon (root)
    RestoreAuto,
    /// Hardware probes for building a profile for a new model (root)
    #[command(subcommand)]
    Probe(ProbeCmd),
    /// Model profile tools
    #[command(subcommand, name = "profiles")]
    Profiles(ProfilesCmd),
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum OnOff {
    On,
    Off,
}

#[derive(Subcommand)]
enum ProbeCmd {
    /// Briefly drive the fans to 100% and 30% to learn the RPM encoding (stop aocd first)
    Fans {
        #[arg(long)]
        base: Option<PathBuf>,
        #[arg(long, default_value = ".")]
        out: PathBuf,
    },
    /// Interactive test of keyboard light, performance profiles, webcam and battery
    Features {
        #[arg(long)]
        base: Option<PathBuf>,
        #[arg(long, default_value = ".")]
        out: PathBuf,
    },
    /// Learn Fn hotkey codes (wizard), or print raw events with --raw SECONDS
    Keys {
        #[arg(long)]
        base: Option<PathBuf>,
        #[arg(long, default_value = ".")]
        out: PathBuf,
        #[arg(long)]
        raw: Option<u64>,
    },
}

#[derive(Subcommand)]
enum ProfilesCmd {
    /// List installed profiles and which one matches this machine
    List,
    /// Validate a profile file
    Check { file: PathBuf },
    /// Print a draft profile built from probe results in DIR (default: current directory)
    Generate {
        #[arg(default_value = ".")]
        dir: PathBuf,
    },
}

const COLOURS: &[(&str, &str)] = &[
    ("white", "ffffff"),
    ("red", "ff0000"),
    ("green", "00ff00"),
    ("blue", "0000ff"),
    ("orange", "ff6000"),
    ("yellow", "ffcc00"),
    ("purple", "8000ff"),
    ("cyan", "00ffff"),
    ("pink", "ff2080"),
];

fn parse_colour(s: &str) -> Result<(u8, u8, u8)> {
    let hx = COLOURS.iter().find(|(n, _)| n.eq_ignore_ascii_case(s)).map(|(_, h)| *h).unwrap_or(s);
    let hx = hx.trim_start_matches('#');
    if hx.len() != 6 {
        bail!("colour must be RRGGBB or a name like orange");
    }
    let p = |i: usize| u8::from_str_radix(&hx[i..i + 2], 16).context("bad hex colour");
    Ok((p(0)?, p(2)?, p(4)?))
}

fn fmt_temp(v: &Value) -> String {
    v.as_f64().map_or("n/a".into(), |t| format!("{t:.0}°C"))
}

fn num(v: &Value) -> String {
    match v.as_f64() {
        Some(x) if x.fract() == 0.0 => format!("{x:.0}"),
        Some(x) => format!("{x}"),
        None => v.to_string(),
    }
}

fn print_status(st: &Value, model: Option<&Value>) {
    let ctl = if let Some(holder) = st["external"]["holder"].as_str() {
        let fans: Vec<String> = st["external"]["fans"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(i, d)| match d.as_u64() {
                Some(d) => format!("fan{} {d}%", i + 1),
                None => format!("fan{} BIOS", i + 1),
            })
            .collect();
        format!("{holder}: {}", fans.join(", "))
    } else if st["bios_control"] == json!(true) {
        "BIOS".to_string()
    } else {
        format!("{}%", st["target"])
    };
    if st["read_only"] == json!(true) {
        println!("!! READ-ONLY: no model profile matches this machine (see `aocctl profiles list`)");
    }
    println!("Mode      : {}   (control: {ctl})", st["mode"].as_str().unwrap_or("?"));
    println!("CPU       : {}", fmt_temp(&st["cpu"]));
    let gpu = if st["gpu_asleep"] == json!(true) { "asleep".into() } else { fmt_temp(&st["gpu"]) };
    println!("GPU       : {gpu}");
    for (i, f) in st["fans"].as_array().into_iter().flatten().enumerate() {
        println!(
            "Fan {} {:<4}: {:>5} rpm  ({}%)",
            i + 1,
            f["label"].as_str().unwrap_or(""),
            f["rpm"].as_u64().unwrap_or(0),
            f["duty"].as_u64().unwrap_or(0)
        );
    }
    let f = &st["features"];
    if let Some(k) = f.get("kbd").filter(|k| !k.is_null()) {
        println!(
            "Keyboard  : #{:02x}{:02x}{:02x}  brightness {}",
            k["r"].as_u64().unwrap_or(0),
            k["g"].as_u64().unwrap_or(0),
            k["b"].as_u64().unwrap_or(0),
            k["brightness"]
        );
    }
    if let Some(p) = f.get("perf_profile").and_then(Value::as_u64) {
        let label = model
            .and_then(|m| m["perf_profiles"].as_array())
            .and_then(|ps| ps.iter().find(|x| x["index"].as_u64() == Some(p)))
            .and_then(|x| x["label"].as_str().map(str::to_string))
            .unwrap_or_else(|| p.to_string());
        println!("Profile   : {label}");
    }
    if let Some(c) = f.get("charge").filter(|c| !c.is_null()) {
        if c["enabled"] == json!(true) {
            println!("Charging  : limited, stops at {}% (resumes below {}%)", c["stop"], c["start"]);
        } else {
            println!("Charging  : no limit");
        }
    }
    if let Some(w) = f.get("webcam").and_then(Value::as_bool) {
        println!("Webcam    : {}", if w { "on" } else { "off" });
    }
    if st["emergency"] == json!(true) {
        println!("!! EMERGENCY: temperature limit reached, fans forced to 100%");
    }
    if let Some(e) = st["fault"].as_str() {
        println!("!! FAULT: {e}");
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    if let Err(e) = run(Cli::parse()) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    let sock = cli.socket.as_str();
    match cli.cmd {
        Cmd::Status { json } => {
            let st = client::request(sock, &json!({"cmd": "status"}))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&st)?);
            } else {
                let model = client::request(sock, &json!({"cmd": "get_model"})).ok();
                print_status(&st, model.as_ref());
            }
        }
        Cmd::Mode { mode } => {
            client::call(sock, json!({"cmd": "set_mode", "mode": mode}))?;
            println!("ok");
        }
        Cmd::Curve { points } if points.is_empty() => {
            let c = client::call(sock, json!({"cmd": "get_config"}))?;
            let pts: Vec<String> = c["config"]["custom_curve"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|p| format!("{}°C→{}%", num(&p[0]), num(&p[1])))
                .collect();
            println!("{}", pts.join("  "));
        }
        Cmd::Curve { points } => {
            let pts = points
                .iter()
                .map(|p| {
                    let (t, d) = p.split_once(':').context("points must look like 50:0 70:40 90:100")?;
                    Ok(json!([t.parse::<f64>()?, d.parse::<f64>()?]))
                })
                .collect::<Result<Vec<_>>>()?;
            client::call(sock, json!({"cmd": "set_curve", "points": pts}))?;
            println!("ok");
        }
        Cmd::Kbd { color, brightness } => {
            let mut req = json!({"cmd": "set_kbd"});
            if let Some(c) = color {
                let (r, g, b) = parse_colour(&c)?;
                req["r"] = json!(r);
                req["g"] = json!(g);
                req["b"] = json!(b);
            }
            if let Some(b) = brightness {
                req["brightness"] =
                    json!(if b == "off" { 0 } else { b.parse::<u8>().context("brightness must be 0..255 or off")? });
            }
            if req.as_object().unwrap().len() == 1 {
                bail!("give --color and/or --brightness");
            }
            client::call(sock, req)?;
            println!("ok");
        }
        Cmd::Profile { profile } => {
            client::call(sock, json!({"cmd": "set_perf_profile", "profile": profile}))?;
            println!("ok");
        }
        Cmd::Charge { stop, start } => {
            let req = if stop == "off" {
                json!({"cmd": "set_charge_limit", "enabled": false, "start": 95, "stop": 100})
            } else {
                let stop: i64 = stop.parse().context("stop must be a percentage or 'off'")?;
                json!({"cmd": "set_charge_limit", "enabled": true, "stop": stop, "start": start.unwrap_or(stop - 5)})
            };
            client::call(sock, req)?;
            println!("ok");
        }
        Cmd::Webcam { state } => {
            client::call(sock, json!({"cmd": "set_webcam", "on": matches!(state, OnOff::On)}))?;
            println!("ok");
        }
        Cmd::Model { json } => {
            let m = client::call(sock, json!({"cmd": "get_model"}))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&m)?);
            } else if m["model"].is_null() {
                println!(
                    "No model profile matches this machine ({}). Daemon is read-only.",
                    m["machine"]["product_name"]
                );
            } else {
                println!(
                    "{} ({}), status: {}",
                    m["model"]["name"].as_str().unwrap_or(""),
                    m["model"]["id"].as_str().unwrap_or(""),
                    m["model"]["status"].as_str().unwrap_or("")
                );
                println!("profile file: {}", m["path"].as_str().unwrap_or("?"));
                println!("capabilities: {}", m["capabilities"]);
                for p in m["perf_profiles"].as_array().into_iter().flatten() {
                    println!(
                        "  profile {:<12} {}",
                        p["key"].as_str().unwrap_or(""),
                        p["description"].as_str().unwrap_or("")
                    );
                }
            }
        }
        Cmd::Reapply => {
            client::call(sock, json!({"cmd": "reapply"}))?;
            println!("ok");
        }
        Cmd::RestoreAuto => restore_auto()?,
        Cmd::Probe(p) => match p {
            ProbeCmd::Fans { base, out } => probe::fans(base.as_deref(), &out)?,
            ProbeCmd::Features { base, out } => probe::features(base.as_deref(), &out)?,
            ProbeCmd::Keys { base, out, raw } => probe::keys(base.as_deref(), &out, raw)?,
        },
        Cmd::Profiles(p) => profiles(p)?,
    }
    Ok(())
}

/// Used by systemd's ExecStopPost: never leave fans under manual control.
fn restore_auto() -> Result<()> {
    probe::require_root()?;
    let machine = Machine::detect();
    let (loaded, _) = profile::load_default_dirs();
    let cfg = aoc_core::config::Config::load(std::path::Path::new(aoc_core::config::CONFIG_PATH));
    let Some(l) = profile::select(&loaded, &machine, cfg.force_profile.as_deref()) else {
        println!("no model profile matches this machine; nothing to restore");
        return Ok(());
    };
    if l.profile.fans.is_none() {
        return Ok(());
    }
    aoc_core::acpi::ensure_module()?;
    let ec = aoc_core::ec::Clevo::new(Arc::new(aoc_core::acpi::AcpiCall), Arc::new(l.profile.clone()));
    ec.set_auto()?;
    println!("fans returned to BIOS automatic control");
    Ok(())
}

fn profiles(cmd: ProfilesCmd) -> Result<()> {
    match cmd {
        ProfilesCmd::List => {
            let machine = Machine::detect();
            let (loaded, errors) = profile::load_default_dirs();
            println!(
                "This machine: {:?} (board {:?}, BIOS {:?})",
                machine.product_name, machine.board_name, machine.bios_version
            );
            let clevo = machine.has_wmi_guid("ABBC0F6D-8EA1-11D1-00A0-C90629100000");
            println!("Clevo WMI interface: {}", if clevo { "present" } else { "NOT present" });
            println!();
            for l in &loaded {
                let mark = if l.profile.matches(&machine) { "*" } else { " " };
                println!("{mark} {:<28} {:<12?} {}", l.profile.model.id, l.profile.model.status, l.path.display());
            }
            for e in errors {
                println!("! {e}");
            }
            if !loaded.iter().any(|l| l.profile.matches(&machine)) {
                println!("\nNo profile matches. To add your model, see docs/ADDING_A_MODEL.md");
            }
        }
        ProfilesCmd::Check { file } => {
            let p = Profile::load(&file)?;
            println!("{}: OK ({} / {:?})", file.display(), p.model.id, p.model.status);
            let m = Machine::detect();
            println!("matches this machine: {}", if p.matches(&m) { "yes" } else { "no" });
        }
        ProfilesCmd::Generate { dir } => {
            let (p, notes) = probe::generate(&dir, &Machine::detect())?;
            println!("# Draft profile generated by `aocctl profiles generate`. Review before use.");
            for n in &notes {
                println!("# NOTE: {n}");
            }
            println!(
                "# Install for testing: sudo cp FILE /etc/acer-open-control/models.d/ && sudo systemctl restart aocd\n"
            );
            print!("{}", toml::to_string_pretty(&p)?);
        }
    }
    Ok(())
}
