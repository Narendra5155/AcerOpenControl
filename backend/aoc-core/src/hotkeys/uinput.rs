//! Minimal uinput virtual keyboard used to re-emit keys GNOME handles natively
//! (currently only the touchpad toggle).

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const KEY_KBDILLUMTOGGLE: u16 = 228;
pub const KEY_KBDILLUMDOWN: u16 = 229;
pub const KEY_KBDILLUMUP: u16 = 230;
pub const KEY_TOUCHPAD_TOGGLE: u16 = 530;
pub const KEY_TOUCHPAD_ON: u16 = 531;
pub const KEY_TOUCHPAD_OFF: u16 = 532;

const UI_SET_EVBIT: libc::c_ulong = 0x4004_5564;
const UI_SET_KEYBIT: libc::c_ulong = 0x4004_5565;
const UI_DEV_SETUP: libc::c_ulong = 0x405C_5503;
const UI_DEV_CREATE: libc::c_ulong = 0x5501;
const UI_DEV_DESTROY: libc::c_ulong = 0x5502;
const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const BUS_HOST: u16 = 0x19;

#[repr(C)]
struct UinputSetup {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
    name: [u8; 80],
    ff_effects_max: u32,
}

#[repr(C)]
struct InputEvent {
    sec: libc::time_t,
    usec: libc::suseconds_t,
    kind: u16,
    code: u16,
    value: i32,
}

pub struct VirtualKeyboard {
    file: File,
}

fn ioctl_int(f: &File, req: libc::c_ulong, v: libc::c_int) -> io::Result<()> {
    // SAFETY: uinput ioctls taking an int argument.
    if unsafe { libc::ioctl(f.as_raw_fd(), req, v) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

impl VirtualKeyboard {
    pub fn create(name: &str, keys: &[u16]) -> io::Result<VirtualKeyboard> {
        let file = OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open("/dev/uinput")?;
        ioctl_int(&file, UI_SET_EVBIT, EV_KEY as i32)?;
        for &k in keys {
            ioctl_int(&file, UI_SET_KEYBIT, k as i32)?;
        }
        let mut setup =
            UinputSetup { bustype: BUS_HOST, vendor: 0xAC37, product: 1, version: 1, name: [0; 80], ff_effects_max: 0 };
        let n = name.len().min(79);
        setup.name[..n].copy_from_slice(&name.as_bytes()[..n]);
        // SAFETY: UI_DEV_SETUP with a correctly laid out uinput_setup.
        if unsafe { libc::ioctl(file.as_raw_fd(), UI_DEV_SETUP, &setup as *const UinputSetup) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: no argument.
        if unsafe { libc::ioctl(file.as_raw_fd(), UI_DEV_CREATE) } < 0 {
            return Err(io::Error::last_os_error());
        }
        std::thread::sleep(Duration::from_millis(200)); // let udev/libinput pick it up
        Ok(VirtualKeyboard { file })
    }

    fn emit(&mut self, kind: u16, code: u16, value: i32) -> io::Result<()> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        let ev = InputEvent { sec: now.as_secs() as _, usec: now.subsec_micros() as _, kind, code, value };
        // SAFETY: InputEvent is repr(C) plain data.
        let bytes = unsafe {
            std::slice::from_raw_parts(&ev as *const InputEvent as *const u8, std::mem::size_of::<InputEvent>())
        };
        self.file.write_all(bytes)
    }

    pub fn tap(&mut self, key: u16) -> io::Result<()> {
        self.emit(EV_KEY, key, 1)?;
        self.emit(EV_SYN, 0, 0)?;
        self.emit(EV_KEY, key, 0)?;
        self.emit(EV_SYN, 0, 0)
    }
}

impl Drop for VirtualKeyboard {
    fn drop(&mut self) {
        // SAFETY: destroying the device we created.
        unsafe { libc::ioctl(self.file.as_raw_fd(), UI_DEV_DESTROY) };
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn struct_sizes_match_kernel_abi() {
        assert_eq!(std::mem::size_of::<super::UinputSetup>(), 92);
        assert_eq!(std::mem::size_of::<super::InputEvent>(), 24);
    }
}
