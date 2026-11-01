//! Platform-neutral tray logic: what the menu shows and what each entry does.
//! The Windows (tray-icon) and Linux (StatusNotifierItem) front-ends only render this.

use crate::daemon::{Handle, State};
use crate::engine::{self, Reason};
use crate::hotkey::Hotkey;
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
    /// Open the settings window, next to the tray icon if we know where it is.
    OpenSettings(Option<Anchor>),
    OpenConfig,
    Quit,
}

/// Where the tray icon is on screen, in physical pixels. Passed to the settings window as
/// `--anchor x,y,width,height` so it can open right next to the icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl std::fmt::Display for Anchor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{},{},{},{}", self.x, self.y, self.width, self.height)
    }
}

impl std::str::FromStr for Anchor {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let n: Vec<i32> = s
            .split(',')
            .map(|p| p.trim().parse())
            .collect::<Result<_, _>>()
            .map_err(|_| format!("expected x,y,width,height, got {s:?}"))?;
        match n[..] {
            [x, y, width, height] => Ok(Anchor {
                x,
                y,
                width,
                height,
            }),
            _ => Err(format!("expected x,y,width,height, got {s:?}")),
        }
    }
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
    /// The configured pause/resume hotkey, shown next to "Enabled".
    pub hotkey: Option<Hotkey>,
}

/// The pause/resume hotkey in the config the daemon is running with. `None` until the daemon
/// has loaded it, or if it's turned off.
pub fn hotkey(state: &State) -> Option<Hotkey> {
    match state {
        State::Running(s) => Hotkey::parse_config(&s.config.general.toggle_hotkey)
            .ok()
            .flatten(),
        _ => None,
    }
}

pub fn view(state: &State) -> View {
    let enabled = engine::paused().is_none();
    let autostart = crate::autostart::is_enabled();
    let hotkey = hotkey(state);
    match state {
        State::Starting => View {
            status: "Starting…".into(),
            tooltip: "Ember".into(),
            active: false,
            enabled,
            app_rule: None,
            autostart,
            hotkey,
        },
        State::Failed(e) => View {
            status: format!("Error: {}", truncate(e, 60)),
            tooltip: format!("Ember — error: {e}"),
            active: false,
            enabled,
            app_rule: None,
            autostart,
            hotkey,
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
                hotkey,
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
        Action::OpenSettings(anchor) => {
            let exe = std::env::current_exe()?;
            let mut cmd = std::process::Command::new(exe);
            cmd.arg("--settings").arg("--config").arg(config_path);
            if let Some(a) = anchor {
                cmd.arg("--anchor").arg(a.to_string());
            }
            cmd.spawn()?;
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

/// Draw the tray icon: a crescent moon cradling a round ember. Glowing yellow to red while
/// Ember is tinting the screen, flat grey when it isn't.
/// Returns straight (non-premultiplied) RGBA, row-major.
pub fn icon_rgba(size: u32, active: bool) -> Vec<u8> {
    const SS: u32 = 4; // samples per pixel along each axis
    let mut px = vec![0u8; (size * size * 4) as usize];
    let n = (size * SS) as f32;
    for y in 0..size {
        for x in 0..size {
            let (mut rgb, mut cover) = ([0.0f32; 3], 0.0f32);
            for sy in 0..SS {
                for sx in 0..SS {
                    // Shape coordinates run 0..1 across the icon, y pointing down.
                    let u = ((x * SS + sx) as f32 + 0.5) / n;
                    let v = ((y * SS + sy) as f32 + 0.5) / n;
                    let (dist, heat) = icon_shape(u, v);
                    let a = (0.5 - dist * size as f32).clamp(0.0, 1.0);
                    if a > 0.0 {
                        let c = if active {
                            heat_colour(heat)
                        } else {
                            [150.0; 3]
                        };
                        for k in 0..3 {
                            rgb[k] += c[k] * a;
                        }
                        cover += a;
                    }
                }
            }
            if cover > 0.0 {
                let i = ((y * size + x) * 4) as usize;
                for k in 0..3 {
                    px[i + k] = (rgb[k] / cover).round() as u8;
                }
                px[i + 3] = (cover / (SS * SS) as f32 * 255.0).round() as u8;
            }
        }
    }
    px
}

/// At a point of the icon (0..1 across, y down): the signed distance to the outline in
/// icon-widths (negative inside), and how hot it glows from 0 (hottest) to 1.
fn icon_shape(x: f32, y: f32) -> (f32, f32) {
    let dist = |cx: f32, cy: f32| (x - cx).hypot(y - cy);
    // Crescent: a disc with a smaller disc bitten out of its upper right.
    let (bite_x, bite_y, bite_r) = (0.67, 0.34, 0.34);
    let moon = (dist(0.45, 0.55) - 0.42).max(bite_r - dist(bite_x, bite_y));
    // The ember, sitting in the curve of the moon.
    let (ember_x, ember_y, ember_r) = (0.70, 0.34, 0.12);
    let ember = dist(ember_x, ember_y) - ember_r;
    if ember < moon {
        let heat = (dist(ember_x, ember_y) / ember_r).min(1.0) * 0.45;
        (ember, heat)
    } else {
        // The moon glows hottest along its inner edge and cools towards the rim.
        let heat = ((dist(bite_x, bite_y) - bite_r) / 0.30)
            .clamp(0.0, 1.0)
            .powf(0.7);
        (moon, heat)
    }
}

/// Hot yellow (0) through orange to deep coal red (1).
fn heat_colour(t: f32) -> [f32; 3] {
    const HOT: [f32; 3] = [255.0, 214.0, 102.0];
    const MID: [f32; 3] = [255.0, 140.0, 40.0];
    const DEEP: [f32; 3] = [214.0, 54.0, 30.0];
    let (from, to, f) = if t < 0.5 {
        (HOT, MID, t * 2.0)
    } else {
        (MID, DEEP, t * 2.0 - 1.0)
    };
    std::array::from_fn(|k| from[k] + (to[k] - from[k]) * f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_has_expected_size_and_transparent_corners() {
        let px = icon_rgba(64, true);
        assert_eq!(px.len(), 64 * 64 * 4);
        let alpha = |px: &[u8], x: usize, y: usize| px[(y * 64 + x) * 4 + 3];
        assert_eq!(alpha(&px, 0, 0), 0, "corner should be transparent");
        assert_eq!(alpha(&px, 19, 44), 255, "moon body is solid");
        assert_eq!(alpha(&px, 33, 25), 0, "moon's curve is empty");
        assert_eq!(alpha(&px, 44, 21), 255, "ember sits in the curve");

        let off = icon_rgba(64, false);
        for (on, off) in px.chunks(4).zip(off.chunks(4)) {
            assert_eq!(on[3], off[3], "same shape when inactive");
            assert!(
                off[3] == 0 || (off[0] == off[1] && off[1] == off[2]),
                "inactive is grey"
            );
        }
    }

    #[test]
    fn anchor_round_trips_through_its_argument() {
        let a = Anchor {
            x: 1830,
            y: -1040,
            width: 24,
            height: 32,
        };
        assert_eq!(a.to_string().parse::<Anchor>(), Ok(a));
        assert!("1,2,3".parse::<Anchor>().is_err());
        assert!("1,2,3,x".parse::<Anchor>().is_err());
    }

    #[test]
    fn starting_view() {
        let v = view(&State::Starting);
        assert!(!v.active);
        assert!(v.app_rule.is_none());
    }
}
