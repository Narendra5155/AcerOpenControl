//! aoc-core: hardware access for AcerOpenControl.
//!
//! Talks to Clevo-designed laptops through the firmware's `WMBB` WMI method,
//! called with the `acpi_call` kernel module. Every model-specific value comes
//! from a [`profile::Profile`].

pub mod acpi;
pub mod config;
pub mod curve;
pub mod ec;
pub mod features;
pub mod hotkeys;
pub mod kbd_led;
pub mod profile;
pub mod sensors;

#[cfg(any(test, feature = "testing"))]
pub mod fake;

pub const API_VERSION: u32 = 1;
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("ACPI: {0}")]
    Acpi(String),
    #[error("firmware call {cmd:#x} failed: returned {ret:#x}")]
    Firmware { cmd: u32, ret: u64 },
    #[error("profile: {0}")]
    Profile(String),
    #[error("invalid value: {0}")]
    Invalid(String),
    #[error("not supported by this model: {0}")]
    Unsupported(&'static str),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
