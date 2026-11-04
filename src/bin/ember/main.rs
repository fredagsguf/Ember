//! `ember` — Ember with a system tray icon. No console window on Windows.

#![cfg_attr(windows, windows_subsystem = "windows")]

use clap::Parser;
use ember::daemon;
use std::path::PathBuf;

#[cfg(target_os = "linux")]
mod linux;
mod settings;
#[cfg(windows)]
mod windows;

#[derive(Parser)]
#[command(
    name = "ember",
    version,
    about = "Warm your screen at night — and stay out of the way while you game."
)]
struct Cli {
    /// Path to config file (default: platform config dir)
    #[arg(short, long)]
    config: Option<PathBuf>,
    /// Don't touch the display, just log what would happen
    #[arg(long)]
    dry_run: bool,
    /// Open the settings window instead of the tray
    #[arg(long)]
    settings: bool,
    /// Tray icon position (x,y,width,height in physical pixels) to open the settings window at
    #[arg(long, hide = true, requires = "settings")]
    anchor: Option<ember::tray_model::Anchor>,
    /// Started by an update: wait for the Ember with this process id to quit first
    #[arg(long, hide = true, conflicts_with = "settings")]
    replaces: Option<u32>,
}

fn init_logging(file_name: &str) {
    let mut b = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));
    b.format_timestamp_secs();
    let path = ember::tray_model::log_path().with_file_name(file_name);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(f) = std::fs::File::create(&path) {
        b.target(env_logger::Target::Pipe(Box::new(f)));
    }
    b.init();
}

fn main() {
    let cli = Cli::parse();
    let config_path = cli
        .config
        .unwrap_or_else(ember::config::default_config_path);
    if cli.settings {
        init_logging("settings.log");
        if let Err(e) = settings::run(config_path, cli.anchor) {
            fatal(&format!("{e:#}"));
        }
        return;
    }
    init_logging("ember.log");
    #[cfg(windows)]
    if let Some(pid) = cli.replaces {
        // It holds the instance lock and the displays until it has quit.
        ember::update::wait_for_exit(pid, std::time::Duration::from_secs(30));
        ember::update::cleanup_after_update();
        log::info!("updated to Ember {}", env!("CARGO_PKG_VERSION"));
    }
    #[cfg(not(windows))]
    let _ = cli.replaces;
    let opts = daemon::Options {
        config_path: config_path.clone(),
        dry_run: cli.dry_run,
    };

    let (handle, join) = match daemon::spawn(opts) {
        Ok(x) => x,
        Err(e) => fatal(&format!("{e:#}")),
    };

    #[cfg(windows)]
    let ui = windows::run(handle.clone(), config_path);
    #[cfg(target_os = "linux")]
    let ui = linux::run(handle.clone(), config_path);

    handle.stop();
    let _ = join.join();
    if let Err(e) = ui {
        fatal(&format!("{e:#}"));
    }
}

fn fatal(msg: &str) -> ! {
    log::error!("{msg}");
    #[cfg(windows)]
    windows::message_box(msg);
    #[cfg(not(windows))]
    eprintln!("ember: {msg}");
    std::process::exit(1)
}
