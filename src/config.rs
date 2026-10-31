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

impl Config {
    /// Is there a rule that fully disables Ember while `app` (normalised name) is focused or running?
    pub fn has_disable_rule(&self, app: &str) -> bool {
        self.rules.iter().any(|r| {
            r.temperature.is_none()
                && r.brightness.is_none()
                && r.processes
                    .iter()
                    .any(|p| crate::rules::normalize(p) == app)
        })
    }
}

/// Add (`enable = true`) or remove a "disable Ember while <app> is focused" rule,
/// editing the TOML in place so comments and formatting are preserved.
pub fn edit_text_set_app_rule(text: &str, app: &str, enable: bool) -> Result<String> {
    use toml_edit::{Array, ArrayOfTables, DocumentMut, Item, Table, value};
    let mut doc: DocumentMut = text.parse().context("parsing config")?;
    if !doc.contains_key("rules") {
        doc.insert("rules", Item::ArrayOfTables(ArrayOfTables::new()));
    }
    let rules = doc["rules"]
        .as_array_of_tables_mut()
        .context("`rules` must be written as [[rules]] tables")?;

    if enable {
        let mut t = Table::new();
        t.insert("name", value(app));
        let mut procs = Array::new();
        procs.push(app);
        t.insert("processes", value(procs));
        t.insert("when", value("focused"));
        rules.push(t);
    } else {
        // Strip `app` from every disable-rule; drop rules left with no processes.
        let is_disable =
            |t: &Table| !t.contains_key("temperature") && !t.contains_key("brightness");
        let mut empty = Vec::new();
        for (i, t) in rules.iter_mut().enumerate() {
            if !is_disable(t) {
                continue;
            }
            if let Some(arr) = t.get_mut("processes").and_then(|p| p.as_array_mut()) {
                arr.retain(|v| v.as_str().is_none_or(|s| crate::rules::normalize(s) != app));
                if arr.is_empty() {
                    empty.push(i);
                }
            }
        }
        for i in empty.into_iter().rev() {
            rules.remove(i);
        }
    }
    let out = doc.to_string();
    Config::parse(&out).context("edited config would be invalid")?;
    Ok(out)
}

/// File-level wrapper around [`edit_text_set_app_rule`]. Creates the config if missing.
pub fn set_app_rule(path: &Path, app: &str, enable: bool) -> Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DEFAULT_CONFIG.to_string(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let out = edit_text_set_app_rule(&text, app, enable)?;
    write_atomic(path, &out)
}

pub fn ensure_exists(path: &Path) -> Result<()> {
    if !path.exists() {
        write_atomic(path, DEFAULT_CONFIG)?;
    }
    Ok(())
}

fn write_atomic(path: &Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
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
    fn add_and_remove_app_rule_preserves_comments() {
        let text = "# my comment\n[color]\nnight_temperature = 3000 # warm\n";
        let added = edit_text_set_app_rule(text, "valorant", true).unwrap();
        assert!(added.contains("# my comment") && added.contains("# warm"));
        let cfg = Config::parse(&added).unwrap();
        assert!(cfg.has_disable_rule("valorant"));
        assert_eq!(cfg.rules[0].when, When::Focused);

        let removed = edit_text_set_app_rule(&added, "valorant", false).unwrap();
        let cfg = Config::parse(&removed).unwrap();
        assert!(!cfg.has_disable_rule("valorant"));
        assert!(cfg.rules.is_empty());
        assert!(removed.contains("# my comment"));
    }

    #[test]
    fn remove_keeps_other_processes_and_override_rules() {
        let text = r#"
[[rules]]
processes = ["cs2.exe", "valorant"]
[[rules]]
processes = ["cs2"]
temperature = 5000
"#;
        let out = edit_text_set_app_rule(text, "cs2", false).unwrap();
        let cfg = Config::parse(&out).unwrap();
        assert_eq!(cfg.rules.len(), 2);
        assert_eq!(cfg.rules[0].processes, vec!["valorant"]);
        assert_eq!(cfg.rules[1].temperature, Some(5000));
    }

    #[test]
    fn add_to_example_config() {
        let out = edit_text_set_app_rule(DEFAULT_CONFIG, "mpv", true).unwrap();
        let cfg = Config::parse(&out).unwrap();
        assert!(cfg.has_disable_rule("mpv") && cfg.has_disable_rule("cs2"));
    }

    #[test]
    fn typos_are_rejected() {
        assert!(Config::parse("[color]\nnight_temp = 3000").is_err());
    }
}
