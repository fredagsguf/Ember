//! The decision loop: schedule + rules + pause -> what to put on screen.

use crate::color::ColorSetting;
use crate::config::{Config, Rule, When};
use crate::gamma::Backend;
use crate::rules::{self, Snapshot};
use crate::schedule;
use chrono::Local;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System, UpdateKind};

#[derive(Debug, Clone, PartialEq)]
pub enum Reason {
    Schedule,
    Rule(String),
    Paused,
    /// The settings window is showing a slider value on screen.
    Preview,
}

impl std::fmt::Display for Reason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Reason::Schedule => write!(f, "schedule"),
            Reason::Rule(r) => write!(f, "rule \"{r}\""),
            Reason::Paused => write!(f, "paused"),
            Reason::Preview => write!(f, "settings preview"),
        }
    }
}

pub struct Decision {
    pub setting: ColorSetting,
    pub scheduled: ColorSetting,
    pub reason: Reason,
}

/// Collects running/focused process names.
pub struct ProcessWatcher {
    sys: System,
}

impl ProcessWatcher {
    pub fn new() -> anyhow::Result<Self> {
        let sys = System::new_with_specifics(RefreshKind::nothing())
            .map_err(|e| anyhow::anyhow!("process enumeration unavailable: {e:?}"))?;
        Ok(ProcessWatcher { sys })
    }

    /// Running processes (only scanned if a rule needs them), the focused process and window.
    pub fn snapshot(&mut self, cfg: &Config, backend: &mut dyn Backend) -> Snapshot {
        let mut snap = Snapshot::default();
        let running = |r: &&Rule| r.when == When::Running;
        let focused = |r: &&Rule| r.when == When::Focused;
        if cfg.rules.iter().any(|r| running(&r)) {
            let steam = cfg.rules.iter().filter(running).any(|r| r.steam_games);
            self.sys.refresh_processes_specifics(
                ProcessesToUpdate::All,
                true,
                steam_details(steam),
            );
            snap.running = self
                .sys
                .processes()
                .values()
                .map(|p| rules::normalize(&p.name().to_string_lossy()))
                .collect();
            snap.steam_running = steam
                && self.sys.processes().values().any(|p| {
                    let name = rules::normalize(&p.name().to_string_lossy());
                    rules::is_steam_game(&name, p.exe(), p.environ())
                });
        }
        let steam = cfg.rules.iter().filter(focused).any(|r| r.steam_games);
        (snap.focused, snap.steam_focused) = self.focused_process(backend, steam);
        if cfg
            .rules
            .iter()
            .filter(focused)
            .any(|r| r.fullscreen || r.titles().next().is_some())
        {
            let w = backend.focused_window();
            snap.title = w.title;
            snap.fullscreen = w.fullscreen;
        }
        snap
    }

    /// Name of the process owning the foreground window. Only refreshes that one process.
    /// On Wayland compositors that only report the window's app_id, that is the name instead.
    pub fn focused_name(&mut self, backend: &mut dyn Backend) -> Option<String> {
        self.focused_process(backend, false).0
    }

    /// The focused process's name, and (if `steam` is asked for) whether it's a Steam game.
    fn focused_process(
        &mut self,
        backend: &mut dyn Backend,
        steam: bool,
    ) -> (Option<String>, bool) {
        let Some(pid) = backend.focused_pid() else {
            let id = backend.focused_app_id().map(|id| rules::normalize(&id));
            // wlroots compositors see Proton games as `steam_app_<id>`.
            let is_steam = steam && id.as_deref().is_some_and(|id| id.starts_with("steam_app_"));
            return (id, is_steam);
        };
        let pid = Pid::from_u32(pid);
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            steam_details(steam),
        );
        let Some(p) = self.sys.process(pid) else {
            return (None, false);
        };
        let name = rules::normalize(&p.name().to_string_lossy());
        let is_steam = steam && rules::is_steam_game(&name, p.exe(), p.environ());
        (Some(name), is_steam)
    }

    /// All running processes, for the settings window's app picker.
    pub fn all_names(&mut self) -> Vec<String> {
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );
        let mut v: Vec<String> = self
            .sys
            .processes()
            .values()
            .map(|p| rules::normalize(&p.name().to_string_lossy()))
            .collect();
        v.sort();
        v.dedup();
        v
    }
}

/// What to read about processes: their executable and environment only when looking for Steam
/// games, since reading another process's environment is comparatively slow.
fn steam_details(steam: bool) -> ProcessRefreshKind {
    if steam {
        ProcessRefreshKind::nothing()
            .with_exe(UpdateKind::OnlyIfNotSet)
            .with_environ(UpdateKind::OnlyIfNotSet)
    } else {
        ProcessRefreshKind::nothing()
    }
}

pub fn pause_file() -> PathBuf {
    crate::config::state_dir().join("paused_until")
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `Some(None)` = paused indefinitely, `Some(Some(t))` = paused until unix time t.
pub fn paused() -> Option<Option<u64>> {
    let text = std::fs::read_to_string(pause_file()).ok()?;
    match text.trim().parse::<u64>() {
        Ok(0) => Some(None),
        Ok(t) if t > now_unix() => Some(Some(t)),
        Ok(_) => {
            let _ = std::fs::remove_file(pause_file());
            None
        }
        Err(_) => Some(None),
    }
}

pub fn set_pause(minutes: Option<u64>) -> std::io::Result<()> {
    std::fs::create_dir_all(crate::config::state_dir())?;
    let until = minutes.map(|m| now_unix() + m * 60).unwrap_or(0);
    std::fs::write(pause_file(), until.to_string())
}

pub fn clear_pause() -> std::io::Result<()> {
    match std::fs::remove_file(pause_file()) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

fn preview_file() -> PathBuf {
    crate::config::state_dir().join("preview")
}

/// A preview counts as live this long after the settings window last wrote it.
const PREVIEW_TTL: std::time::Duration = std::time::Duration::from_millis(1500);

/// Called repeatedly by the settings window while a slider is being dragged.
pub fn set_preview(s: ColorSetting) -> std::io::Result<()> {
    std::fs::create_dir_all(crate::config::state_dir())?;
    std::fs::write(
        preview_file(),
        format!("{} {}", s.temperature, s.brightness),
    )
}

pub fn clear_preview() {
    let _ = std::fs::remove_file(preview_file());
}

pub fn preview() -> Option<ColorSetting> {
    let path = preview_file();
    let age = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()?
        .elapsed()
        .ok()?;
    if age > PREVIEW_TTL {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let mut it = text.split_whitespace();
    let temperature = it.next()?.parse().ok()?;
    let brightness = it.next()?.parse().ok()?;
    Some(
        ColorSetting {
            temperature,
            brightness,
        }
        .clamped(),
    )
}

pub fn decide(cfg: &Config, snap: &Snapshot) -> Decision {
    let scheduled = schedule::scheduled_setting(cfg, Local::now());
    if let Some(p) = preview() {
        return Decision {
            setting: p,
            scheduled,
            reason: Reason::Preview,
        };
    }
    if paused().is_some() {
        return Decision {
            setting: ColorSetting::NEUTRAL,
            scheduled,
            reason: Reason::Paused,
        };
    }
    match rules::matching_rule(&cfg.rules, snap) {
        Some(rule) => Decision {
            setting: rules::rule_setting(rule, scheduled),
            scheduled,
            reason: Reason::Rule(rule.label()),
        },
        None => Decision {
            setting: scheduled,
            scheduled,
            reason: Reason::Schedule,
        },
    }
}
