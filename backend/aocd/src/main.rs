//! aocd: the AcerOpenControl daemon (runs as root under systemd).

mod auth;
mod server;
mod service;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aoc_core::acpi::{self, AcpiCall, Firmware};
use aoc_core::config::{Config, CONFIG_PATH};
use aoc_core::hotkeys::{self, netlink::Listener, uinput};
use aoc_core::kbd_led::KbdLed;
use aoc_core::profile::{self, Machine};
use aoc_core::sensors::SystemSensors;
use service::Service;

pub const SOCKET_PATH: &str = "/run/acer-open-control/aocd.sock";

struct Args {
    socket: PathBuf,
    config: PathBuf,
    models: Vec<PathBuf>,
    fake: bool,
}

fn parse_args() -> anyhow::Result<Args> {
    let mut a = Args {
        socket: SOCKET_PATH.into(),
        config: CONFIG_PATH.into(),
        models: vec![profile::SYSTEM_MODELS_DIR.into(), profile::USER_MODELS_DIR.into()],
        fake: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = || it.next().ok_or_else(|| anyhow::anyhow!("{arg} needs a value"));
        match arg.as_str() {
            "--socket" => a.socket = val()?.into(),
            "--config" => a.config = val()?.into(),
            "--models" => a.models = vec![val()?.into()],
            "--fake" => a.fake = true,
            "--version" | "-V" => {
                println!("aocd {}", aoc_core::VERSION);
                std::process::exit(0);
            }
            "--help" | "-h" => {
                println!(
                    "aocd {}: AcerOpenControl daemon\n\n\
                     Usage: aocd [--socket PATH] [--config PATH] [--models DIR]{}\n\n\
                     Normally started by systemd (aocd.service).",
                    aoc_core::VERSION,
                    if cfg!(feature = "dev-fake") { " [--fake]" } else { "" }
                );
                std::process::exit(0);
            }
            other => anyhow::bail!("unknown argument {other} (try --help)"),
        }
    }
    Ok(a)
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).format_timestamp(None).init();
    if let Err(e) = run() {
        log::error!("{e:#}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let args = parse_args()?;
    let fake = args.fake && cfg!(feature = "dev-fake");
    if args.fake && !fake {
        anyhow::bail!("--fake needs a build with `--features dev-fake`");
    }
    // SAFETY: getuid has no preconditions.
    if !fake && unsafe { libc::getuid() } != 0 {
        anyhow::bail!("aocd must run as root");
    }

    let (service, fw_ok) = if fake { build_fake(&args) } else { build_real(&args) };
    let service = Arc::new(Mutex::new(service));

    {
        let mut s = service.lock().unwrap();
        s.apply_saved_features();
        if fw_ok {
            setup_kbd_led(&mut s, &service);
            setup_hotkeys(&mut s, &service);
        }
    }

    let listener = server::bind(&args.socket)?;
    {
        let s = service.clone();
        std::thread::Builder::new().name("server".into()).spawn(move || server::serve(listener, s))?;
    }
    log::info!("listening on {}", args.socket.display());

    let stop = Arc::new(AtomicBool::new(false));
    for sig in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT, signal_hook::consts::SIGHUP] {
        signal_hook::flag::register(sig, stop.clone())?;
    }
    while !stop.load(Ordering::Relaxed) {
        let interval = {
            let mut s = service.lock().unwrap();
            s.tick();
            Duration::from_secs_f64(s.cfg.poll_interval)
        };
        let until = Instant::now() + interval;
        while Instant::now() < until && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    service.lock().unwrap_or_else(|p| p.into_inner()).shutdown();
    let _ = std::fs::remove_file(&args.socket);
    log::info!("aocd stopped; fans returned to BIOS control");
    Ok(())
}

fn saver(path: PathBuf) -> service::SaveFn {
    Box::new(move |c: &Config| {
        if let Err(e) = c.save(&path) {
            log::error!("saving {}: {e}", path.display());
        }
    })
}

fn clock() -> service::Clock {
    let start = Instant::now();
    Box::new(move || start.elapsed().as_secs_f64())
}

fn build_real(args: &Args) -> (Service, bool) {
    let cfg = Config::load(&args.config);
    let machine = Machine::detect();
    let dirs: Vec<&Path> = args.models.iter().map(PathBuf::as_path).collect();
    let (loaded, errors) = profile::load_dirs(&dirs);
    for e in &errors {
        log::warn!("{e}");
    }
    let selected = profile::select(&loaded, &machine, cfg.force_profile.as_deref());
    match (selected, &cfg.force_profile) {
        (Some(l), Some(_)) => log::warn!("using FORCED profile {} ({})", l.profile.model.id, l.path.display()),
        (Some(l), None) => log::info!("model profile: {} ({})", l.profile.model.id, l.path.display()),
        (None, _) => log::warn!(
            "no model profile matches this machine ({:?}); running READ-ONLY. See docs/ADDING_A_MODEL.md",
            machine.product_name
        ),
    }
    let fw: Option<Arc<dyn Firmware>> = match (selected, acpi::ensure_module()) {
        (Some(_), Ok(())) => Some(Arc::new(AcpiCall)),
        (Some(_), Err(e)) => {
            log::error!("{e}; running READ-ONLY");
            None
        }
        (None, _) => None,
    };
    let fw_ok = fw.is_some();
    let mut s = Service::new(
        machine,
        selected,
        fw,
        cfg,
        Box::new(SystemSensors::default()),
        saver(args.config.clone()),
        clock(),
    );
    s.ac_path = service::find_mains_online();
    (s, fw_ok)
}

#[cfg(feature = "dev-fake")]
fn build_fake(args: &Args) -> (Service, bool) {
    use aoc_core::fake::{a715, FakeFirmware};
    struct FakeSensors;
    impl aoc_core::sensors::Sensors for FakeSensors {
        fn cpu(&self) -> Option<f64> {
            Some(72.0)
        }
        fn gpu_asleep(&self) -> bool {
            false
        }
        fn gpu(&self) -> Option<f64> {
            Some(61.0)
        }
    }
    log::warn!("FAKE mode: emulated A715-79G firmware, no hardware access");
    let loaded = profile::Loaded { profile: a715(), path: "<built-in fake>".into() };
    let machine = Machine { product_name: "Aspire A715-79G (fake)".into(), ..Machine::default() };
    let s = Service::new(
        machine,
        Some(&loaded),
        Some(Arc::new(FakeFirmware::default())),
        Config::load(&args.config),
        Box::new(FakeSensors),
        saver(args.config.clone()),
        clock(),
    );
    (s, false)
}

#[cfg(not(feature = "dev-fake"))]
fn build_fake(_: &Args) -> (Service, bool) {
    unreachable!("checked in run()")
}

fn setup_kbd_led(s: &mut Service, shared: &Arc<Mutex<Service>>) {
    let Some(max) = s.profile.as_ref().and_then(|p| p.keyboard.as_ref()).map(|k| k.max_brightness) else { return };
    if !s.cfg.features.gnome_kbd_slider {
        return;
    }
    match KbdLed::create(max) {
        Ok(led) => {
            let weak = Arc::downgrade(shared);
            let r = led.spawn_reader(move |v| {
                if let Some(svc) = weak.upgrade() {
                    let mut svc = svc.lock().unwrap_or_else(|p| p.into_inner());
                    let _ = svc.handle(&serde_json::json!({"cmd": "set_kbd", "brightness": v, "from_led": true}));
                }
            });
            if let Err(e) = r {
                log::warn!("keyboard LED reader: {e}");
            }
            if let Some(k) = s.cfg.features.saved.kbd {
                led.sync(k.brightness);
            }
            s.kbd_led = Some(led);
        }
        Err(e) => log::warn!("keyboard LED bridge unavailable: {e}"),
    }
}

fn setup_hotkeys(s: &mut Service, shared: &Arc<Mutex<Service>>) {
    let Some(cfg) = s.profile.as_ref().and_then(|p| p.hotkeys.clone()) else { return };
    if !s.cfg.features.hotkeys {
        return;
    }
    let listener = match Listener::subscribe() {
        Ok(l) => l,
        Err(e) => {
            log::warn!("hotkeys disabled: cannot subscribe to ACPI netlink events: {e}");
            return;
        }
    };
    if cfg.map.values().any(|a| a.starts_with("touchpad")) {
        let keys = [uinput::KEY_TOUCHPAD_TOGGLE, uinput::KEY_TOUCHPAD_ON, uinput::KEY_TOUCHPAD_OFF];
        match uinput::VirtualKeyboard::create("AcerOpenControl hotkeys", &keys) {
            Ok(k) => s.vkbd = Some(k),
            Err(e) => log::warn!("virtual keyboard unavailable ({e}); touchpad key won't work"),
        }
    }
    s.enable_hotkeys();
    let weak = Arc::downgrade(shared);
    let r = std::thread::Builder::new().name("hotkeys".into()).spawn(move || loop {
        let events = match listener.next_events() {
            Ok(ev) => ev,
            Err(e) => {
                log::error!("hotkey listener stopped: {e}");
                return;
            }
        };
        let Some(svc) = weak.upgrade() else { return };
        for _ev in events.iter().filter(|e| hotkeys::is_hotkey_event(&cfg, e)) {
            let mut svc = svc.lock().unwrap_or_else(|p| p.into_inner());
            let Some(ec) = svc.ec.clone() else { continue };
            match hotkeys::read_code(&ec, &cfg) {
                Ok(code) => svc.hotkey(code),
                Err(e) => log::error!("reading hotkey code failed: {e}"),
            }
        }
    });
    match r {
        Ok(_) => log::info!("Fn hotkeys enabled"),
        Err(e) => log::error!("hotkey thread: {e}"),
    }
}
