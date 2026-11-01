//! The settings window (`ember --settings`). Runs as its own process, edits config.toml,
//! and previews slider values on screen while you drag them.

use anyhow::{Result, anyhow};
use eframe::egui::{self, Color32, CornerRadius, RichText, SliderClamping, Stroke, Ui};
use ember::color::{self, ColorSetting};
use ember::config::{self, Config, DayTimes, Rule, ScheduleMode, When};
use ember::hotkey::Hotkey;
use ember::tray_model::Anchor;
use ember::{engine, gamma, schedule, tray_model};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

const DAY_NAMES: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

/// A sunset/sunrise offset as the slider shows it.
fn offset_text(minutes: f64) -> String {
    match minutes as i64 {
        0 => "as the sun says".into(),
        m if m > 0 => format!("{m} min later"),
        m => format!("{} min earlier", -m),
    }
}

/// Wait this long after the last edit before writing the file.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(400);
const PREVIEW_REFRESH: Duration = Duration::from_millis(250);

/// Space between the window and the taskbar / screen edges, in points.
const SCREEN_GAP: f32 = 12.0;

pub fn run(config_path: PathBuf, anchor: Option<Anchor>) -> Result<()> {
    // Only one settings window at a time.
    let lock_path = config::state_dir().join("settings.lock");
    std::fs::create_dir_all(config::state_dir())?;
    let lock = File::create(&lock_path)?;
    if lock.try_lock().is_err() {
        log::info!("settings window already open; bringing it to the front");
        return request_show(anchor);
    }
    let _ = std::fs::remove_file(show_request_path()); // left over from a crash

    let icon = egui::IconData {
        rgba: tray_model::icon_rgba(64, true),
        width: 64,
        height: 64,
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Ember settings")
            // Wayland app_id / X11 class; "ember" is skipped as a rule suggestion like the tray.
            .with_app_id("ember")
            .with_inner_size([520.0, 680.0])
            .with_min_inner_size([420.0, 400.0])
            .with_icon(icon),
        ..Default::default()
    };
    let app = SettingsApp::new(config_path, anchor);
    // winit panics (rather than erroring) when e.g. libxkbcommon-x11 is missing; report it cleanly.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        eframe::run_native(
            "Ember settings",
            options,
            Box::new(|cc| {
                apply_theme(&cc.egui_ctx);
                watch_show_requests(cc.egui_ctx.clone());
                Ok(Box::new(app))
            }),
        )
    }));
    match result {
        Ok(r) => r.map_err(|e| anyhow!("could not open the settings window: {e}"))?,
        Err(p) => {
            let msg = p
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| p.downcast_ref::<&str>().copied())
                .unwrap_or("unknown error");
            engine::clear_preview();
            return Err(anyhow!("could not open the settings window: {msg}"));
        }
    }
    drop(lock);
    Ok(())
}

struct SettingsApp {
    path: PathBuf,
    cfg: Config,
    /// What is on disk (as far as we know).
    saved: Config,
    file_mtime: Option<SystemTime>,
    last_edit: Option<Instant>,
    message: Option<(String, bool)>, // (text, is_error)
    /// Comma-separated process lists, one per rule (kept separately so typing "a, " works).
    proc_text: Vec<String>,
    /// Each rule's window titles, as typed (comma-separated).
    title_text: Vec<String>,
    backend: Option<Box<dyn gamma::Backend>>,
    /// Dragging a slider shows on screen, through `backend` or the running tray app.
    preview_works: bool,
    /// Connected displays as (config id, label), from the backend.
    displays: Vec<(String, String)>,
    /// False when the desktop tints every screen alike (KDE Night Light).
    per_display: bool,
    previewing: Option<ColorSetting>,
    last_preview_write: Option<Instant>,
    running_apps: Vec<String>,
    new_app: String,
    /// The tray icon this window was opened from; it's moved next to it on the first frame.
    anchor: Option<Anchor>,
    first_frame: bool,
    logo: Option<egui::TextureHandle>,
    /// Waiting for the user to press a new toggle hotkey.
    recording_hotkey: bool,
    /// Why the last key combination pressed can't be used.
    hotkey_error: Option<String>,
}

/// Offline shortcut for setting a location. Sorted by name.
const CITIES: &[(&str, f64, f64)] = &[
    ("Amsterdam", 52.37, 4.90),
    ("Athens", 37.98, 23.73),
    ("Auckland", -36.85, 174.76),
    ("Bangkok", 13.76, 100.50),
    ("Beijing", 39.90, 116.40),
    ("Berlin", 52.52, 13.40),
    ("Buenos Aires", -34.60, -58.38),
    ("Cairo", 30.04, 31.24),
    ("Chicago", 41.88, -87.63),
    ("Copenhagen", 55.68, 12.57),
    ("Delhi", 28.61, 77.21),
    ("Denver", 39.74, -104.99),
    ("Dubai", 25.20, 55.27),
    ("Helsinki", 60.17, 24.94),
    ("Istanbul", 41.01, 28.98),
    ("Jakarta", -6.21, 106.85),
    ("Johannesburg", -26.20, 28.05),
    ("Kyiv", 50.45, 30.52),
    ("Lagos", 6.52, 3.38),
    ("London", 51.51, -0.13),
    ("Los Angeles", 34.05, -118.24),
    ("Madrid", 40.42, -3.70),
    ("Mexico City", 19.43, -99.13),
    ("Moscow", 55.76, 37.62),
    ("Mumbai", 19.08, 72.88),
    ("New York", 40.71, -74.01),
    ("Oslo", 59.91, 10.75),
    ("Paris", 48.86, 2.35),
    ("Reykjavik", 64.15, -21.94),
    ("Rome", 41.90, 12.50),
    ("San Francisco", 37.77, -122.42),
    ("São Paulo", -23.55, -46.63),
    ("Seattle", 47.61, -122.33),
    ("Seoul", 37.57, 126.98),
    ("Singapore", 1.35, 103.82),
    ("Stockholm", 59.33, 18.07),
    ("Sydney", -33.87, 151.21),
    ("Tokyo", 35.68, 139.69),
    ("Toronto", 43.65, -79.38),
    ("Vancouver", 49.28, -123.12),
    ("Warsaw", 52.23, 21.01),
];

/// Written by a second `ember --settings` (the tray icon clicked again) to ask the open window
/// to come to the front, next to the icon in the anchor it contains (if any).
fn show_request_path() -> PathBuf {
    config::state_dir().join("settings.show")
}

fn request_show(anchor: Option<Anchor>) -> Result<()> {
    // We were started by a click, so Windows lets us hand our right to take the foreground to
    // the open window; without this it would only flash in the taskbar.
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(
            windows_sys::Win32::UI::WindowsAndMessaging::ASFW_ANY,
        );
    }
    let text = anchor.map(|a| a.to_string()).unwrap_or_default();
    std::fs::write(show_request_path(), text)?;
    Ok(())
}

/// Takes a pending show request: `Some(anchor)` if there is one.
fn take_show_request() -> Option<Option<Anchor>> {
    let path = show_request_path();
    let text = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    Some(text.trim().parse().ok())
}

/// The window only repaints every second when idle, so wake it as soon as a request arrives.
fn watch_show_requests(ctx: egui::Context) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(150));
            if show_request_path().exists() {
                ctx.request_repaint();
            }
        }
    });
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Where a window of `size` should go so it sits next to the tray `icon`: just above a bottom
/// taskbar, below a top one, beside a side one, and always inside `work` (the screen minus the
/// taskbar).
fn place_near(icon: egui::Rect, work: egui::Rect, size: egui::Vec2) -> egui::Pos2 {
    let g = SCREEN_GAP;
    let pos = if icon.min.y >= work.max.y {
        egui::pos2(icon.center().x - size.x / 2.0, work.max.y - size.y - g)
    } else if icon.max.y <= work.min.y {
        egui::pos2(icon.center().x - size.x / 2.0, work.min.y + g)
    } else if icon.min.x >= work.max.x {
        egui::pos2(work.max.x - size.x - g, icon.center().y - size.y / 2.0)
    } else if icon.max.x <= work.min.x {
        egui::pos2(work.min.x + g, icon.center().y - size.y / 2.0)
    } else {
        // The icon is inside the work area, e.g. in the taskbar's "^" overflow flyout.
        egui::pos2(icon.center().x - size.x / 2.0, icon.min.y - size.y - g)
    };
    let max = (work.max - size - egui::vec2(g, g)).max(work.min);
    pos.clamp(work.min + egui::vec2(g, g), max)
}

/// The usable area (screen minus taskbar) of the monitor containing `a`, in physical pixels.
#[cfg(windows)]
fn work_area(a: Anchor) -> Option<egui::Rect> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
    };
    let centre = POINT {
        x: a.x + a.width / 2,
        y: a.y + a.height / 2,
    };
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    let ok = unsafe {
        GetMonitorInfoW(
            MonitorFromPoint(centre, MONITOR_DEFAULTTONEAREST),
            &mut info,
        )
    };
    let r = info.rcWork;
    (ok != 0).then(|| {
        egui::Rect::from_min_max(
            egui::pos2(r.left as f32, r.top as f32),
            egui::pos2(r.right as f32, r.bottom as f32),
        )
    })
}

#[cfg(not(windows))]
fn work_area(_: Anchor) -> Option<egui::Rect> {
    None
}

/// Move (and if needed shrink) the window so it opens next to the tray icon it was opened from.
fn move_next_to(ctx: &egui::Context, anchor: Anchor) {
    let (outer, inner) = ctx.input(|i| (i.viewport().outer_rect, i.viewport().inner_rect));
    let (Some(outer), Some(work)) = (outer, work_area(anchor)) else {
        return;
    };
    // Physical pixels -> points. egui turns the position back into pixels with the same scale.
    let ppp = ctx.pixels_per_point();
    let work = egui::Rect::from_min_max(work.min / ppp, work.max / ppp);
    let icon = egui::Rect::from_min_size(
        egui::pos2(anchor.x as f32, anchor.y as f32) / ppp,
        egui::vec2(anchor.width as f32, anchor.height as f32) / ppp,
    );
    let mut size = outer.size();
    let too_tall = size.y - (work.height() - 2.0 * SCREEN_GAP);
    if too_tall > 0.0
        && let Some(inner) = inner
    {
        size.y -= too_tall;
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
            inner.width(),
            inner.height() - too_tall,
        )));
    }
    ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(place_near(
        icon, work, size,
    )));
}

/// Night sky and ember colours, matching the logo and the README.
mod palette {
    use eframe::egui::Color32;

    pub const BACKGROUND: Color32 = Color32::from_rgb(22, 25, 31);
    pub const CARD: Color32 = Color32::from_rgb(30, 34, 42);
    pub const CARD_RAISED: Color32 = Color32::from_rgb(38, 43, 53);
    pub const FIELD: Color32 = Color32::from_rgb(16, 18, 23);
    pub const BORDER: Color32 = Color32::from_rgb(48, 54, 66);
    pub const CONTROL: Color32 = Color32::from_rgb(47, 52, 64);
    pub const CONTROL_HOVER: Color32 = Color32::from_rgb(60, 65, 79);
    pub const TEXT: Color32 = Color32::from_rgb(232, 234, 238);
    pub const TEXT_WEAK: Color32 = Color32::from_rgb(146, 153, 166);
    /// The logo's orange, for accents.
    pub const EMBER: Color32 = Color32::from_rgb(240, 140, 40);
    /// The logo's hot yellow core, for highlights and warnings.
    pub const EMBER_HOT: Color32 = Color32::from_rgb(255, 196, 96);
    /// A deeper orange that light text stays readable on, for selections and slider fills.
    pub const EMBER_DEEP: Color32 = Color32::from_rgb(196, 84, 28);
    pub const ERROR: Color32 = Color32::from_rgb(240, 96, 84);
}

fn apply_theme(ctx: &egui::Context) {
    use palette::*;
    let widget =
        |bg: Color32, stroke: Stroke, fg: Color32, radius: u8| egui::style::WidgetVisuals {
            bg_fill: bg,
            weak_bg_fill: bg,
            bg_stroke: stroke,
            corner_radius: CornerRadius::same(radius),
            fg_stroke: Stroke::new(1.0, fg),
            expansion: 0.0,
        };
    let mut v = egui::Visuals::dark();
    v.panel_fill = BACKGROUND;
    v.window_fill = CARD;
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.faint_bg_color = CARD_RAISED;
    v.extreme_bg_color = FIELD;
    v.text_edit_bg_color = Some(FIELD);
    v.weak_text_color = Some(TEXT_WEAK);
    v.hyperlink_color = EMBER_HOT;
    v.warn_fg_color = EMBER_HOT;
    v.error_fg_color = ERROR;
    v.selection.bg_fill = EMBER_DEEP;
    v.selection.stroke = Stroke::new(1.0, EMBER_HOT);
    v.slider_trailing_fill = true;
    v.widgets.noninteractive = widget(CARD, Stroke::new(1.0, BORDER), TEXT, 8);
    v.widgets.inactive = widget(CONTROL, Stroke::NONE, TEXT, 6);
    v.widgets.hovered = widget(CONTROL_HOVER, Stroke::new(1.0, EMBER), Color32::WHITE, 6);
    v.widgets.active = widget(EMBER_DEEP, Stroke::new(1.0, EMBER_HOT), Color32::WHITE, 6);
    v.widgets.open = widget(CONTROL, Stroke::new(1.0, EMBER), TEXT, 6);
    ctx.set_visuals_of(egui::Theme::Dark, v);
    ctx.style_mut_of(egui::Theme::Dark, |s| {
        s.spacing.item_spacing = egui::vec2(8.0, 7.0);
        s.spacing.button_padding = egui::vec2(10.0, 4.0);
        s.spacing.interact_size.y = 24.0;
    });
    // Same look whatever the system theme is.
    ctx.set_theme(egui::Theme::Dark);
}

impl SettingsApp {
    fn new(path: PathBuf, anchor: Option<Anchor>) -> Self {
        let (cfg, message) = match path.exists().then(|| Config::load(&path)) {
            None => (
                Config::parse(config::DEFAULT_CONFIG).unwrap_or_default(),
                None,
            ),
            Some(Ok(c)) => (c, None),
            Some(Err(e)) => (
                Config::default(),
                Some((
                    format!(
                        "Your config file has an error, so changes won't be saved until it's fixed: {e:#}"
                    ),
                    true,
                )),
            ),
        };
        // Same calibration handling as the tray app, so previewing doesn't wipe the curve.
        let mut displays = Vec::new();
        let mut per_display = true;
        let mut desktop_picks_colour = false;
        let backend = match gamma::open(false, cfg.general.preserve_calibration, &cfg.displays) {
            Ok(mut b) if b.live_preview() => {
                displays = b.displays();
                Some(b)
            }
            Ok(b) => {
                log::info!("live preview unavailable with the {} backend", b.name());
                // The desktop picks the colours itself (KDE Night Light), for every screen.
                desktop_picks_colour = true;
                per_display = false;
                None
            }
            Err(e) => {
                log::info!("no gamma control of our own: {e:#}");
                None
            }
        };
        // Without gamma control of its own (on wlroots only one program can have it, and the
        // tray app does), the running Ember still shows the preview: it applies what we send
        // with `engine::set_preview`.
        let preview_works =
            backend.is_some() || (!desktop_picks_colour && ember::daemon::is_running());
        let mut me = SettingsApp {
            file_mtime: mtime(&path),
            path,
            saved: cfg.clone(),
            proc_text: Vec::new(),
            title_text: Vec::new(),
            cfg,
            last_edit: None,
            message,
            backend,
            preview_works,
            displays,
            per_display,
            previewing: None,
            last_preview_write: None,
            running_apps: Vec::new(),
            new_app: String::new(),
            anchor,
            first_frame: true,
            logo: None,
            recording_hotkey: false,
            hotkey_error: None,
        };
        me.sync_proc_text();
        me.refresh_running_apps();
        me
    }

    fn sync_proc_text(&mut self) {
        self.proc_text = self
            .cfg
            .rules
            .iter()
            .map(|r| r.processes.join(", "))
            .collect();
        self.title_text = self.cfg.rules.iter().map(|r| r.titles.join(", ")).collect();
    }

    fn refresh_running_apps(&mut self) {
        if let Ok(mut w) = engine::ProcessWatcher::new() {
            self.running_apps = w
                .all_names()
                .into_iter()
                .filter(|n| {
                    n.len() > 1 && !n.contains([':', '/', ' ']) && !n.starts_with("kworker")
                })
                .collect();
        }
    }

    /// Pick up edits made elsewhere (e.g. "Disable while … is focused" in the tray).
    fn reload_if_changed_on_disk(&mut self) {
        let m = mtime(&self.path);
        if m == self.file_mtime || self.last_edit.is_some() {
            return;
        }
        self.file_mtime = m;
        if let Ok(c) = Config::load(&self.path) {
            if let Some(b) = self.backend.as_mut() {
                b.set_preserve_calibration(c.general.preserve_calibration);
                b.set_displays(&c.displays);
            }
            self.cfg = c.clone();
            self.saved = c;
            self.sync_proc_text();
        }
    }

    fn save_if_due(&mut self) {
        let Some(t) = self.last_edit else { return };
        if t.elapsed() < SAVE_DEBOUNCE || self.previewing.is_some() {
            return;
        }
        self.last_edit = None;
        // A rule that's still being filled in (nothing to match yet) shouldn't block saving
        // everything else; it's kept in the window and saved once it has a condition.
        let mut to_save = self.cfg.clone();
        to_save.rules.retain(Rule::has_conditions);
        if to_save == self.saved {
            return;
        }
        if let Err(e) = to_save.validate() {
            self.message = Some((format!("Not saved: {e:#}"), true));
            return;
        }
        match config::save(&self.path, &to_save) {
            Ok(()) => {
                self.saved = to_save;
                self.file_mtime = mtime(&self.path);
                self.message = Some(("Saved".into(), false));
            }
            Err(e) => self.message = Some((format!("Not saved: {e:#}"), true)),
        }
    }

    /// Show `s` on screen right now, and tell a running Ember daemon to hold it.
    fn preview(&mut self, s: ColorSetting) {
        let due = self
            .last_preview_write
            .is_none_or(|t| t.elapsed() >= PREVIEW_REFRESH);
        if self.previewing != Some(s) || due {
            let _ = engine::set_preview(s);
            self.last_preview_write = Some(Instant::now());
        }
        if self.previewing != Some(s)
            && let Some(b) = self.backend.as_mut()
        {
            let _ = b.apply(s);
        }
        self.previewing = Some(s);
    }

    fn end_preview(&mut self) {
        if self.previewing.take().is_none() {
            return;
        }
        engine::clear_preview();
        // Snap back to what the schedule says; a running daemon will take over from here.
        let target = if engine::paused().is_some() {
            ColorSetting::NEUTRAL
        } else {
            schedule::scheduled_setting(&self.cfg, chrono::Local::now())
        };
        if let Some(b) = self.backend.as_mut() {
            let _ = b.apply(target);
            let _ = b.sync();
        }
    }
}

/// A small swatch showing how white looks at this temperature/brightness.
fn swatch(ui: &mut Ui, s: ColorSetting) {
    let [r, g, b] = color::temperature_to_rgb(s.temperature);
    let k = s.brightness.clamp(0.1, 1.0) as f64 * 255.0;
    let c = Color32::from_rgb((r * k) as u8, (g * k) as u8, (b * k) as u8);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(28.0, 18.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 4.0, c);
}

/// Temperature slider. Returns the new value if the user is dragging (for live preview).
fn kelvin_slider(
    ui: &mut Ui,
    value: &mut u32,
    range: std::ops::RangeInclusive<u32>,
) -> (bool, bool) {
    let r = ui.add(
        egui::Slider::new(value, range)
            .step_by(50.0)
            .suffix(" K")
            .clamping(SliderClamping::Edits),
    );
    (r.changed(), r.dragged() || r.has_focus())
}

fn percent_slider(ui: &mut Ui, value: &mut f32) -> (bool, bool) {
    let mut pct = (*value * 100.0).round() as u32;
    let r = ui.add(egui::Slider::new(&mut pct, 10..=100).suffix(" %"));
    if r.changed() {
        *value = pct as f32 / 100.0;
    }
    (r.changed(), r.dragged() || r.has_focus())
}

fn time_edit(ui: &mut Ui, id: &str, hhmm: &mut String) -> bool {
    let t = config::parse_hhmm(hhmm).unwrap_or_default();
    use chrono::Timelike;
    let (mut h, mut m) = (t.hour(), t.minute());
    let mut changed = false;
    ui.push_id(id, |ui| {
        changed |= ui
            .add(egui::DragValue::new(&mut h).range(0..=23).speed(0.1))
            .changed();
        ui.label(":");
        changed |= ui
            .add(
                egui::DragValue::new(&mut m)
                    .range(0..=59)
                    .speed(0.2)
                    .custom_formatter(|v, _| format!("{v:02}")),
            )
            .changed();
    });
    if changed {
        *hhmm = format!("{h:02}:{m:02}");
    }
    changed
}

/// The toggle hotkey: a button that, once clicked, records the next key combination pressed.
/// Escape cancels; Backspace or Delete on its own turns the hotkey off.
fn hotkey_button(
    ui: &mut Ui,
    value: &mut String,
    recording: &mut bool,
    error: &mut Option<String>,
) {
    if *recording {
        // Take the key events before any widget sees them, so e.g. Space doesn't click the button.
        let pressed = ui.input_mut(|i| {
            let found = i.events.iter().find_map(|e| match e {
                egui::Event::Key {
                    key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } => Some((*key, *modifiers)),
                _ => None,
            });
            i.events
                .retain(|e| !matches!(e, egui::Event::Key { .. } | egui::Event::Text(_)));
            found
        });
        if let Some((key, m)) = pressed {
            *error = None;
            let plain = !(m.ctrl || m.alt || m.shift);
            match key {
                egui::Key::Escape => *recording = false,
                egui::Key::Backspace | egui::Key::Delete if plain => {
                    value.clear();
                    *recording = false;
                }
                // Keep recording after a bad combination so the user can just try another.
                _ => match recorded_hotkey(key, m) {
                    Ok(h) => {
                        *value = h.to_string();
                        *recording = false;
                    }
                    Err(e) => *error = Some(format!("{e:#}")),
                },
            }
        }
    }
    let text = if *recording {
        RichText::new("Press a key combination…").color(palette::EMBER_HOT)
    } else if value.is_empty() {
        RichText::new("None")
    } else {
        RichText::new(value.as_str())
    };
    let hint = "Pauses or resumes Ember from any app. Click, then press the keys. \
                Esc cancels, Backspace turns the hotkey off. \
                To use the Win (Super) key, edit toggle_hotkey in the config file.";
    if ui.button(text).on_hover_text(hint).clicked() {
        *recording = !*recording;
        *error = None;
    }
}

/// The hotkey for `key` pressed with modifiers `m`. egui's key names match the config's.
fn recorded_hotkey(key: egui::Key, m: egui::Modifiers) -> Result<Hotkey> {
    let mut text = String::new();
    for (on, name) in [(m.ctrl, "Ctrl+"), (m.alt, "Alt+"), (m.shift, "Shift+")] {
        if on {
            text.push_str(name);
        }
    }
    text.push_str(key.name());
    text.parse()
}

/// A display in the Displays section.
#[derive(Debug, PartialEq)]
struct DisplayRow {
    id: String,
    label: String,
    connected: bool,
}

/// The connected displays, then any configured ones that aren't connected right now (so their
/// settings can still be undone).
fn display_rows(connected: &[(String, String)], configured: &[config::Display]) -> Vec<DisplayRow> {
    let mut rows: Vec<DisplayRow> = connected
        .iter()
        .map(|(id, label)| DisplayRow {
            id: id.clone(),
            label: label.clone(),
            connected: true,
        })
        .collect();
    for d in configured {
        if !rows.iter().any(|r| r.id.eq_ignore_ascii_case(d.id.trim())) {
            rows.push(DisplayRow {
                id: d.id.clone(),
                label: d.id.clone(),
                connected: false,
            });
        }
    }
    rows
}

fn section(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) {
    ui.add_space(14.0);
    ui.label(
        RichText::new(title)
            .strong()
            .size(15.0)
            .color(palette::EMBER_HOT),
    );
    ui.add_space(4.0);
    egui::Frame::new()
        .fill(palette::CARD)
        .stroke(Stroke::new(1.0, palette::BORDER))
        .corner_radius(10.0)
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        });
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, root: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        if std::mem::take(&mut self.first_frame) {
            // A dark title bar to match, even when Windows is in light mode.
            ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(egui::SystemTheme::Dark));
            if let Some(a) = self.anchor {
                move_next_to(&ctx, a);
            }
        }
        if let Some(anchor) = take_show_request() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            if let Some(a) = anchor {
                move_next_to(&ctx, a);
            }
        }
        self.reload_if_changed_on_disk();
        let before = self.cfg.clone();
        let mut preview: Option<ColorSetting> = None;
        let logo = self
            .logo
            .get_or_insert_with(|| {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [96, 96],
                    &tray_model::icon_rgba(96, true),
                );
                ctx.load_texture("logo", image, egui::TextureOptions::LINEAR)
            })
            .clone();

        let panel = egui::Frame::central_panel(&ctx.global_style())
            .inner_margin(egui::Margin::symmetric(18, 14));
        egui::CentralPanel::default().frame(panel).show(root, |ui| {
            ui.spacing_mut().slider_width = (ui.available_width() - 260.0).clamp(120.0, 300.0);
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                // ── Header ─────────────────────────────────────────────
                let now = schedule::scheduled_setting(&self.cfg, chrono::Local::now());
                ui.horizontal(|ui| {
                    ui.add(egui::Image::new(&logo).fit_to_exact_size(egui::vec2(44.0, 44.0)));
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new("Ember").strong().size(22.0).color(palette::TEXT));
                        ui.horizontal(|ui| {
                            swatch(ui, now);
                            let paused = if engine::paused().is_some() { " · paused" } else { "" };
                            ui.label(format!("Right now {} K, {:.0} %{paused}", now.temperature, now.brightness * 100.0));
                        });
                    });
                });
                ui.add_space(2.0);
                if self.preview_works {
                    ui.weak("Drag a slider to preview it on screen.");
                } else {
                    ui.weak("Live preview isn't available on this display server.");
                }

                // ── Colour ─────────────────────────────────────────────
                section(ui, "Colour", |ui| {
                    egui::Grid::new("colour").num_columns(3).spacing([12.0, 10.0]).show(ui, |ui| {
                        let c = &mut self.cfg.color;
                        ui.label("Night temperature");
                        let (_, d) = kelvin_slider(ui, &mut c.night_temperature, 1200..=6500);
                        if d {
                            preview = Some(ColorSetting { temperature: c.night_temperature, brightness: c.night_brightness });
                        }
                        swatch(ui, ColorSetting { temperature: c.night_temperature, brightness: c.night_brightness });
                        ui.end_row();

                        ui.label("Night brightness");
                        let (_, d) = percent_slider(ui, &mut c.night_brightness);
                        if d {
                            preview = Some(ColorSetting { temperature: c.night_temperature, brightness: c.night_brightness });
                        }
                        ui.label("");
                        ui.end_row();

                        ui.label("Day temperature");
                        let (_, d) = kelvin_slider(ui, &mut c.day_temperature, 3000..=6500);
                        if d {
                            preview = Some(ColorSetting { temperature: c.day_temperature, brightness: c.day_brightness });
                        }
                        swatch(ui, ColorSetting { temperature: c.day_temperature, brightness: c.day_brightness });
                        ui.end_row();

                        ui.label("Day brightness");
                        let (_, d) = percent_slider(ui, &mut c.day_brightness);
                        if d {
                            preview = Some(ColorSetting { temperature: c.day_temperature, brightness: c.day_brightness });
                        }
                        ui.label("");
                        ui.end_row();
                    });
                    ui.weak("6500 K is neutral. Lower is warmer. f.lux's default night setting is 3400 K.");
                });

                // ── Schedule ───────────────────────────────────────────
                section(ui, "Schedule", |ui| {
                    let s = &mut self.cfg.schedule;
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut s.mode, ScheduleMode::Fixed, "Fixed times");
                        ui.radio_value(&mut s.mode, ScheduleMode::Solar, "Sunset to sunrise");
                        ui.radio_value(&mut s.mode, ScheduleMode::AlwaysNight, "Always night");
                    });
                    ui.add_space(6.0);
                    match s.mode {
                        ScheduleMode::Fixed => {
                            egui::Grid::new("fixed").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                                ui.label("Night starts");
                                ui.horizontal(|ui| time_edit(ui, "start", &mut s.night_start));
                                ui.end_row();
                                ui.label("Night ends");
                                ui.horizontal(|ui| time_edit(ui, "end", &mut s.night_end));
                                ui.end_row();
                                ui.label("Fade duration");
                                ui.add(egui::Slider::new(&mut s.transition_minutes, 0..=180).suffix(" min"));
                                ui.end_row();
                            });
                            let any_days = (0..7).any(|i| s.days.get(i).is_some());
                            egui::CollapsingHeader::new("Different times on some days")
                                .default_open(any_days)
                                .show(ui, |ui| {
                                    let (start, end) = (s.night_start.clone(), s.night_end.clone());
                                    egui::Grid::new("days").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                                        for (i, day) in DAY_NAMES.iter().enumerate() {
                                            let times = s.days.get_mut(i);
                                            let mut own = times.is_some();
                                            if ui.checkbox(&mut own, format!("{day} night")).changed() {
                                                *times = own.then(|| DayTimes {
                                                    night_start: start.clone(),
                                                    night_end: end.clone(),
                                                });
                                            }
                                            ui.horizontal(|ui| match times {
                                                Some(t) => {
                                                    time_edit(ui, &format!("day-start-{i}"), &mut t.night_start);
                                                    ui.label("to");
                                                    time_edit(ui, &format!("day-end-{i}"), &mut t.night_end);
                                                }
                                                None => {
                                                    ui.weak(format!("{start} to {end}"));
                                                }
                                            });
                                            ui.end_row();
                                        }
                                    });
                                    ui.weak("A night belongs to the evening it starts on: Friday night ends on Saturday morning.");
                                });
                        }
                        ScheduleMode::Solar => {
                            if s.latitude.is_none() || s.longitude.is_none() {
                                s.latitude.get_or_insert(0.0);
                                s.longitude.get_or_insert(0.0);
                            }
                            egui::ComboBox::from_id_salt("city")
                                .selected_text("Pick a nearby city…")
                                .width(220.0)
                                .show_ui(ui, |ui| {
                                    for &(name, lat, lon) in CITIES {
                                        if ui.selectable_label(false, name).clicked() {
                                            s.latitude = Some(lat);
                                            s.longitude = Some(lon);
                                        }
                                    }
                                });
                            ui.add_space(4.0);
                            egui::Grid::new("solar").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                                ui.label("Latitude");
                                ui.add(
                                    egui::DragValue::new(s.latitude.as_mut().expect("set above"))
                                        .range(-90.0..=90.0)
                                        .speed(0.05)
                                        .fixed_decimals(2)
                                        .suffix("°"),
                                );
                                ui.end_row();
                                ui.label("Longitude");
                                ui.add(
                                    egui::DragValue::new(s.longitude.as_mut().expect("set above"))
                                        .range(-180.0..=180.0)
                                        .speed(0.05)
                                        .fixed_decimals(2)
                                        .suffix("°"),
                                );
                                ui.end_row();
                            });
                            ui.weak("Pick a city or enter coordinates (north and east are positive). Ember never uses the network.");
                            egui::Grid::new("offsets").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                                let range = -config::MAX_SOLAR_OFFSET..=config::MAX_SOLAR_OFFSET;
                                ui.label("Start the evening");
                                ui.add(egui::Slider::new(&mut s.sunset_offset_minutes, range.clone()).custom_formatter(|v, _| offset_text(v)));
                                ui.end_row();
                                ui.label("Start the morning");
                                ui.add(egui::Slider::new(&mut s.sunrise_offset_minutes, range).custom_formatter(|v, _| offset_text(v)));
                                ui.end_row();
                            });
                            let (lat, lon) = (s.latitude.unwrap_or(0.0), s.longitude.unwrap_or(0.0));
                            if lat == 0.0 && lon == 0.0 {
                                ui.colored_label(palette::EMBER_HOT, "Set your location to get correct sunset times.");
                            }
                            let t = schedule::schedule_solar_times(s, chrono::Local::now().date_naive());
                            let fmt = |p: Option<(chrono::NaiveTime, chrono::NaiveTime)>| {
                                p.map(|(a, b)| format!("{} to {}", a.format("%H:%M"), b.format("%H:%M")))
                                    .unwrap_or_else(|| "none today".into())
                            };
                            ui.label(format!("Today: evening fade {}, morning fade {}", fmt(t.evening), fmt(t.morning)));
                        }
                        ScheduleMode::AlwaysNight => {
                            ui.weak("The night settings are used all the time.");
                        }
                    }

                    ui.add_space(6.0);
                    let mut bedtime = !s.bedtime.trim().is_empty();
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut bedtime, "Bedtime: even warmer from").changed() {
                            s.bedtime = if bedtime { "23:00".into() } else { String::new() };
                        }
                        if bedtime {
                            time_edit(ui, "bedtime", &mut s.bedtime);
                        }
                    });
                    if bedtime {
                        let c = &mut self.cfg.color;
                        egui::Grid::new("bedtime").num_columns(3).spacing([12.0, 10.0]).show(ui, |ui| {
                            let bed = |c: &config::Color| ColorSetting { temperature: c.bedtime_temperature, brightness: c.bedtime_brightness };
                            ui.label("Bedtime temperature");
                            let (_, d) = kelvin_slider(ui, &mut c.bedtime_temperature, 1200..=6500);
                            if d {
                                preview = Some(bed(c));
                            }
                            swatch(ui, bed(c));
                            ui.end_row();
                            ui.label("Bedtime brightness");
                            let (_, d) = percent_slider(ui, &mut c.bedtime_brightness);
                            if d {
                                preview = Some(bed(c));
                            }
                            ui.label("");
                            ui.end_row();
                        });
                        ui.weak("Fades in over half an hour and lasts until morning.");
                    }
                });

                // ── Apps & games ───────────────────────────────────────
                section(ui, "Apps & games", |ui| {
                    ui.weak("Ember turns off (or uses a different colour) while these apps are in use. First match wins.");
                    ui.add_space(6.0);
                    for (label, preset) in Rule::presets() {
                        let mut on = self.cfg.rules.contains(&preset);
                        if ui.checkbox(&mut on, label).changed() {
                            if on {
                                self.cfg.rules.push(preset);
                            } else {
                                self.cfg.rules.retain(|r| *r != preset);
                            }
                            self.sync_proc_text();
                        }
                    }
                    ui.add_space(6.0);
                    let mut remove = None;
                    for (i, rule) in self.cfg.rules.iter_mut().enumerate() {
                        ui.push_id(i, |ui| {
                            egui::Frame::new()
                                .fill(ui.visuals().faint_bg_color)
                                .corner_radius(6.0)
                                .inner_margin(8.0)
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    rule_editor(ui, rule, &mut self.proc_text[i], &mut self.title_text[i], &mut preview, &mut remove, i);
                                });
                        });
                        ui.add_space(4.0);
                    }
                    if let Some(i) = remove {
                        self.cfg.rules.remove(i);
                        self.proc_text.remove(i);
                        self.title_text.remove(i);
                    }

                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("running")
                            .width(220.0)
                            .selected_text(if self.new_app.is_empty() { "Pick a running app…" } else { &self.new_app })
                            .show_ui(ui, |ui| {
                                for n in &self.running_apps {
                                    ui.selectable_value(&mut self.new_app, n.clone(), n);
                                }
                            });
                        if ui.button("Refresh").on_hover_text("Refresh the list of running apps").clicked() {
                            self.refresh_running_apps();
                        }
                        let label = if self.new_app.is_empty() { "Add empty rule" } else { "Add rule" };
                        if ui.button(label).clicked() {
                            let app = std::mem::take(&mut self.new_app);
                            self.cfg.rules.push(Rule {
                                name: (!app.is_empty()).then(|| app.clone()),
                                processes: if app.is_empty() { vec![] } else { vec![app.clone()] },
                                when: When::Focused,
                                ..Rule::default()
                            });
                            self.proc_text.push(app);
                            self.title_text.push(String::new());
                        }
                    });
                });

                // ── Displays ───────────────────────────────────────────
                if self.displays.len() >= 2 || !self.cfg.displays.is_empty() {
                    section(ui, "Displays", |ui| {
                        ui.weak("Untick a colour-critical screen, such as one for photo editing, to leave it alone. It keeps its calibration.");
                        ui.add_space(6.0);
                        let rows = display_rows(&self.displays, &self.cfg.displays);
                        egui::Grid::new("displays").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                            for row in &rows {
                                ui.horizontal(|ui| {
                                    ui.label(&row.label);
                                    if !row.connected {
                                        ui.weak(format!("{} · not connected", row.id));
                                    } else if row.label != row.id {
                                        ui.weak(&row.id);
                                    }
                                });
                                let mut tint = config::tints_display(&self.cfg.displays, &row.id);
                                if ui.checkbox(&mut tint, "Tint this display").changed() {
                                    config::set_display_tint(&mut self.cfg.displays, &row.id, tint);
                                }
                                ui.end_row();
                            }
                        });
                        if !self.per_display {
                            ui.colored_label(palette::EMBER_HOT, "KDE's Night Light tints every screen, so these settings have no effect here.");
                        }
                    });
                }

                // ── Advanced ───────────────────────────────────────────
                section(ui, "Advanced", |ui| {
                    egui::Grid::new("adv").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                        let g = &mut self.cfg.general;
                        ui.label("Fade when an app rule switches");
                        ui.add(egui::Slider::new(&mut g.fade_secs, 0.0..=3.0).suffix(" s").fixed_decimals(1));
                        ui.end_row();
                        ui.label("Ease in after a jump");
                        ui.add(egui::Slider::new(&mut g.jump_fade_minutes, 0.0..=config::MAX_JUMP_FADE_MINUTES).suffix(" min").fixed_decimals(0))
                            .on_hover_text("When Ember starts in the evening or the PC wakes from sleep, fade into the right colour over this long instead of switching at once. 0 = at once.");
                        ui.end_row();
                        ui.label("Check apps every");
                        ui.add(egui::Slider::new(&mut g.poll_interval_secs, 1..=10).suffix(" s"));
                        ui.end_row();
                        ui.label("Toggle hotkey");
                        hotkey_button(ui, &mut g.toggle_hotkey, &mut self.recording_hotkey, &mut self.hotkey_error);
                        ui.end_row();
                    });
                    if let Some(e) = &self.hotkey_error {
                        ui.colored_label(palette::ERROR, e);
                    }
                    ui.checkbox(&mut self.cfg.general.preserve_calibration, "Keep my display's calibration curve")
                        .on_hover_text("Tint on top of the ICC/VCGT curve loaded by your calibration software, and put it back when Ember turns off. Untick to reset to neutral instead.");
                    ui.horizontal(|ui| {
                        if ui.button("Open config file").clicked() {
                            let _ = config::ensure_exists(&self.path);
                            let _ = tray_model::open_in_default_app(&self.path);
                        }
                        if ui.button("Reset to defaults").clicked() {
                            self.cfg = Config::parse(config::DEFAULT_CONFIG).unwrap_or_default();
                            self.sync_proc_text();
                        }
                    });
                    ui.weak(self.path.display().to_string());
                });

                ui.add_space(8.0);
                if let Some((msg, err)) = &self.message {
                    let color = if *err { palette::ERROR } else { ui.visuals().weak_text_color() };
                    ui.colored_label(color, msg);
                }
            });
        });

        // Process lists are edited as text; turn them back into the config.
        let list = |text: &str| -> Vec<String> {
            text.split(',')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect()
        };
        for ((rule, procs), titles) in self
            .cfg
            .rules
            .iter_mut()
            .zip(&self.proc_text)
            .zip(&self.title_text)
        {
            let (procs, titles) = (list(procs), list(titles));
            if rule.processes != procs {
                rule.processes = procs;
            }
            if rule.titles != titles {
                rule.titles = titles;
            }
            // Titles and fullscreen are about the active window.
            if rule.fullscreen || !rule.titles.is_empty() {
                rule.when = When::Focused;
            }
        }

        if self.cfg != before {
            self.last_edit = Some(Instant::now());
            self.message = None;
        }
        if self.cfg.general.preserve_calibration != before.general.preserve_calibration
            && let Some(b) = self.backend.as_mut()
        {
            b.set_preserve_calibration(self.cfg.general.preserve_calibration);
        }
        if self.cfg.displays != before.displays
            && let Some(b) = self.backend.as_mut()
        {
            b.set_displays(&self.cfg.displays);
        }

        match preview {
            Some(s) => self.preview(s),
            None => self.end_preview(),
        }
        self.save_if_due();

        // Keep the "Right now" line and the debounce/preview timers ticking.
        let next = if self.previewing.is_some() || self.last_edit.is_some() {
            100
        } else {
            1000
        };
        ctx.request_repaint_after(Duration::from_millis(next));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.end_preview();
        self.last_edit = Some(Instant::now() - SAVE_DEBOUNCE);
        self.save_if_due();
    }
}

fn rule_editor(
    ui: &mut Ui,
    rule: &mut Rule,
    procs: &mut String,
    titles: &mut String,
    preview: &mut Option<ColorSetting>,
    remove: &mut Option<usize>,
    i: usize,
) {
    ui.horizontal(|ui| {
        let mut name = rule.name.clone().unwrap_or_default();
        let r = ui.add(
            egui::TextEdit::singleline(&mut name)
                .hint_text("Name")
                .desired_width(180.0),
        );
        if r.changed() {
            rule.name = (!name.trim().is_empty()).then_some(name);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Remove").clicked() {
                *remove = Some(i);
            }
        });
    });
    egui::Grid::new("rule")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Processes");
            ui.add(
                egui::TextEdit::singleline(procs)
                    .hint_text("e.g. cs2, valorant")
                    .desired_width(f32::INFINITY),
            )
            .on_hover_text("Comma-separated. Case doesn't matter and \".exe\" is optional.");
            ui.end_row();

            ui.label("Window title");
            ui.add(
                egui::TextEdit::singleline(titles)
                    .hint_text("optional, e.g. YouTube, Netflix")
                    .desired_width(f32::INFINITY),
            )
            .on_hover_text("Only while the active window's title contains one of these. Comma-separated, case doesn't matter.");
            ui.end_row();

            ui.label("Only for");
            ui.horizontal(|ui| {
                ui.checkbox(&mut rule.fullscreen, "fullscreen windows");
                ui.checkbox(&mut rule.steam_games, "Steam games");
            });
            ui.end_row();

            ui.label("Applies");
            let window_only = rule.fullscreen || !titles.trim().is_empty();
            ui.add_enabled_ui(!window_only, |ui| egui::ComboBox::from_id_salt("when")
                .selected_text(match rule.when {
                    When::Focused => "while it's the active window",
                    When::Running => "while it's running",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut rule.when,
                        When::Focused,
                        "while it's the active window",
                    );
                    ui.selectable_value(&mut rule.when, When::Running, "while it's running");
                }))
            .response
            .on_disabled_hover_text("Window titles and fullscreen are about the active window.");
            ui.end_row();

            ui.label("Action");
            let disables = rule.temperature.is_none() && rule.brightness.is_none();
            egui::ComboBox::from_id_salt("action")
                .selected_text(if disables {
                    "Turn Ember off"
                } else {
                    "Use a custom colour"
                })
                .show_ui(ui, |ui| {
                    if ui.selectable_label(disables, "Turn Ember off").clicked() {
                        rule.temperature = None;
                        rule.brightness = None;
                    }
                    if ui
                        .selectable_label(!disables, "Use a custom colour")
                        .clicked()
                        && disables
                    {
                        rule.temperature = Some(5000);
                    }
                });
            ui.end_row();

            if rule.temperature.is_some() || rule.brightness.is_some() {
                let mut k = rule.temperature.unwrap_or(color::NEUTRAL_K);
                let b = rule.brightness.unwrap_or(1.0);
                ui.label("Temperature");
                ui.horizontal(|ui| {
                    let (changed, dragging) = kelvin_slider(ui, &mut k, 1200..=6500);
                    if changed {
                        rule.temperature = Some(k);
                    }
                    if dragging {
                        *preview = Some(ColorSetting {
                            temperature: k,
                            brightness: b,
                        });
                    }
                    swatch(
                        ui,
                        ColorSetting {
                            temperature: k,
                            brightness: b,
                        },
                    );
                });
                ui.end_row();
            }
        });
    if rule.processes.is_empty()
        && titles.trim().is_empty()
        && !rule.fullscreen
        && !rule.steam_games
    {
        ui.colored_label(
            palette::EMBER_HOT,
            "Add a process or a window title, or tick fullscreen or Steam games.",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Rect, pos2, vec2};

    // A 1920x1080 screen with a 48px taskbar, and a 520x680 window.
    const SIZE: egui::Vec2 = vec2(520.0, 680.0);

    #[test]
    fn opens_above_a_bottom_taskbar_centred_on_the_icon() {
        let work = Rect::from_min_max(pos2(0.0, 0.0), pos2(1920.0, 1032.0));
        let icon = Rect::from_min_size(pos2(1500.0, 1040.0), vec2(24.0, 32.0));
        let p = place_near(icon, work, SIZE);
        assert_eq!(p, pos2(1512.0 - 260.0, 1032.0 - 680.0 - SCREEN_GAP));
    }

    #[test]
    fn stays_on_screen_when_the_icon_is_near_the_corner() {
        let work = Rect::from_min_max(pos2(0.0, 0.0), pos2(1920.0, 1032.0));
        let icon = Rect::from_min_size(pos2(1880.0, 1040.0), vec2(24.0, 32.0));
        let p = place_near(icon, work, SIZE);
        assert_eq!(p.x, 1920.0 - 520.0 - SCREEN_GAP);
    }

    #[test]
    fn opens_beside_a_right_hand_taskbar() {
        let work = Rect::from_min_max(pos2(0.0, 0.0), pos2(1872.0, 1080.0));
        let icon = Rect::from_min_size(pos2(1884.0, 900.0), vec2(24.0, 24.0));
        let p = place_near(icon, work, SIZE);
        assert_eq!(p.x, 1872.0 - 520.0 - SCREEN_GAP);
        assert_eq!(
            p.y,
            1080.0 - 680.0 - SCREEN_GAP,
            "clamped above the bottom edge"
        );
    }

    #[test]
    fn records_hotkeys_from_egui_key_presses() {
        let ctrl_alt = egui::Modifiers {
            ctrl: true,
            alt: true,
            ..Default::default()
        };
        let none = egui::Modifiers::NONE;
        let rec = |k, m| recorded_hotkey(k, m).map(|h| h.to_string()).ok();
        assert_eq!(
            rec(egui::Key::End, ctrl_alt).as_deref(),
            Some("Ctrl+Alt+End")
        );
        assert_eq!(
            rec(egui::Key::ArrowLeft, ctrl_alt).as_deref(),
            Some("Ctrl+Alt+Left")
        );
        assert_eq!(
            rec(egui::Key::Num7, ctrl_alt).as_deref(),
            Some("Ctrl+Alt+7")
        );
        assert_eq!(
            rec(egui::Key::PageDown, ctrl_alt).as_deref(),
            Some("Ctrl+Alt+PageDown")
        );
        assert_eq!(rec(egui::Key::F13, none).as_deref(), Some("F13"));
        assert_eq!(rec(egui::Key::A, none), None, "needs a modifier");
        assert_eq!(rec(egui::Key::Minus, ctrl_alt), None, "not a supported key");
    }

    #[test]
    fn display_rows_list_connected_then_configured() {
        let connected = vec![
            (r"\\.\DISPLAY1".to_string(), "DELL U2720Q".to_string()),
            (
                r"\\.\DISPLAY2".to_string(),
                "Generic PnP Monitor".to_string(),
            ),
        ];
        let configured = vec![
            config::Display {
                tint: false,
                ..config::Display::new(r"\\.\display2")
            },
            config::Display {
                tint: false,
                ..config::Display::new("HDMI-1")
            },
        ];
        let rows = display_rows(&connected, &configured);
        let ids: Vec<(&str, bool)> = rows.iter().map(|r| (r.id.as_str(), r.connected)).collect();
        assert_eq!(
            ids,
            [
                (r"\\.\DISPLAY1", true),
                (r"\\.\DISPLAY2", true),
                ("HDMI-1", false)
            ]
        );
        assert_eq!(rows[0].label, "DELL U2720Q");
    }

    #[test]
    fn opens_above_an_icon_in_the_overflow_flyout() {
        let work = Rect::from_min_max(pos2(0.0, 0.0), pos2(1920.0, 1032.0));
        let icon = Rect::from_min_size(pos2(1400.0, 960.0), vec2(32.0, 32.0));
        let p = place_near(icon, work, SIZE);
        assert_eq!(p, pos2(1416.0 - 260.0, 960.0 - 680.0 - SCREEN_GAP));
    }
}
