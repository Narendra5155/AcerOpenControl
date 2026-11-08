//! Fn hotkeys routed by the EC through the Clevo WMI event.
//!
//! The firmware only delivers them after a driver registers (`enable_cmd`,
//! which sets `WMI.HKDR`); sleep clears that flag. Each key arrives as an ACPI
//! netlink event; the key code is then read with `read_cmd` (`WMI.EVNT`).

pub mod netlink;
pub mod uinput;

use crate::ec::Clevo;
use crate::profile::HotkeysCfg;
use crate::Result;
use netlink::AcpiEvent;

/// Does this ACPI event come from the profile's hotkey notify?
pub fn is_hotkey_event(cfg: &HotkeysCfg, ev: &AcpiEvent) -> bool {
    ev.kind == cfg.notify
        && (ev.bus_id.to_uppercase().starts_with(&cfg.event_guid_prefix.to_uppercase())
            || ev.bus_id.contains("PNP0C14")
            || ev.device_class.eq_ignore_ascii_case("wmi"))
}

/// Register as the hotkey driver. The firmware returns a feature word here,
/// not the command id, so this uses a raw call.
pub fn enable(ec: &Clevo, cfg: &HotkeysCfg) -> Result<()> {
    ec.raw(cfg.enable_cmd, 0).map(|_| ())
}

/// Read the code of the hotkey that was just pressed.
pub fn read_code(ec: &Clevo, cfg: &HotkeysCfg) -> Result<u8> {
    Ok((ec.get(cfg.read_cmd, 0)? & 0xFF) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::a715;

    #[test]
    fn filter() {
        let p = a715();
        let cfg = p.hotkeys.as_ref().unwrap();
        let ev =
            |class: &str, bus: &str, kind| AcpiEvent { device_class: class.into(), bus_id: bus.into(), kind, data: 0 };
        assert!(is_hotkey_event(cfg, &ev("", "ABBC0F6B-8EA1-1", 0xD0)));
        assert!(!is_hotkey_event(cfg, &ev("ac_adapter", "ACPI0003:00", 0x80)));
        assert!(!is_hotkey_event(cfg, &ev("", "ABBC0F6B-8EA1-1", 0x80)));
    }
}
