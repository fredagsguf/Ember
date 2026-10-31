//! Platform backends that actually change the screen colours.

use crate::color::ColorSetting;
use anyhow::Result;

#[cfg(windows)]
mod windows;
#[cfg(target_os = "linux")]
mod x11;

pub trait Backend {
    fn name(&self) -> &'static str;
    /// Set the gamma ramps on every output.
    fn apply(&mut self, setting: ColorSetting) -> Result<()>;
    /// Return every output to a neutral (identity) ramp.
    /// Note: this does not preserve ICC/VCGT calibration ramps loaded by other tools.
    fn restore(&mut self) -> Result<()>;
    /// Block until previously sent changes have taken effect (needed before exiting).
    fn sync(&mut self) -> Result<()> {
        Ok(())
    }
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

pub fn open(dry_run: bool) -> Result<Box<dyn Backend>> {
    if dry_run {
        return Ok(Box::new(DryRun));
    }
    #[cfg(windows)]
    {
        return Ok(Box::new(windows::Gdi::open()?));
    }
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_some() {
            anyhow::bail!(
                "native Wayland is not supported yet (needs wlr-gamma-control / desktop-specific APIs). \
                 Ember currently works on X11 sessions on Linux."
            );
        }
        return Ok(Box::new(x11::RandR::open()?));
    }
    #[allow(unreachable_code)]
    {
        anyhow::bail!("this platform is not supported yet")
    }
}
