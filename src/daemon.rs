//! The long-running loop behind the tray app.

use crate::color::{self, ColorSetting};
use crate::config::{self, Config};
use crate::engine::{self, ProcessWatcher, Reason};
use crate::gamma::{self, Backend};
use anyhow::{Context, Result, bail};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

/// A fade towards the scheduled colour after it jumped, started at `start` from `from`.
struct JumpFade {
    from: ColorSetting,
    start: Instant,
}

/// Processes that are "the desktop", not an app the user would want a rule for.
/// Used so that clicking the tray icon doesn't make the taskbar the "current app".
const SHELL_PROCESSES: &[&str] = &[
    "explorer",
    "ember",
    "shellexperiencehost",
    "startmenuexperiencehost",
    "searchhost",
    "searchapp",
    "applicationframehost",
    "lockapp",
    "plasmashell",
    "gnome-shell",
    "xfce4-panel",
    "xfdesktop",
    "lxpanel",
    "polybar",
    "tint2",
];

#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub setting: ColorSetting,
    pub scheduled: ColorSetting,
    pub reason: Reason,
    /// Most recently focused real application (taskbar/tray excluded).
    pub last_app: Option<String>,
    pub config: Config,
    pub backend: &'static str,
    /// Windows refused the last warm colour (see `Backend::warm_colours_refused`).
    pub warm_refused: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum State {
    #[default]
    Starting,
    Running(Box<Status>),
    Failed(String),
}

pub enum Msg {
    /// Re-evaluate now (pause state or config changed).
    Wake,
    Stop,
}

/// Lets a front-end talk to a daemon running on another thread.
#[derive(Clone)]
pub struct Handle {
    tx: Sender<Msg>,
    state: Arc<Mutex<State>>,
}

impl Handle {
    pub fn state(&self) -> State {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }
    pub fn wake(&self) {
        let _ = self.tx.send(Msg::Wake);
    }
    pub fn stop(&self) {
        let _ = self.tx.send(Msg::Stop);
    }
}

pub struct Options {
    pub config_path: PathBuf,
    pub dry_run: bool,
}

/// Start the daemon on a background thread. Fails immediately if another instance is running.
pub fn spawn(opts: Options) -> Result<(Handle, JoinHandle<Result<()>>)> {
    let lock = single_instance_lock()?;
    let (tx, rx) = channel();
    let state = Arc::new(Mutex::new(State::Starting));
    let handle = Handle {
        tx,
        state: state.clone(),
    };
    let join = std::thread::Builder::new()
        .name("ember-daemon".into())
        .spawn(move || {
            let r = run_locked(&opts, rx, &state);
            drop(lock);
            if let Err(e) = &r {
                log::error!("{e:#}");
                if let Ok(mut s) = state.lock() {
                    *s = State::Failed(format!("{e:#}"));
                }
            }
            r
        })
        .context("spawning daemon thread")?;
    Ok((handle, join))
}

/// Only one Ember may drive the displays at a time, otherwise they fight over the gamma ramp.
fn lock_path() -> PathBuf {
    config::state_dir().join("ember.lock")
}

/// Whether Ember (the tray app) is running, i.e. another process holds the instance lock.
pub fn is_running() -> bool {
    let Ok(f) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path())
    else {
        return false;
    };
    matches!(f.try_lock(), Err(std::fs::TryLockError::WouldBlock))
}

fn single_instance_lock() -> Result<File> {
    std::fs::create_dir_all(config::state_dir())?;
    let path = lock_path();
    let f = File::create(&path).with_context(|| format!("creating {}", path.display()))?;
    match f.try_lock() {
        Ok(()) => Ok(f),
        Err(std::fs::TryLockError::WouldBlock) => {
            bail!("another Ember instance is already running")
        }
        Err(std::fs::TryLockError::Error(e)) => Err(e).context("locking instance file"),
    }
}

fn load_or_default(path: &Path) -> Result<Config> {
    if path.exists() {
        Config::load(path)
    } else {
        log::warn!("no config at {} — using defaults", path.display());
        Ok(Config::default())
    }
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Run the daemon on the current thread until `Msg::Stop` arrives (or the sender is dropped).
pub fn run(opts: &Options, rx: Receiver<Msg>, state: &Mutex<State>) -> Result<()> {
    let _lock = single_instance_lock()?;
    run_locked(opts, rx, state)
}

fn run_locked(opts: &Options, rx: Receiver<Msg>, state: &Mutex<State>) -> Result<()> {
    let path = &opts.config_path;
    let mut cfg = load_or_default(path)?;
    let mut cfg_mtime = mtime(path);
    let mut backend = gamma::open(
        opts.dry_run,
        cfg.general.preserve_calibration,
        &cfg.displays,
    )?;
    let mut watcher = ProcessWatcher::new()?;

    log::info!(
        "ember running with backend {} ({} rule(s))",
        backend.name(),
        cfg.rules.len()
    );
    let mut current: Option<ColorSetting> = None;
    let mut last_reason: Option<Reason> = None;
    let mut last_app: Option<String> = None;
    let mut jump: Option<JumpFade> = None;

    loop {
        let m = mtime(path);
        if m != cfg_mtime {
            cfg_mtime = m;
            match load_or_default(path) {
                Ok(c) => {
                    log::info!("config reloaded ({} rule(s))", c.rules.len());
                    backend.set_preserve_calibration(c.general.preserve_calibration);
                    backend.set_displays(&c.displays);
                    cfg = c;
                }
                Err(e) => log::error!("keeping previous config: {e:#}"),
            }
        }

        let snap = watcher.snapshot(&cfg, backend.as_mut());
        if let Some(f) = &snap.focused
            && !SHELL_PROCESSES.contains(&f.as_str())
        {
            last_app = Some(f.clone());
        }
        let mut d = engine::decide(&cfg, &snap);
        // The schedule can still jump: Ember starting in the evening (the screen is neutral until
        // now), waking from sleep, or an edited schedule. Ease into those over minutes, one poll
        // at a time so rules, pausing and previews still react at once. While a rule or pause
        // takes over, the fade keeps running underneath.
        let jump_fade = cfg.general.jump_fade_minutes * 60.0;
        if d.reason == Reason::Schedule
            && jump_fade > 0.0
            && jump.is_none()
            && matches!(last_reason, None | Some(Reason::Schedule))
        {
            let from = current.unwrap_or(ColorSetting::NEUTRAL);
            if is_jump(from, d.setting) {
                jump = Some(JumpFade {
                    from,
                    start: Instant::now(),
                });
            }
        }
        if let Some(j) = &jump {
            let t = j.start.elapsed().as_secs_f64() / jump_fade;
            if t >= 1.0 || jump_fade <= 0.0 {
                jump = None;
            } else if d.reason == Reason::Schedule {
                d.setting = color::lerp(j.from, d.setting, t);
            }
        }
        backend.set_app_rule_active(matches!(d.reason, Reason::Rule(_)));
        if last_reason.as_ref() != Some(&d.reason) {
            log::info!(
                "{}K @ {:.0}% ({})",
                d.setting.temperature,
                d.setting.brightness * 100.0,
                d.reason
            );
        }

        // Fade only on a state change (rule on/off, pause); schedule changes are already gradual.
        // Previews must track the slider exactly, so never fade into or out of them.
        let state_changed = last_reason.is_some()
            && last_reason.as_ref() != Some(&d.reason)
            && d.reason != Reason::Preview
            && last_reason != Some(Reason::Preview);
        let mut stop = false;
        match current {
            Some(from) if state_changed && cfg.general.fade_secs > 0.0 => {
                stop = fade(
                    backend.as_mut(),
                    from,
                    d.setting,
                    cfg.general.fade_secs,
                    &rx,
                )
            }
            // Re-apply every tick: games, driver resets and display hot-plugs can clobber the ramp.
            _ => apply(backend.as_mut(), d.setting),
        }
        current = Some(d.setting);
        last_reason = Some(d.reason.clone());

        if let Ok(mut s) = state.lock() {
            *s = State::Running(Box::new(Status {
                setting: d.setting,
                scheduled: d.scheduled,
                reason: d.reason,
                last_app: last_app.clone(),
                config: cfg.clone(),
                backend: backend.name(),
                warm_refused: backend.warm_colours_refused(),
            }));
        }

        if stop {
            break;
        }
        // Poll quickly while previewing so we notice the end of the drag promptly, and at least
        // every second while easing into a jump so its steps stay too small to see.
        let wait = if is_preview(&last_reason) {
            Duration::from_millis(200)
        } else if jump.is_some() {
            Duration::from_secs(1)
        } else {
            Duration::from_secs(cfg.general.poll_interval_secs.max(1))
        };
        match rx.recv_timeout(wait) {
            Ok(Msg::Wake) | Err(RecvTimeoutError::Timeout) => {}
            Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    log::info!("restoring normal colours");
    backend.restore()
}

fn is_preview(r: &Option<Reason>) -> bool {
    matches!(r, Some(Reason::Preview))
}

fn apply(backend: &mut dyn Backend, s: ColorSetting) {
    if let Err(e) = backend.apply(s) {
        log::debug!("apply failed: {e:#}");
    }
}

/// Whether going from `a` to `b` is a jump rather than one step of a gradual schedule transition.
/// Even a 1-minute transition moves only about 100K (or 0.03 brightness) per 2-second poll.
fn is_jump(a: ColorSetting, b: ColorSetting) -> bool {
    a.temperature.abs_diff(b.temperature) > 150 || (a.brightness - b.brightness).abs() > 0.05
}

/// Returns true if a stop was requested during the fade.
fn fade(
    backend: &mut dyn Backend,
    from: ColorSetting,
    to: ColorSetting,
    secs: f64,
    rx: &Receiver<Msg>,
) -> bool {
    const STEP: Duration = Duration::from_millis(30);
    let start = Instant::now();
    loop {
        let t = (start.elapsed().as_secs_f64() / secs).min(1.0);
        apply(backend, color::lerp(from, to, t));
        if t >= 1.0 {
            return false;
        }
        match rx.recv_timeout(STEP) {
            Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => return true,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(temperature: u32, brightness: f32) -> ColorSetting {
        ColorSetting {
            temperature,
            brightness,
        }
    }

    #[test]
    fn gradual_schedule_steps_are_not_jumps() {
        // A 45-minute 6500K -> 3400K transition polled every 2 s.
        assert!(!is_jump(k(6500, 1.0), k(6498, 1.0)));
        // A 1-minute transition, also with brightness going 1.0 -> 0.1.
        assert!(!is_jump(k(6500, 1.0), k(6397, 0.97)));
    }

    #[test]
    fn evening_start_and_wake_from_sleep_are_jumps() {
        assert!(is_jump(ColorSetting::NEUTRAL, k(3400, 1.0)));
        assert!(is_jump(k(3400, 1.0), k(6500, 1.0)));
        assert!(is_jump(k(6500, 1.0), k(6500, 0.7)));
    }
}
