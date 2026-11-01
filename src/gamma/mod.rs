//! Platform backends that actually change the screen colours.

use crate::color::ColorSetting;
use anyhow::Result;

pub mod calibration;
#[cfg(target_os = "linux")]
mod kwin;
#[cfg(target_os = "linux")]
mod session;
#[cfg(windows)]
mod windows;
#[cfg(target_os = "linux")]
mod wlroots;
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
    /// Wayland app_id of the focused window, for compositors that can't tell the PID.
    /// Only consulted when `focused_pid` returns `None`.
    fn focused_app_id(&mut self) -> Option<String> {
        None
    }
    /// Whether `apply` shows exactly the colour asked for, so the settings window can preview
    /// slider values with it. False when the desktop picks the colour itself (KDE Night Light).
    fn live_preview(&self) -> bool {
        true
    }
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
        let env = |k: &str| std::env::var(k).ok();
        if session::is_wayland(
            env("WAYLAND_DISPLAY").as_deref(),
            env("XDG_SESSION_TYPE").as_deref(),
        ) {
            return open_wayland(&session::desktop(env("XDG_CURRENT_DESKTOP").as_deref()));
        }
        return Ok(Box::new(x11::RandR::open(preserve_calibration)?));
    }
    #[allow(unreachable_code)]
    {
        let _ = preserve_calibration;
        anyhow::bail!("this platform is not supported yet")
    }
}

/// Wayland has no common gamma API: use wlroots' protocol if the compositor has it, else KDE's
/// Night Light, else explain what is supported.
#[cfg(target_os = "linux")]
fn open_wayland(desktop: &session::Desktop) -> Result<Box<dyn Backend>> {
    use anyhow::Context;

    if let Some(b) = wlroots::Wlr::open()? {
        return Ok(Box::new(b));
    }
    log::debug!("wayland: compositor has no wlr-gamma-control");
    match kwin::NightLight::open() {
        Ok(b) => Ok(Box::new(b)),
        // On KDE the KWin error (Night Light switched off, too old…) is the useful part.
        Err(e) if *desktop == session::Desktop::Kde => {
            Err(e).context(session::unsupported_message(desktop))
        }
        Err(e) => {
            log::debug!("wayland: KWin Night Light unavailable: {e:#}");
            anyhow::bail!(session::unsupported_message(desktop))
        }
    }
}
