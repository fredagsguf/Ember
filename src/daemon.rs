//! The long-running loop shared by `ember run` and `ember-tray`.

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

/// Processes that are "the desktop", not an app the user would want a rule for.
/// Used so that clicking the tray icon doesn't make the taskbar the "current app".
const SHELL_PROCESSES: &[&str] = &[
    "explorer",
    "ember",
    "ember-tray",
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
fn single_instance_lock() -> Result<File> {
    let dir = config::state_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("ember.lock");
    let f = File::create(&path).with_context(|| format!("creating {}", path.display()))?;
    match f.try_lock() {
        Ok(()) => Ok(f),
        Err(std::fs::TryLockError::WouldBlock) => {
            bail!("another Ember instance is already running (ember run or ember-tray)")
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
    let mut backend = gamma::open(opts.dry_run)?;
    let mut watcher = ProcessWatcher::new()?;

    log::info!(
        "ember running with backend {} ({} rule(s))",
        backend.name(),
        cfg.rules.len()
    );
    let mut current: Option<ColorSetting> = None;
    let mut last_reason: Option<Reason> = None;
    let mut last_app: Option<String> = None;

    loop {
        let m = mtime(path);
        if m != cfg_mtime {
            cfg_mtime = m;
            match load_or_default(path) {
                Ok(c) => {
                    log::info!("config reloaded ({} rule(s))", c.rules.len());
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
        let d = engine::decide(&cfg, &snap);
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
            }));
        }

        if stop {
            break;
        }
        // Poll quickly while previewing so we notice the end of the drag promptly.
        let wait = if is_preview(&last_reason) {
            Duration::from_millis(200)
        } else {
            Duration::from_secs(cfg.general.poll_interval_secs.max(1))
        };
        match rx.recv_timeout(wait) {
            Ok(Msg::Wake) | Err(RecvTimeoutError::Timeout) => {}
            Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    log::info!("restoring neutral colours");
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
