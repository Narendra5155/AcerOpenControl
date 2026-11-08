//! Expose the keyboard backlight as `/sys/class/leds/aoc::kbd_backlight`
//! through `uleds`, so UPower (and GNOME, when it picks the LED up at login)
//! can control it. Writes to the LED arrive as 4-byte ints on the fd.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

pub const LED_NAME: &str = "aoc::kbd_backlight";
const LED_MAX_NAME_SIZE: usize = 64;
const NO_SUPPRESS: i32 = -1;

pub fn pack_uleds(name: &str, max_brightness: i32) -> Vec<u8> {
    let mut v = vec![0u8; LED_MAX_NAME_SIZE];
    let n = name.len().min(LED_MAX_NAME_SIZE - 1);
    v[..n].copy_from_slice(&name.as_bytes()[..n]);
    v.extend_from_slice(&max_brightness.to_ne_bytes());
    v
}

pub struct KbdLed {
    file: File,
    suppress: Arc<AtomicI32>,
}

impl KbdLed {
    pub fn create(max_brightness: u8) -> io::Result<KbdLed> {
        if !Path::new("/dev/uleds").exists() {
            let _ = Command::new("modprobe").arg("uleds").status();
        }
        let mut file = OpenOptions::new().read(true).write(true).open("/dev/uleds")?;
        file.write_all(&pack_uleds(LED_NAME, max_brightness as i32))?;
        log::info!("created /sys/class/leds/{LED_NAME}");
        Ok(KbdLed { file, suppress: Arc::new(AtomicI32::new(NO_SUPPRESS)) })
    }

    /// Mirror a brightness change made elsewhere (app, CLI, hotkey) into the
    /// LED, without echoing it back to the firmware.
    pub fn sync(&self, brightness: u8) {
        self.suppress.store(brightness as i32, Ordering::SeqCst);
        if fs::write(format!("/sys/class/leds/{LED_NAME}/brightness"), brightness.to_string()).is_err() {
            self.suppress.store(NO_SUPPRESS, Ordering::SeqCst);
        }
    }

    /// Spawn the reader thread; `on_change` gets brightness values written by
    /// other programs (UPower).
    pub fn spawn_reader(&self, on_change: impl Fn(u8) + Send + 'static) -> io::Result<()> {
        let mut f = self.file.try_clone()?;
        let suppress = self.suppress.clone();
        std::thread::Builder::new().name("kbd-led".into()).spawn(move || {
            let mut buf = [0u8; 4];
            while f.read_exact(&mut buf).is_ok() {
                let v = i32::from_ne_bytes(buf);
                if suppress.compare_exchange(v, NO_SUPPRESS, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                    continue;
                }
                on_change(v.clamp(0, 255) as u8);
            }
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn uleds_struct() {
        let v = super::pack_uleds("aoc::kbd_backlight", 255);
        assert_eq!(v.len(), 68);
        assert_eq!(&v[..18], b"aoc::kbd_backlight");
        assert_eq!(i32::from_ne_bytes(v[64..68].try_into().unwrap()), 255);
    }
}
