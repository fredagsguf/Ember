//! The decision loop: schedule + rules + pause -> what to put on screen.

use crate::color::ColorSetting;
use crate::config::{Config, When};
use crate::gamma::Backend;
use crate::rules::{self, Snapshot};
use crate::schedule;
use chrono::Local;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

#[derive(Debug, Clone, PartialEq)]
pub enum Reason {
    Schedule,
    Rule(String),
    Paused,
}

impl std::fmt::Display for Reason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Reason::Schedule => write!(f, "schedule"),
            Reason::Rule(r) => write!(f, "rule \"{r}\""),
            Reason::Paused => write!(f, "paused"),
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

    pub fn snapshot(&mut self, cfg: &Config, backend: &mut dyn Backend) -> Snapshot {
        let mut snap = Snapshot::default();
        let need_all = cfg.rules.iter().any(|r| r.when == When::Running);
        let need_focus = cfg.rules.iter().any(|r| r.when == When::Focused);
        let focused_pid = if need_focus {
            backend.focused_pid()
        } else {
            None
        };

        let kind = ProcessRefreshKind::nothing();
        if need_all {
            self.sys
                .refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
            snap.running = self
                .sys
                .processes()
                .values()
                .map(|p| rules::normalize(&p.name().to_string_lossy()))
                .collect();
        } else if let Some(pid) = focused_pid {
            // Only look up the one process we care about; much cheaper than a full scan.
            self.sys.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[Pid::from_u32(pid)]),
                true,
                kind,
            );
        }
        if let Some(pid) = focused_pid {
            snap.focused = self
                .sys
                .process(Pid::from_u32(pid))
                .map(|p| rules::normalize(&p.name().to_string_lossy()));
        }
        snap
    }

    /// Name of the focused process, for `ember processes`.
    pub fn focused_name(&mut self, backend: &mut dyn Backend) -> Option<String> {
        let pid = Pid::from_u32(backend.focused_pid()?);
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing(),
        );
        self.sys
            .process(pid)
            .map(|p| rules::normalize(&p.name().to_string_lossy()))
    }

    /// All running processes, for `ember processes`.
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

pub fn decide(cfg: &Config, snap: &Snapshot) -> Decision {
    let scheduled = schedule::scheduled_setting(cfg, Local::now());
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
