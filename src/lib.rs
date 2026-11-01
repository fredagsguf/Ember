//! Ember — an open-source f.lux alternative with per-app rules.
//! The `ember` tray app (`src/bin/ember/`) is a thin front-end over this library.

pub mod autostart;
pub mod color;
pub mod config;
pub mod daemon;
pub mod engine;
pub mod gamma;
pub mod rules;
pub mod schedule;
pub mod solar;
pub mod tray_model;
