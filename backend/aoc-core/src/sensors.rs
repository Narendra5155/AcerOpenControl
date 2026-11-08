//! Temperatures. hwmon devices are matched by name, never by index.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// What the daemon needs from the temperature side (mockable in tests).
pub trait Sensors: Send {
    fn cpu(&self) -> Option<f64>;
    fn gpu_asleep(&self) -> bool;
    fn gpu(&self) -> Option<f64>;
}

fn read(p: &Path) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

fn dirs(pattern_dir: &str, prefix: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(pattern_dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

pub fn find_hwmon(name: &str) -> Option<PathBuf> {
    dirs("/sys/class/hwmon", "hwmon").into_iter().find(|d| read(&d.join("name")).as_deref() == Some(name))
}

/// CPU package temperature (coretemp "Package id 0"), else the hottest core,
/// else the x86_pkg_temp thermal zone.
pub fn cpu_temp() -> Option<f64> {
    if let Some(d) = find_hwmon("coretemp") {
        let mut best: Option<f64> = None;
        for entry in fs::read_dir(&d).ok()?.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().to_string();
            let Some(stem) = name.strip_suffix("_label") else { continue };
            let label = read(&entry.path()).unwrap_or_default();
            let Some(v) = read(&d.join(format!("{stem}_input"))).and_then(|s| s.parse::<f64>().ok()) else {
                continue;
            };
            let t = v / 1000.0;
            if label.starts_with("Package") {
                return Some(t);
            }
            best = Some(best.map_or(t, |b: f64| b.max(t)));
        }
        if best.is_some() {
            return best;
        }
    }
    for z in dirs("/sys/class/thermal", "thermal_zone") {
        if read(&z.join("type")).as_deref() == Some("x86_pkg_temp") {
            return read(&z.join("temp")).and_then(|s| s.parse::<f64>().ok()).map(|v| v / 1000.0);
        }
    }
    None
}

pub fn nvidia_pci_dev() -> Option<PathBuf> {
    dirs("/sys/bus/pci/devices", "").into_iter().find(|d| {
        read(&d.join("vendor")).as_deref() == Some("0x10de")
            && read(&d.join("class")).is_some_and(|c| c.starts_with("0x03"))
    })
}

/// Real sensors. The GPU is never woken: if the dGPU is runtime-suspended its
/// temperature is reported as unavailable instead of calling nvidia-smi.
pub struct SystemSensors {
    gpu_dev: Option<PathBuf>,
    cache: Mutex<(Option<Instant>, Option<f64>)>,
    cache_for: Duration,
}

impl Default for SystemSensors {
    fn default() -> Self {
        SystemSensors { gpu_dev: nvidia_pci_dev(), cache: Mutex::new((None, None)), cache_for: Duration::from_secs(2) }
    }
}

impl Sensors for SystemSensors {
    fn cpu(&self) -> Option<f64> {
        cpu_temp()
    }

    fn gpu_asleep(&self) -> bool {
        match &self.gpu_dev {
            None => true,
            Some(d) => read(&d.join("power/runtime_status")).as_deref() == Some("suspended"),
        }
    }

    fn gpu(&self) -> Option<f64> {
        if self.gpu_asleep() {
            return None;
        }
        let mut c = self.cache.lock().unwrap();
        if let Some(t) = c.0 {
            if t.elapsed() < self.cache_for {
                return c.1;
            }
        }
        let v = Command::new("nvidia-smi")
            .args(["--query-gpu=temperature.gpu", "--format=csv,noheader,nounits"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.lines().next().and_then(|l| l.trim().parse::<f64>().ok()));
        *c = (Some(Instant::now()), v);
        v
    }
}
