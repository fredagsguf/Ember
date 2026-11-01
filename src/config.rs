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
    /// Global hotkey that pauses/resumes Ember, e.g. "Ctrl+Alt+End". Empty = no hotkey.
    pub toggle_hotkey: String,
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
            toggle_hotkey: "Ctrl+Alt+End".into(),
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
        crate::hotkey::Hotkey::parse_config(&self.general.toggle_hotkey)
            .context("general.toggle_hotkey")?;
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

fn round_to(x: f64, places: i32) -> f64 {
    let m = 10f64.powi(places);
    (x * m).round() / m
}

/// Rewrite `text` so it parses to `new`, touching only the keys whose values changed.
/// Comments, ordering and formatting of everything else are preserved.
pub fn edit_text_apply(text: &str, new: &Config) -> Result<String> {
    use toml_edit::{Array, ArrayOfTables, DocumentMut, Item, Table, Value};

    new.validate()?;
    let old = Config::parse(text).unwrap_or_default();
    let mut doc: DocumentMut = text.parse().context("parsing config")?;

    fn table<'a>(doc: &'a mut DocumentMut, name: &str) -> Result<&'a mut Table> {
        if !doc.contains_key(name) {
            doc.insert(name, Item::Table(Table::new()));
        }
        doc[name]
            .as_table_mut()
            .with_context(|| format!("[{name}] must be a table"))
    }
    /// Replace a value but keep its surrounding whitespace and trailing comment.
    fn set(t: &mut Table, key: &str, v: impl Into<Value>) {
        let mut v: Value = v.into();
        match t.get_mut(key) {
            Some(item) => {
                if let Some(old) = item.as_value() {
                    *v.decor_mut() = old.decor().clone();
                }
                *item = Item::Value(v);
            }
            None => {
                t.insert(key, Item::Value(v));
            }
        }
    }
    fn set_opt(t: &mut Table, key: &str, v: Option<Value>) {
        match v {
            Some(v) => set(t, key, v),
            None => {
                t.remove(key);
            }
        }
    }
    let f2 = |x: f32| round_to(x as f64, 2);
    let int = |x: u32| x as i64;
    let when = |w: When| match w {
        When::Focused => "focused",
        When::Running => "running",
    };
    let mode = |m: ScheduleMode| match m {
        ScheduleMode::Solar => "solar",
        ScheduleMode::Fixed => "fixed",
        ScheduleMode::AlwaysNight => "always_night",
    };

    macro_rules! sync {
        ($t:expr, $key:literal, $old:expr, $new:expr, $conv:expr) => {
            if $old != $new {
                set($t, $key, $conv($new));
            }
        };
    }

    let (o, n) = (&old.general, &new.general);
    if o != n {
        let t = table(&mut doc, "general")?;
        sync!(
            t,
            "poll_interval_secs",
            o.poll_interval_secs,
            n.poll_interval_secs,
            |x: u64| x as i64
        );
        sync!(t, "fade_secs", o.fade_secs, n.fade_secs, |x: f64| round_to(
            x, 2
        ));
        sync!(
            t,
            "toggle_hotkey",
            o.toggle_hotkey.as_str(),
            n.toggle_hotkey.as_str(),
            |x: &str| x.to_string()
        );
    }
    let (o, n) = (&old.color, &new.color);
    if o != n {
        let t = table(&mut doc, "color")?;
        sync!(
            t,
            "day_temperature",
            o.day_temperature,
            n.day_temperature,
            int
        );
        sync!(
            t,
            "night_temperature",
            o.night_temperature,
            n.night_temperature,
            int
        );
        sync!(t, "day_brightness", o.day_brightness, n.day_brightness, f2);
        sync!(
            t,
            "night_brightness",
            o.night_brightness,
            n.night_brightness,
            f2
        );
    }
    let (o, n) = (&old.schedule, &new.schedule);
    if o != n {
        let t = table(&mut doc, "schedule")?;
        sync!(t, "mode", o.mode, n.mode, mode);
        if o.latitude != n.latitude {
            set_opt(t, "latitude", n.latitude.map(|x| round_to(x, 4).into()));
        }
        if o.longitude != n.longitude {
            set_opt(t, "longitude", n.longitude.map(|x| round_to(x, 4).into()));
        }
        sync!(
            t,
            "night_start",
            o.night_start.as_str(),
            n.night_start.as_str(),
            |x: &str| x.to_string()
        );
        sync!(
            t,
            "night_end",
            o.night_end.as_str(),
            n.night_end.as_str(),
            |x: &str| x.to_string()
        );
        sync!(
            t,
            "transition_minutes",
            o.transition_minutes,
            n.transition_minutes,
            int
        );
    }

    if old.rules != new.rules {
        if !doc.contains_key("rules") {
            doc.insert("rules", Item::ArrayOfTables(ArrayOfTables::new()));
        }
        let arr = doc["rules"]
            .as_array_of_tables_mut()
            .context("`rules` must be written as [[rules]] tables")?;
        while arr.len() > new.rules.len() {
            arr.remove(arr.len() - 1);
        }
        for (i, r) in new.rules.iter().enumerate() {
            if i >= arr.len() {
                arr.push(Table::new());
            }
            let prev = old.rules.get(i);
            if prev == Some(r) {
                continue;
            }
            let t = arr.get_mut(i).expect("index checked above");
            if prev.map(|p| &p.name) != Some(&r.name) {
                set_opt(t, "name", r.name.clone().map(Value::from));
            }
            if prev.map(|p| &p.processes) != Some(&r.processes) {
                set(
                    t,
                    "processes",
                    r.processes.iter().map(String::as_str).collect::<Array>(),
                );
            }
            if prev.map(|p| p.when) != Some(r.when) {
                set(t, "when", when(r.when));
            }
            if prev.map(|p| p.temperature) != Some(r.temperature) {
                set_opt(t, "temperature", r.temperature.map(|x| int(x).into()));
            }
            if prev.map(|p| p.brightness) != Some(r.brightness) {
                set_opt(t, "brightness", r.brightness.map(|x| f2(x).into()));
            }
        }
    }

    let out = doc.to_string();
    let check = Config::parse(&out).context("edited config would be invalid")?;
    if &check != new {
        bail!("internal error: saved config does not round-trip");
    }
    Ok(out)
}

/// Save `cfg` to `path`, preserving the existing file's comments where possible.
pub fn save(path: &Path, cfg: &Config) -> Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DEFAULT_CONFIG.to_string(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    write_atomic(path, &edit_text_apply(&text, cfg)?)
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
    fn apply_round_trips_and_keeps_comments() {
        let mut cfg = Config::parse(DEFAULT_CONFIG).unwrap();
        cfg.color.night_temperature = 2700;
        cfg.color.night_brightness = 0.8;
        cfg.schedule.mode = ScheduleMode::Solar;
        cfg.schedule.latitude = Some(59.91);
        cfg.schedule.longitude = Some(10.75);
        cfg.general.fade_secs = 1.25;
        cfg.rules[0].temperature = Some(5000);
        cfg.rules.push(Rule {
            name: Some("Valorant".into()),
            processes: vec!["valorant".into(), "valorant-win64-shipping".into()],
            when: When::Running,
            temperature: None,
            brightness: Some(0.9),
        });
        let out = edit_text_apply(DEFAULT_CONFIG, &cfg).unwrap();
        assert_eq!(Config::parse(&out).unwrap(), cfg);
        // untouched comments survive, including the inline one on a changed value
        assert!(out.contains("# Ember configuration."));
        assert!(out.contains("night_temperature = 2700 # lower = warmer/redder"));
        assert!(out.contains("# \"solar\"        – follow sunset/sunrise"));

        // and removing things works too
        cfg.rules.clear();
        cfg.schedule.latitude = None;
        cfg.schedule.longitude = None;
        cfg.schedule.mode = ScheduleMode::AlwaysNight;
        let out2 = edit_text_apply(&out, &cfg).unwrap();
        assert_eq!(Config::parse(&out2).unwrap(), cfg);
        assert!(!out2.contains("\n[[rules]]"));
    }

    #[test]
    fn apply_without_changes_is_identity() {
        let cfg = Config::parse(DEFAULT_CONFIG).unwrap();
        assert_eq!(
            edit_text_apply(DEFAULT_CONFIG, &cfg).unwrap(),
            DEFAULT_CONFIG
        );
    }

    #[test]
    fn apply_to_empty_file() {
        let mut cfg = Config::default();
        cfg.color.night_temperature = 3000;
        let out = edit_text_apply("", &cfg).unwrap();
        assert_eq!(Config::parse(&out).unwrap(), cfg);
    }

    #[test]
    fn typos_are_rejected() {
        assert!(Config::parse("[color]\nnight_temp = 3000").is_err());
        let e = Config::parse("[general]\ntoggle_hotkey = \"Ctrl+Alt+Ennd\"").unwrap_err();
        assert!(format!("{e:#}").contains("toggle_hotkey"), "{e:#}");
    }

    #[test]
    fn toggle_hotkey_defaults_and_can_be_disabled() {
        assert_eq!(Config::default().general.toggle_hotkey, "Ctrl+Alt+End");
        let cfg = Config::parse("[general]\ntoggle_hotkey = \"\"").unwrap();
        assert_eq!(cfg.general.toggle_hotkey, "");
    }

    #[test]
    fn apply_writes_toggle_hotkey() {
        let mut cfg = Config::parse(DEFAULT_CONFIG).unwrap();
        cfg.general.toggle_hotkey = "Ctrl+Shift+F12".into();
        let out = edit_text_apply(DEFAULT_CONFIG, &cfg).unwrap();
        assert_eq!(Config::parse(&out).unwrap(), cfg);
        assert!(out.contains("toggle_hotkey = \"Ctrl+Shift+F12\""));

        // Missing from the file (older configs): written into [general] when changed.
        cfg.general.toggle_hotkey = String::new();
        let out = edit_text_apply("[general]\nfade_secs = 1.0\n", &cfg).unwrap();
        assert_eq!(Config::parse(&out).unwrap().general.toggle_hotkey, "");
    }
}
