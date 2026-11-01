//! Platform backends that actually change the screen colours.

use crate::color::ColorSetting;
use anyhow::Result;

pub mod calibration;
#[cfg(windows)]
mod windows;
#[cfg(target_os = "linux")]
mod x11;

pub trait Backend {
    fn name(&self) -> &'static str;
    /// Set the gamma ramps on every output: each display's base curve tinted by `setting`.
    fn apply(&mut self, setting: ColorSetting) -> Result<()>;
    /// Return every output to its base curve: the calibration curve (ICC/VCGT) found on it
    /// when the backend opened, or neutral (identity) if there was none or preserving is off.
    fn restore(&mut self) -> Result<()>;
    /// Block until previously sent changes have taken effect (needed before exiting).
    fn sync(&mut self) -> Result<()> {
        Ok(())
    }
    /// Build ramps on each display's calibration curve (`true`) or on identity (`false`).
    fn set_preserve_calibration(&mut self, _on: bool) {}
    /// PID of the process that owns the foreground window, if the platform can tell.
    fn focused_pid(&mut self) -> Option<u32>;
}

/// Backend that only logs. Useful for testing rules without touching the display.
pub struct DryRun;

impl Backend for DryRun {
    fn name(&self) -> &'static str {
        "dry-run"
    }
    fn apply(&mut self, s: ColorSetting) -> Result<()> {
        log::debug!(
            "dry-run: would apply {}K @ {:.0}%",
            s.temperature,
            s.brightness * 100.0
        );
        Ok(())
    }
    fn restore(&mut self) -> Result<()> {
        Ok(())
    }
    fn focused_pid(&mut self) -> Option<u32> {
        None
    }
}

/// `preserve_calibration`: see [`Backend::set_preserve_calibration`].
pub fn open(dry_run: bool, preserve_calibration: bool) -> Result<Box<dyn Backend>> {
    if dry_run {
        return Ok(Box::new(DryRun));
    }
    #[cfg(windows)]
    {
        return Ok(Box::new(windows::Gdi::open(preserve_calibration)?));
    }
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_some() {
            anyhow::bail!(
                "native Wayland is not supported yet (needs wlr-gamma-control / desktop-specific APIs). \
                 Ember currently works on X11 sessions on Linux."
            );
        }
        return Ok(Box::new(x11::RandR::open(preserve_calibration)?));
    }
    #[allow(unreachable_code)]
    {
        let _ = preserve_calibration;
        anyhow::bail!("this platform is not supported yet")
    }
}
