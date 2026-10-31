//! `ember-tray` — Ember with a system tray icon. No console window on Windows.

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
#[command(name = "ember-tray", version, about = "Ember in the system tray")]
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
        if let Err(e) = settings::run(config_path) {
            fatal(&format!("{e:#}"));
        }
        return;
    }
    init_logging("ember.log");
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
    eprintln!("ember-tray: {msg}");
    std::process::exit(1)
}
