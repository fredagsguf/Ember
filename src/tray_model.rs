//! Platform-neutral tray logic: what the menu shows and what each entry does.
//! The Windows (tray-icon) and Linux (StatusNotifierItem) front-ends only render this.

use crate::daemon::{Handle, State};
use crate::engine::{self, Reason};
use anyhow::Result;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Pause indefinitely / resume.
    ToggleEnabled,
    DisableFor(u64),
    /// Add/remove a disable rule for the most recently used app.
    ToggleAppRule,
    ToggleAutostart,
    OpenSettings,
    OpenConfig,
    Quit,
}

pub const DISABLE_OPTIONS: &[(u64, &str)] = &[(15, "15 minutes"), (60, "1 hour"), (180, "3 hours")];

/// Everything a front-end needs to draw the menu and icon.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    /// e.g. "3400K — night" or "Off — rule: Counter-Strike 2"
    pub status: String,
    pub tooltip: String,
    /// Ember is tinting the screen right now (drives the icon colour).
    pub active: bool,
    /// Not paused by the user.
    pub enabled: bool,
    /// (label, checked) for "Disable while <app> is focused".
    pub app_rule: Option<(String, bool)>,
    pub autostart: bool,
}

pub fn view(state: &State) -> View {
    let enabled = engine::paused().is_none();
    let autostart = crate::autostart::is_enabled();
    match state {
        State::Starting => View {
            status: "Starting…".into(),
            tooltip: "Ember".into(),
            active: false,
            enabled,
            app_rule: None,
            autostart,
        },
        State::Failed(e) => View {
            status: format!("Error: {}", truncate(e, 60)),
            tooltip: format!("Ember — error: {e}"),
            active: false,
            enabled,
            app_rule: None,
            autostart,
        },
        State::Running(s) => {
            let k = s.setting.temperature;
            let status = match &s.reason {
                Reason::Schedule if k == crate::color::NEUTRAL_K => "Daytime — no tint".to_string(),
                Reason::Schedule => format!("{k}K"),
                Reason::Rule(name) if k == crate::color::NEUTRAL_K => format!("Off while {name}"),
                Reason::Rule(name) => format!("{k}K for {name}"),
                Reason::Preview => format!("Previewing {k}K"),
                Reason::Paused => match engine::paused() {
                    Some(Some(until)) => format!("Paused for {} more min", minutes_until(until)),
                    _ => "Paused".into(),
                },
            };
            let app_rule = s.last_app.as_ref().map(|app| {
                (
                    format!("Disable while \"{app}\" is focused"),
                    s.config.has_disable_rule(app),
                )
            });
            View {
                tooltip: format!("Ember — {status}"),
                status,
                active: k != crate::color::NEUTRAL_K,
                enabled,
                app_rule,
                autostart,
            }
        }
    }
}

fn minutes_until(unix: u64) -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    unix.saturating_sub(now).div_ceil(60)
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>() + "…"
    }
}

/// Perform a menu action. Returns `true` if the app should exit.
pub fn perform(action: Action, daemon: &Handle, config_path: &Path) -> Result<bool> {
    match action {
        Action::ToggleEnabled => {
            if engine::paused().is_some() {
                engine::clear_pause()?;
            } else {
                engine::set_pause(None)?;
            }
        }
        Action::DisableFor(m) => engine::set_pause(Some(m))?,
        Action::ToggleAppRule => {
            if let State::Running(s) = daemon.state()
                && let Some(app) = &s.last_app
            {
                let has = s.config.has_disable_rule(app);
                crate::config::set_app_rule(config_path, app, !has)?;
                log::info!(
                    "{} disable rule for {app}",
                    if has { "removed" } else { "added" }
                );
            }
        }
        Action::ToggleAutostart => crate::autostart::set_enabled(!crate::autostart::is_enabled())?,
        Action::OpenSettings => {
            let exe = std::env::current_exe()?;
            std::process::Command::new(exe)
                .arg("--settings")
                .arg("--config")
                .arg(config_path)
                .spawn()?;
        }
        Action::OpenConfig => {
            crate::config::ensure_exists(config_path)?;
            open_in_default_app(config_path)?;
        }
        Action::Quit => {
            daemon.stop();
            return Ok(true);
        }
    }
    daemon.wake();
    Ok(false)
}

#[cfg(windows)]
pub fn open_in_default_app(path: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let wide = |s: &std::ffi::OsStr| s.encode_wide().chain(Some(0)).collect::<Vec<u16>>();
    let file = wide(path.as_os_str());
    let verb = wide("open".as_ref());
    let r = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecute returns a value <= 32 on failure (e.g. no app associated with .toml).
    if (r as isize) <= 32 {
        let notepad = wide("notepad.exe".as_ref());
        unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                notepad.as_ptr(),
                file.as_ptr(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn open_in_default_app(path: &Path) -> Result<()> {
    std::process::Command::new("xdg-open").arg(path).spawn()?;
    Ok(())
}

/// Log file for the tray app (it has no console on Windows).
pub fn log_path() -> PathBuf {
    crate::config::state_dir().join("ember.log")
}

/// Draw the tray icon: a glowing ember when active, a grey ring when not.
/// Returns straight (non-premultiplied) RGBA, row-major.
pub fn icon_rgba(size: u32, active: bool) -> Vec<u8> {
    let mut px = vec![0u8; (size * size * 4) as usize];
    let c = (size as f32 - 1.0) / 2.0;
    let r_outer = size as f32 * 0.46;
    for y in 0..size {
        for x in 0..size {
            let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
            // 1px anti-aliased edge
            let cover = (r_outer + 0.5 - d).clamp(0.0, 1.0);
            if cover <= 0.0 {
                continue;
            }
            let t = (d / r_outer).clamp(0.0, 1.0);
            let (rgb, alpha) = if active {
                // Hot yellow core fading to deep orange-red.
                let mix = |a: f32, b: f32| a + (b - a) * t;
                ([mix(255.0, 226.0), mix(200.0, 74.0), mix(90.0, 28.0)], 1.0)
            } else {
                // Grey ring.
                let ring = ((d - r_outer * 0.62) * 2.0).clamp(0.0, 1.0);
                ([150.0, 150.0, 150.0], ring)
            };
            let i = ((y * size + x) * 4) as usize;
            px[i] = rgb[0] as u8;
            px[i + 1] = rgb[1] as u8;
            px[i + 2] = rgb[2] as u8;
            px[i + 3] = (alpha * cover * 255.0) as u8;
        }
    }
    px
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_has_expected_size_and_transparent_corners() {
        let px = icon_rgba(32, true);
        assert_eq!(px.len(), 32 * 32 * 4);
        assert_eq!(px[3], 0, "corner should be transparent");
        let centre = ((16 * 32 + 16) * 4) as usize;
        assert_eq!(px[centre + 3], 255);
        let off = icon_rgba(32, false);
        assert_eq!(off[centre + 3], 0, "inactive icon is a ring");
    }

    #[test]
    fn starting_view() {
        let v = view(&State::Starting);
        assert!(!v.active);
        assert!(v.app_rule.is_none());
    }
}
