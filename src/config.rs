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
    /// Per-display settings, `[[displays]]`. Displays that aren't listed get the defaults.
    pub displays: Vec<Display>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct General {
    /// How often to check processes and re-apply the gamma ramp.
    pub poll_interval_secs: u64,
    /// Seconds to fade when switching between rule/normal state. 0 = instant.
    pub fade_secs: f64,
    /// Tint on top of each display's calibration curve (ICC/VCGT) and restore it on exit,
    /// instead of replacing it with a neutral ramp.
    pub preserve_calibration: bool,
    /// Global hotkey that pauses/resumes Ember, e.g. "Ctrl+Alt+End". Empty = no hotkey.
    pub toggle_hotkey: String,
    /// Minutes to ease in when the scheduled colour jumps instead of drifting: Ember starting
    /// in the evening, waking from sleep, an edited schedule. 0 = instant.
    pub jump_fade_minutes: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Color {
    pub day_temperature: u32,
    pub night_temperature: u32,
    pub day_brightness: f32,
    pub night_brightness: f32,
    /// The deeper colour from `schedule.bedtime` on.
    pub bedtime_temperature: u32,
    pub bedtime_brightness: f32,
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
    /// Fixed mode: other night times on some days of the week.
    pub days: Days,
    /// Solar mode: start the evening fade this many minutes later than the sun says (negative
    /// = earlier).
    pub sunset_offset_minutes: i32,
    /// Solar mode: start the morning fade this many minutes later than the sun says.
    pub sunrise_offset_minutes: i32,
    /// "HH:MM": from here until noon, night deepens to the bedtime colour. Empty = off.
    pub bedtime: String,
}

/// Night times for one evening: `night_start` on that day, `night_end` the morning after.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DayTimes {
    pub night_start: String,
    pub night_end: String,
}

/// `[schedule.days]`: evenings that differ from `night_start`/`night_end`, by the day the night
/// starts on (`fri` is Friday evening to Saturday morning).
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Days {
    pub mon: Option<DayTimes>,
    pub tue: Option<DayTimes>,
    pub wed: Option<DayTimes>,
    pub thu: Option<DayTimes>,
    pub fri: Option<DayTimes>,
    pub sat: Option<DayTimes>,
    pub sun: Option<DayTimes>,
}

impl Days {
    pub const NAMES: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

    /// The entry for `chrono::Weekday::num_days_from_monday() == i`.
    pub fn get(&self, i: usize) -> &Option<DayTimes> {
        [
            &self.mon, &self.tue, &self.wed, &self.thu, &self.fri, &self.sat, &self.sun,
        ][i]
    }

    pub fn get_mut(&mut self, i: usize) -> &mut Option<DayTimes> {
        match i {
            0 => &mut self.mon,
            1 => &mut self.tue,
            2 => &mut self.wed,
            3 => &mut self.thu,
            4 => &mut self.fri,
            5 => &mut self.sat,
            _ => &mut self.sun,
        }
    }
}

impl Schedule {
    /// `(night_start, night_end)` for the night that starts on `day`.
    pub fn times_for(&self, day: chrono::Weekday) -> (&str, &str) {
        match self.days.get(day.num_days_from_monday() as usize) {
            Some(d) => (&d.night_start, &d.night_end),
            None => (&self.night_start, &self.night_end),
        }
    }
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

/// A rule matches when every kind of condition it sets holds (any one of a list will do):
/// `processes = ["firefox"]` with `titles = ["YouTube"]` is YouTube in Firefox.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    #[serde(default)]
    pub name: Option<String>,
    /// Process names, case-insensitive; a trailing ".exe" is optional
    /// ("cs2" matches both `cs2.exe` on Windows and `cs2` on Linux).
    #[serde(default)]
    pub processes: Vec<String>,
    /// Text in the focused window's title, case-insensitive. Focused rules only.
    #[serde(default)]
    pub titles: Vec<String>,
    /// The focused window covers its whole screen. Focused rules only.
    #[serde(default)]
    pub fullscreen: bool,
    /// The app is a game started by Steam.
    #[serde(default)]
    pub steam_games: bool,
    #[serde(default)]
    pub when: When,
    /// If neither `temperature` nor `brightness` is set the rule disables Ember
    /// (neutral 6500K, full brightness) while it matches.
    #[serde(default)]
    pub temperature: Option<u32>,
    #[serde(default)]
    pub brightness: Option<f32>,
}

/// Settings for one display. Ignored with KDE Night Light, which tints every screen alike.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Display {
    /// Windows device name (`\\.\DISPLAY2`) or X11/Wayland output name (`HDMI-1`).
    /// Case-insensitive.
    pub id: String,
    /// `false` leaves the display untouched: it keeps its calibration curve (or neutral), exactly
    /// as when Ember is off.
    #[serde(default = "yes")]
    pub tint: bool,
}

fn yes() -> bool {
    true
}

impl Display {
    /// Settings for `id` with nothing changed from the defaults.
    pub fn new(id: &str) -> Display {
        Display {
            id: id.to_string(),
            tint: true,
        }
    }

    fn is(&self, id: &str) -> bool {
        self.id.trim().eq_ignore_ascii_case(id.trim())
    }
}

/// Should Ember tint display `id`? Yes unless a `[[displays]]` entry says `tint = false`.
/// X11 outputs that mirror each other share one ramp and are named `A+B`; that ramp is left
/// alone if any of them is.
pub fn tints_display(displays: &[Display], id: &str) -> bool {
    let excluded = |name: &str| displays.iter().any(|d| !d.tint && d.is(name));
    !(excluded(id) || (id.contains('+') && id.split('+').any(excluded)))
}

/// Turn tinting of display `id` on or off, dropping its entry once it matches the defaults.
pub fn set_display_tint(displays: &mut Vec<Display>, id: &str, tint: bool) {
    match displays.iter_mut().find(|d| d.is(id)) {
        Some(d) => d.tint = tint,
        None => displays.push(Display {
            tint,
            ..Display::new(id)
        }),
    }
    displays.retain(|d| !d.is(id) || *d != Display::new(&d.id));
}

impl Rule {
    pub fn label(&self) -> String {
        if let Some(n) = &self.name {
            return n.clone();
        }
        let mut parts = Vec::new();
        if !self.processes.is_empty() {
            parts.push(self.processes.join(", "));
        }
        if !self.titles.is_empty() {
            parts.push(format!("\"{}\"", self.titles.join("\", \"")));
        }
        if self.fullscreen {
            parts.push("fullscreen apps".into());
        }
        if self.steam_games {
            parts.push("Steam games".into());
        }
        parts.join(" ")
    }

    pub fn titles(&self) -> impl Iterator<Item = &str> {
        self.titles
            .iter()
            .map(|t| t.trim())
            .filter(|t| !t.is_empty())
    }

    /// Whether the rule says what it matches at all.
    pub fn has_conditions(&self) -> bool {
        self.processes.iter().any(|p| !p.trim().is_empty())
            || self.titles().next().is_some()
            || self.fullscreen
            || self.steam_games
    }

    /// Matches on process names only: the kind the tray's "Disable while …" adds and removes.
    fn is_plain(&self) -> bool {
        self.titles().next().is_none() && !self.fullscreen && !self.steam_games
    }

    pub fn disables(&self) -> bool {
        self.temperature.is_none() && self.brightness.is_none()
    }

    /// Ready-made rules the settings window offers as one tick each.
    pub fn presets() -> [(&'static str, Rule); 3] {
        [
            (
                "Turn off for any fullscreen app",
                Rule {
                    name: Some("Fullscreen apps".into()),
                    fullscreen: true,
                    ..Rule::default()
                },
            ),
            (
                "Turn off for every Steam game",
                Rule {
                    name: Some("Steam games".into()),
                    steam_games: true,
                    ..Rule::default()
                },
            ),
            (
                "Milder tint in video players",
                Rule {
                    name: Some("Video players".into()),
                    processes: VIDEO_PLAYERS.iter().map(|p| p.to_string()).collect(),
                    temperature: Some(4800),
                    ..Rule::default()
                },
            ),
        ]
    }
}

/// Process names (and Wayland app_ids) of common video players, for the video player preset.
const VIDEO_PLAYERS: &[&str] = &[
    "vlc",
    "mpv",
    "mpc-hc64",
    "mpc-hc",
    "mpc-be64",
    "mpc-be",
    "potplayermini64",
    "potplayermini",
    "wmplayer",
    "video.ui",
    "celluloid",
    "io.github.celluloid_player.celluloid",
    "totem",
    "org.gnome.totem",
    "haruna",
    "org.kde.haruna",
    "smplayer",
];

impl Default for General {
    fn default() -> Self {
        General {
            poll_interval_secs: 2,
            fade_secs: 0.5,
            preserve_calibration: true,
            toggle_hotkey: "Ctrl+Alt+End".into(),
            jump_fade_minutes: 7.0,
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
            bedtime_temperature: 2700,
            bedtime_brightness: 1.0,
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
            days: Days::default(),
            sunset_offset_minutes: 0,
            sunrise_offset_minutes: 0,
            bedtime: String::new(),
        }
    }
}

/// Limits for `schedule.sunset_offset_minutes`/`sunrise_offset_minutes`.
pub const MAX_SOLAR_OFFSET: i32 = 180;
pub const MAX_JUMP_FADE_MINUTES: f64 = 30.0;

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
        for (i, name) in Days::NAMES.iter().enumerate() {
            if let Some(d) = s.days.get(i) {
                parse_hhmm(&d.night_start).with_context(|| format!("schedule.days.{name}"))?;
                parse_hhmm(&d.night_end).with_context(|| format!("schedule.days.{name}"))?;
            }
        }
        for (key, m) in [
            ("sunset_offset_minutes", s.sunset_offset_minutes),
            ("sunrise_offset_minutes", s.sunrise_offset_minutes),
        ] {
            if !(-MAX_SOLAR_OFFSET..=MAX_SOLAR_OFFSET).contains(&m) {
                bail!("schedule.{key} must be between -{MAX_SOLAR_OFFSET} and {MAX_SOLAR_OFFSET}");
            }
        }
        if !s.bedtime.trim().is_empty() {
            parse_hhmm(&s.bedtime).context("schedule.bedtime")?;
        }
        if !(0.0..=MAX_JUMP_FADE_MINUTES).contains(&self.general.jump_fade_minutes) {
            bail!("general.jump_fade_minutes must be between 0 and {MAX_JUMP_FADE_MINUTES}");
        }
        for (i, r) in self.rules.iter().enumerate() {
            if !r.has_conditions() {
                bail!("rules[{i}] needs processes, titles, fullscreen or steam_games");
            }
            if r.when == When::Running && (r.titles().next().is_some() || r.fullscreen) {
                bail!("rules[{i}]: titles and fullscreen only work with when = \"focused\"");
            }
        }
        for (i, d) in self.displays.iter().enumerate() {
            if d.id.trim().is_empty() {
                bail!("displays[{i}] has no id");
            }
            if self.displays[..i].iter().any(|e| e.is(&d.id)) {
                bail!("displays[{i}]: display {:?} is listed twice", d.id);
            }
        }
        Ok(())
    }
}

impl Config {
    /// Is there a rule that fully disables Ember while `app` (normalised name) is focused or running?
    pub fn has_disable_rule(&self, app: &str) -> bool {
        self.rules.iter().any(|r| {
            r.disables()
                && r.is_plain()
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
        // Strip `app` from every plain disable-rule; drop rules left with no processes. Rules that
        // also look at titles, fullscreen or Steam are the user's own and stay as they are.
        let is_disable = |t: &Table| {
            [
                "temperature",
                "brightness",
                "titles",
                "fullscreen",
                "steam_games",
            ]
            .iter()
            .all(|k| !t.contains_key(k))
        };
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
            "preserve_calibration",
            o.preserve_calibration,
            n.preserve_calibration,
            |x: bool| x
        );
        sync!(
            t,
            "toggle_hotkey",
            o.toggle_hotkey.as_str(),
            n.toggle_hotkey.as_str(),
            |x: &str| x.to_string()
        );
        sync!(
            t,
            "jump_fade_minutes",
            o.jump_fade_minutes,
            n.jump_fade_minutes,
            |x: f64| round_to(x, 1)
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
        sync!(
            t,
            "bedtime_temperature",
            o.bedtime_temperature,
            n.bedtime_temperature,
            int
        );
        sync!(
            t,
            "bedtime_brightness",
            o.bedtime_brightness,
            n.bedtime_brightness,
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
        sync!(
            t,
            "sunset_offset_minutes",
            o.sunset_offset_minutes,
            n.sunset_offset_minutes,
            |x: i32| x as i64
        );
        sync!(
            t,
            "sunrise_offset_minutes",
            o.sunrise_offset_minutes,
            n.sunrise_offset_minutes,
            |x: i32| x as i64
        );
        sync!(
            t,
            "bedtime",
            o.bedtime.as_str(),
            n.bedtime.as_str(),
            |x: &str| x.to_string()
        );
        if o.days != n.days {
            if !t.contains_key("days") {
                let mut days = Table::new();
                days.set_implicit(false);
                t.insert("days", Item::Table(days));
            }
            let days = t["days"]
                .as_table_like_mut()
                .context("schedule.days must be a table")?;
            for (i, name) in Days::NAMES.iter().enumerate() {
                if o.days.get(i) == n.days.get(i) {
                    continue;
                }
                match n.days.get(i) {
                    Some(d) => {
                        let mut v = toml_edit::InlineTable::new();
                        v.insert("night_start", d.night_start.as_str().into());
                        v.insert("night_end", d.night_end.as_str().into());
                        days.insert(name, Item::Value(Value::InlineTable(v)));
                    }
                    None => {
                        days.remove(name);
                    }
                }
            }
            if days.is_empty() {
                t.remove("days");
            }
        }
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
                let procs = r.processes.iter().map(String::as_str).collect::<Array>();
                set_opt(
                    t,
                    "processes",
                    (!r.processes.is_empty()).then(|| procs.into()),
                );
            }
            if prev.map(|p| &p.titles) != Some(&r.titles) {
                let titles = r.titles.iter().map(String::as_str).collect::<Array>();
                set_opt(t, "titles", (!r.titles.is_empty()).then(|| titles.into()));
            }
            if prev.map(|p| p.fullscreen) != Some(r.fullscreen) {
                set_opt(t, "fullscreen", r.fullscreen.then(|| true.into()));
            }
            if prev.map(|p| p.steam_games) != Some(r.steam_games) {
                set_opt(t, "steam_games", r.steam_games.then(|| true.into()));
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

    if old.displays != new.displays {
        if !doc.contains_key("displays") {
            doc.insert("displays", Item::ArrayOfTables(ArrayOfTables::new()));
        }
        let arr = doc["displays"]
            .as_array_of_tables_mut()
            .context("`displays` must be written as [[displays]] tables")?;
        while arr.len() > new.displays.len() {
            arr.remove(arr.len() - 1);
        }
        for (i, d) in new.displays.iter().enumerate() {
            if i >= arr.len() {
                arr.push(Table::new());
            }
            let prev = old.displays.get(i);
            if prev == Some(d) {
                continue;
            }
            let t = arr.get_mut(i).expect("index checked above");
            if prev.map(|p| &p.id) != Some(&d.id) {
                set(t, "id", d.id.as_str());
            }
            if prev.map(|p| p.tint) != Some(d.tint) {
                set(t, "tint", d.tint);
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
    fn new_rule_kinds_validate() {
        assert!(Config::parse("[[rules]]\nfullscreen = true").is_ok());
        assert!(Config::parse("[[rules]]\nsteam_games = true\nwhen = \"running\"").is_ok());
        assert!(Config::parse("[[rules]]\ntitles = [\"YouTube\"]").is_ok());
        assert!(Config::parse("[[rules]]\nname = \"nothing\"").is_err());
        assert!(Config::parse("[[rules]]\ntitles = [\" \"]").is_err());
        assert!(Config::parse("[[rules]]\nfullscreen = true\nwhen = \"running\"").is_err());
    }

    #[test]
    fn schedule_extras_validate() {
        assert!(Config::parse("[schedule]\nbedtime = \"23:00\"").is_ok());
        assert!(Config::parse("[schedule]\nbedtime = \"late\"").is_err());
        assert!(Config::parse("[schedule]\nsunset_offset_minutes = 400").is_err());
        assert!(
            Config::parse(
                "[schedule.days]\nsat = { night_start = \"00:30\", night_end = \"10:00\" }"
            )
            .is_ok()
        );
        assert!(Config::parse("[schedule.days]\nsat = { night_start = \"00:30\" }").is_err());
        assert!(Config::parse("[general]\njump_fade_minutes = -1").is_err());
    }

    #[test]
    fn tray_app_rule_leaves_title_rules_alone() {
        let text = "[[rules]]\nprocesses = [\"firefox\"]\ntitles = [\"YouTube\"]\n";
        let cfg = Config::parse(text).unwrap();
        assert!(!cfg.has_disable_rule("firefox"));
        let out = edit_text_set_app_rule(text, "firefox", false).unwrap();
        assert_eq!(Config::parse(&out).unwrap(), cfg);
    }

    #[test]
    fn presets_are_valid_rules() {
        let cfg = Config {
            rules: Rule::presets().into_iter().map(|(_, r)| r).collect(),
            ..Config::default()
        };
        cfg.validate().unwrap();
        let out = edit_text_apply("", &cfg).unwrap();
        assert_eq!(Config::parse(&out).unwrap(), cfg);
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
            brightness: Some(0.9),
            ..Rule::default()
        });
        cfg.rules.push(Rule {
            processes: vec!["firefox".into()],
            titles: vec!["YouTube".into()],
            ..Rule::default()
        });
        cfg.rules.push(Rule {
            fullscreen: true,
            ..Rule::default()
        });
        cfg.rules.push(Rule {
            steam_games: true,
            when: When::Running,
            ..Rule::default()
        });
        cfg.general.jump_fade_minutes = 10.0;
        cfg.color.bedtime_temperature = 2200;
        cfg.schedule.bedtime = "23:30".into();
        cfg.schedule.sunset_offset_minutes = 30;
        cfg.schedule.sunrise_offset_minutes = -15;
        cfg.schedule.days.fri = Some(DayTimes {
            night_start: "23:30".into(),
            night_end: "09:00".into(),
        });
        let out = edit_text_apply(DEFAULT_CONFIG, &cfg).unwrap();
        assert_eq!(Config::parse(&out).unwrap(), cfg);
        assert!(out.contains("fri = { night_start = \"23:30\", night_end = \"09:00\" }"));
        // Taking a day out again drops the then empty table.
        let mut fewer = cfg.clone();
        fewer.schedule.days.fri = None;
        let out = edit_text_apply(&out, &fewer).unwrap();
        assert_eq!(Config::parse(&out).unwrap(), fewer);
        assert!(!out.lines().any(|l| l.trim() == "[schedule.days]"));
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
    fn preserve_calibration_defaults_on_and_saves() {
        assert!(Config::default().general.preserve_calibration);
        assert!(
            Config::parse("[general]\nfade_secs = 1.0")
                .unwrap()
                .general
                .preserve_calibration
        );
        let mut cfg = Config::parse(DEFAULT_CONFIG).unwrap();
        cfg.general.preserve_calibration = false;
        let out = edit_text_apply(DEFAULT_CONFIG, &cfg).unwrap();
        assert_eq!(Config::parse(&out).unwrap(), cfg);
        assert!(out.contains("preserve_calibration = false"));
        // Also when the key isn't in the file yet.
        let out = edit_text_apply("[general]\nfade_secs = 0.5\n", &cfg).unwrap();
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

    fn untinted(id: &str) -> Display {
        Display {
            tint: false,
            ..Display::new(id)
        }
    }

    #[test]
    fn displays_parse_with_defaults() {
        assert!(Config::default().displays.is_empty());
        let cfg = Config::parse(
            r#"
[[displays]]
id = '\\.\DISPLAY2'
tint = false

[[displays]]
id = "HDMI-1"
"#,
        )
        .unwrap();
        assert_eq!(
            cfg.displays,
            vec![untinted(r"\\.\DISPLAY2"), Display::new("HDMI-1")]
        );
    }

    #[test]
    fn bad_display_entries_are_rejected() {
        assert!(Config::parse("[[displays]]\nid = \" \"\ntint = false").is_err());
        assert!(Config::parse("[[displays]]\ntint = false").is_err());
        assert!(Config::parse("[[displays]]\nid = \"DP-1\"\ntin = false").is_err());
        let twice = "[[displays]]\nid = \"DP-1\"\n[[displays]]\nid = \"dp-1\"\ntint = false";
        let e = Config::parse(twice).unwrap_err();
        assert!(format!("{e:#}").contains("twice"), "{e:#}");
    }

    #[test]
    fn which_displays_are_tinted() {
        let displays = vec![
            untinted(r"\\.\DISPLAY2"),
            Display::new("DP-1"),
            untinted("HDMI-1"),
        ];
        assert!(tints_display(&displays, r"\\.\DISPLAY1"));
        assert!(!tints_display(&displays, r"\\.\DISPLAY2"));
        assert!(
            !tints_display(&displays, r"\\.\display2"),
            "case-insensitive"
        );
        assert!(tints_display(&displays, "DP-1"), "listed with tint = true");
        assert!(!tints_display(&displays, "HDMI-1"));
        assert!(tints_display(&displays, "HDMI-10"));
        // Mirrored X11 outputs share one ramp.
        assert!(!tints_display(&displays, "DP-1+HDMI-1"));
        assert!(tints_display(&displays, "DP-1+DP-2"));
        assert!(tints_display(&[], "anything"));
    }

    #[test]
    fn set_display_tint_adds_and_drops_entries() {
        let mut displays = Vec::new();
        set_display_tint(&mut displays, "HDMI-1", false);
        assert_eq!(displays, vec![untinted("HDMI-1")]);
        // Ticking it again leaves nothing that differs from the defaults.
        set_display_tint(&mut displays, "hdmi-1", true);
        assert!(displays.is_empty());
        set_display_tint(&mut displays, "DP-1", true);
        assert!(displays.is_empty());
    }

    #[test]
    fn apply_writes_displays_and_keeps_comments() {
        let mut cfg = Config::parse(DEFAULT_CONFIG).unwrap();
        set_display_tint(&mut cfg.displays, r"\\.\DISPLAY2", false);
        set_display_tint(&mut cfg.displays, "HDMI-1", false);
        let out = edit_text_apply(DEFAULT_CONFIG, &cfg).unwrap();
        assert_eq!(Config::parse(&out).unwrap(), cfg);
        assert!(out.contains("# Ember configuration."));
        assert!(out.contains("night_temperature = 3400 # lower = warmer/redder"));

        cfg.displays[0].tint = true;
        let out2 = edit_text_apply(&out, &cfg).unwrap();
        assert_eq!(Config::parse(&out2).unwrap(), cfg);

        cfg.displays.clear();
        let out3 = edit_text_apply(&out2, &cfg).unwrap();
        assert_eq!(Config::parse(&out3).unwrap(), cfg);
        assert!(!out3.contains("\n[[displays]]"));
    }
}
