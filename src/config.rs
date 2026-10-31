//! User configuration (TOML).

use anyhow::{Context, Result, bail};
use chrono::NaiveTime;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const DEFAULT_CONFIG: &str = include_str!("../config.example.toml");

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
#[derive(Default)]
pub struct Config {
    pub general: General,
    pub color: Color,
    pub schedule: Schedule,
    #[serde(rename = "rules")]
    pub rules: Vec<Rule>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct General {
    /// How often to check processes and re-apply the gamma ramp.
    pub poll_interval_secs: u64,
    /// Seconds to fade when switching between rule/normal state. 0 = instant.
    pub fade_secs: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Color {
    pub day_temperature: u32,
    pub night_temperature: u32,
    pub day_brightness: f32,
    pub night_brightness: f32,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleMode {
    /// Follow the sun at `latitude`/`longitude`.
    Solar,
    /// Fixed clock times.
    Fixed,
    /// Always use the night setting.
    AlwaysNight,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Schedule {
    pub mode: ScheduleMode,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    /// "HH:MM", local time. Fixed mode only.
    pub night_start: String,
    pub night_end: String,
    /// Fixed mode: how long the fade into/out of night takes.
    pub transition_minutes: u32,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum When {
    /// Rule applies while the process exists at all.
    Running,
    /// Rule applies only while the process owns the foreground window.
    #[default]
    Focused,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    #[serde(default)]
    pub name: Option<String>,
    /// Process names, case-insensitive; a trailing ".exe" is optional
    /// ("cs2" matches both `cs2.exe` on Windows and `cs2` on Linux).
    pub processes: Vec<String>,
    #[serde(default)]
    pub when: When,
    /// If neither `temperature` nor `brightness` is set the rule disables Ember
    /// (neutral 6500K, full brightness) while it matches.
    #[serde(default)]
    pub temperature: Option<u32>,
    #[serde(default)]
    pub brightness: Option<f32>,
}

impl Rule {
    pub fn label(&self) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| self.processes.join(", "))
    }
}

impl Default for General {
    fn default() -> Self {
        General {
            poll_interval_secs: 2,
            fade_secs: 0.5,
        }
    }
}

impl Default for Color {
    fn default() -> Self {
        Color {
            day_temperature: 6500,
            night_temperature: 3400,
            day_brightness: 1.0,
            night_brightness: 1.0,
        }
    }
}

impl Default for Schedule {
    fn default() -> Self {
        Schedule {
            mode: ScheduleMode::Fixed,
            latitude: None,
            longitude: None,
            night_start: "21:00".into(),
            night_end: "07:00".into(),
            transition_minutes: 45,
        }
    }
}

pub fn parse_hhmm(s: &str) -> Result<NaiveTime> {
    NaiveTime::parse_from_str(s.trim(), "%H:%M")
        .with_context(|| format!("invalid time {s:?}, expected HH:MM"))
}

impl Config {
    pub fn parse(text: &str) -> Result<Config> {
        let cfg: Config = toml::from_str(text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn load(path: &Path) -> Result<Config> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Config::parse(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn validate(&self) -> Result<()> {
        let s = &self.schedule;
        match s.mode {
            ScheduleMode::Solar => match (s.latitude, s.longitude) {
                (Some(lat), Some(lon))
                    if (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon) => {}
                (Some(_), Some(_)) => bail!("schedule.latitude/longitude out of range"),
                _ => bail!(
                    "schedule.mode = \"solar\" requires schedule.latitude and schedule.longitude"
                ),
            },
            ScheduleMode::Fixed => {
                parse_hhmm(&s.night_start)?;
                parse_hhmm(&s.night_end)?;
            }
            ScheduleMode::AlwaysNight => {}
        }
        for (i, r) in self.rules.iter().enumerate() {
            if r.processes.iter().all(|p| p.trim().is_empty()) {
                bail!("rules[{i}] has no process names");
            }
        }
        Ok(())
    }
}

pub fn default_config_path() -> PathBuf {
    directories::ProjectDirs::from("", "", "ember")
        .map(|d| d.config_dir().join("config.toml"))
        .unwrap_or_else(|| PathBuf::from("ember.toml"))
}

pub fn state_dir() -> PathBuf {
    directories::ProjectDirs::from("", "", "ember")
        .map(|d| d.data_local_dir().to_path_buf())
        .unwrap_or_else(std::env::temp_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_config_parses() {
        let cfg = Config::parse(DEFAULT_CONFIG).unwrap();
        assert!(
            cfg.rules
                .iter()
                .any(|r| r.processes.iter().any(|p| p == "cs2"))
        );
    }

    #[test]
    fn empty_config_uses_defaults() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }

    #[test]
    fn solar_requires_location() {
        assert!(Config::parse("[schedule]\nmode = \"solar\"").is_err());
        assert!(
            Config::parse("[schedule]\nmode = \"solar\"\nlatitude = 59.9\nlongitude = 10.7")
                .is_ok()
        );
    }

    #[test]
    fn typos_are_rejected() {
        assert!(Config::parse("[color]\nnight_temp = 3000").is_err());
    }
}
