//! Ember — an open-source f.lux alternative with per-game rules.

mod color;
mod config;
mod engine;
mod gamma;
mod rules;
mod schedule;
mod solar;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use color::ColorSetting;
use config::Config;
use engine::{ProcessWatcher, Reason};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

#[derive(Parser)]
#[command(
    name = "ember",
    version,
    about = "Warm your screen at night — and stay out of the way while you game."
)]
struct Cli {
    /// Path to config file (default: platform config dir, see `ember config-path`)
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run Ember in the foreground (the default when no command is given)
    Run {
        /// Don't touch the display, just log what would happen
        #[arg(long)]
        dry_run: bool,
    },
    /// Show what Ember would do right now and why
    Status,
    /// Apply a fixed colour temperature once and exit (stays until changed)
    Set {
        /// Kelvin, 1000..10000 (6500 = neutral)
        kelvin: u32,
        #[arg(short, long, default_value_t = 1.0)]
        brightness: f32,
    },
    /// Reset all displays to neutral colours and exit
    Reset,
    /// Pause Ember (a running instance goes neutral). Without --minutes pauses until `resume`.
    Pause {
        #[arg(short, long)]
        minutes: Option<u64>,
    },
    /// Undo `pause`
    Resume,
    /// List running process names (and the focused one), for writing rules
    Processes,
    /// Write the example config to the config path
    Init {
        /// Overwrite an existing file
        #[arg(long)]
        force: bool,
    },
    /// Print where the config file is expected
    ConfigPath,
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_secs()
        .init();
    if let Err(e) = real_main() {
        eprintln!("ember: {e:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let cli = Cli::parse();
    let path = cli
        .config
        .clone()
        .unwrap_or_else(config::default_config_path);
    match cli.command.unwrap_or(Cmd::Run { dry_run: false }) {
        Cmd::Run { dry_run } => run(&path, dry_run),
        Cmd::Status => status(&path),
        Cmd::Set { kelvin, brightness } => {
            let mut b = gamma::open(false)?;
            let s = ColorSetting {
                temperature: kelvin,
                brightness,
            }
            .clamped();
            b.apply(s)?;
            b.sync()?;
            println!("applied {}K @ {:.0}%", s.temperature, s.brightness * 100.0);
            Ok(())
        }
        Cmd::Reset => {
            gamma::open(false)?.restore()?;
            println!("displays reset to neutral");
            Ok(())
        }
        Cmd::Pause { minutes } => {
            engine::set_pause(minutes)?;
            match minutes {
                Some(m) => println!("paused for {m} minute(s)"),
                None => println!("paused until `ember resume`"),
            }
            Ok(())
        }
        Cmd::Resume => {
            engine::clear_pause()?;
            println!("resumed");
            Ok(())
        }
        Cmd::Processes => {
            let mut w = ProcessWatcher::new()?;
            for n in w.all_names() {
                println!("{n}");
            }
            if let Ok(mut b) = gamma::open(false)
                && let Some(f) = w.focused_name(b.as_mut())
            {
                eprintln!("\nfocused: {f}");
            }
            Ok(())
        }
        Cmd::Init { force } => {
            if path.exists() && !force {
                bail!(
                    "{} already exists (use --force to overwrite)",
                    path.display()
                );
            }
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, config::DEFAULT_CONFIG)?;
            println!("wrote {}", path.display());
            Ok(())
        }
        Cmd::ConfigPath => {
            println!("{}", path.display());
            Ok(())
        }
    }
}

fn load_or_default(path: &Path) -> Result<Config> {
    if path.exists() {
        Config::load(path)
    } else {
        log::warn!(
            "no config at {} — using defaults (run `ember init` to create one)",
            path.display()
        );
        Ok(Config::default())
    }
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn status(path: &Path) -> Result<()> {
    let cfg = load_or_default(path)?;
    let mut backend = gamma::open(false).or_else(|e| {
        log::warn!("{e:#}; focus detection unavailable");
        gamma::open(true)
    })?;
    let mut watcher = ProcessWatcher::new()?;
    let snap = watcher.snapshot(&cfg, backend.as_mut());
    let d = engine::decide(&cfg, &snap);
    println!("config:     {}", path.display());
    println!("backend:    {}", backend.name());
    println!(
        "schedule:   {}K @ {:.0}%",
        d.scheduled.temperature,
        d.scheduled.brightness * 100.0
    );
    println!(
        "focused:    {}",
        snap.focused.as_deref().unwrap_or("(unknown)")
    );
    println!(
        "effective:  {}K @ {:.0}%  ({})",
        d.setting.temperature,
        d.setting.brightness * 100.0,
        d.reason
    );
    Ok(())
}

fn run(path: &Path, dry_run: bool) -> Result<()> {
    let mut cfg = load_or_default(path)?;
    let mut cfg_mtime = mtime(path);
    let mut backend = gamma::open(dry_run)?;
    let mut watcher = ProcessWatcher::new()?;

    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        ctrlc::set_handler(move || stop.store(true, Ordering::SeqCst))
            .context("installing signal handler")?;
    }

    log::info!(
        "ember running with backend {} ({} rule(s))",
        backend.name(),
        cfg.rules.len()
    );
    let mut current: Option<ColorSetting> = None;
    let mut last_reason: Option<Reason> = None;

    while !stop.load(Ordering::SeqCst) {
        // Live config reload.
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
        let state_changed = last_reason.is_some() && last_reason.as_ref() != Some(&d.reason);
        match current {
            Some(from) if state_changed && cfg.general.fade_secs > 0.0 => fade(
                backend.as_mut(),
                from,
                d.setting,
                cfg.general.fade_secs,
                &stop,
            ),
            // Re-apply every tick: games, driver resets and display hot-plugs can clobber the ramp.
            _ => apply(backend.as_mut(), d.setting),
        }
        current = Some(d.setting);
        last_reason = Some(d.reason);

        sleep_interruptible(
            Duration::from_secs(cfg.general.poll_interval_secs.max(1)),
            &stop,
        );
    }

    log::info!("restoring neutral colours");
    backend.restore()
}

fn apply(backend: &mut dyn gamma::Backend, s: ColorSetting) {
    if let Err(e) = backend.apply(s) {
        log::debug!("apply failed: {e:#}");
    }
}

fn fade(
    backend: &mut dyn gamma::Backend,
    from: ColorSetting,
    to: ColorSetting,
    secs: f64,
    stop: &AtomicBool,
) {
    const STEP: Duration = Duration::from_millis(30);
    let total = Duration::from_secs_f64(secs);
    let start = Instant::now();
    loop {
        let t = (start.elapsed().as_secs_f64() / total.as_secs_f64()).min(1.0);
        apply(backend, color::lerp(from, to, t));
        if t >= 1.0 || stop.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(STEP);
    }
}

fn sleep_interruptible(d: Duration, stop: &AtomicBool) {
    let end = Instant::now() + d;
    while Instant::now() < end && !stop.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(100).min(end - Instant::now()));
    }
}
