//! Keep each display's calibration curve (ICC/VCGT, loaded by Windows colour management,
//! DisplayCAL, xcalib, dispwin…) as the base Ember's tint is applied on top of.
//!
//! The base is read from the display when a backend opens. The catch: if Ember was killed while
//! the screen was warm, the ramp found there is Ember's own tint, and taking that as calibration
//! would make the tint permanent (and stack it again on every start). So captured bases are saved
//! in the state dir, and a ramp only counts as new calibration if it is *not* a per-channel
//! scaling of the saved base or of identity, which is all Ember (or any f.lux-style tool) writes.

use crate::color::{ColorSetting, Ramp, resample};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// How far (in 16-bit units, about 0.5 %) a ramp may stray from a scaled curve and still count
/// as one. Covers Ember's rounding and drivers that keep fewer than 16 bits per entry.
const TOLERANCE: f64 = 328.0;
/// Ember never boosts a channel; the slack is for rounding.
const MAX_GAIN: f64 = 1.005;

/// What to use as a display's base, given the ramp found on it at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// The ramp is the saved base, possibly tinted by Ember: keep the saved base.
    KeepSaved,
    /// The ramp is identity, possibly tinted (a leftover tint, not calibration).
    Identity,
    /// Something else, so another tool loaded a curve: make it the new base.
    Capture,
}

pub fn decide(current: &Ramp, saved: Option<&Ramp>) -> Decision {
    if current.size().is_none() {
        return Decision::Identity;
    }
    if saved.is_some_and(|s| is_scaled(current, Some(s))) {
        Decision::KeepSaved
    } else if is_scaled(current, None) {
        Decision::Identity
    } else {
        Decision::Capture
    }
}

/// Is `current` the `base` curve (`None` = identity) scaled per channel by 0..=1, within rounding?
pub fn is_scaled(current: &Ramp, base: Option<&Ramp>) -> bool {
    let Some(size) = current.size() else {
        return false;
    };
    let identity;
    let base = match base {
        Some(b) => b,
        None => {
            identity = Ramp::identity(size);
            &identity
        }
    };
    current
        .channels()
        .iter()
        .zip(base.channels())
        .all(|(c, b)| channel_is_scaled(c, &resample(b, size)))
}

/// `base` is in 0..=1. Fits the gain by least squares, then checks every entry against it.
fn channel_is_scaled(current: &[u16], base: &[f64]) -> bool {
    let base: Vec<f64> = base.iter().map(|b| b * 65535.0).collect();
    let bb: f64 = base.iter().map(|b| b * b).sum();
    let cb: f64 = current.iter().zip(&base).map(|(&c, b)| c as f64 * b).sum();
    let k = if bb > 0.0 { cb / bb } else { 0.0 };
    k <= MAX_GAIN
        && current
            .iter()
            .zip(&base)
            .all(|(&c, b)| (c as f64 - k * b).abs() <= TOLERANCE)
}

/// Saved bases, keyed by a stable display name (Windows `\\.\DISPLAY1`, X11 output name).
/// Displays without calibration have no entry.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct Store {
    #[serde(default)]
    pub displays: BTreeMap<String, Ramp>,
}

impl Store {
    pub fn parse(text: &str) -> Result<Store> {
        let mut store: Store = toml::from_str(text)?;
        store.displays.retain(|_, r| r.size().is_some());
        Ok(store)
    }

    pub fn to_text(&self) -> Result<String> {
        let body = toml::to_string(self)?;
        Ok(format!(
            "# Written by Ember: the calibration curve found on each display, kept so a\n\
             # crash can't leave Ember's own tint behind as \"calibration\". Safe to delete.\n{body}"
        ))
    }

    /// Pick `display`'s base from the ramp currently on it (`None` = couldn't read it) and
    /// record the choice. Returns the base, `None` meaning identity.
    pub fn base_for(&mut self, display: &str, current: Option<&Ramp>) -> Option<Ramp> {
        let current = current?;
        match decide(current, self.displays.get(display)) {
            Decision::KeepSaved => self.displays.get(display).cloned(),
            Decision::Identity => {
                self.displays.remove(display);
                None
            }
            Decision::Capture => {
                self.displays.insert(display.to_string(), current.clone());
                Some(current.clone())
            }
        }
    }
}

fn store_path() -> PathBuf {
    crate::config::state_dir().join("calibration.toml")
}

fn load_store(path: &Path) -> Store {
    match std::fs::read_to_string(path) {
        Ok(text) => Store::parse(&text).unwrap_or_else(|e| {
            log::warn!("ignoring {}: {e:#}", path.display());
            Store::default()
        }),
        Err(_) => Store::default(),
    }
}

/// The settings window and the tray app may both write this, so each uses its own temp file.
fn save_store(path: &Path, store: &Store) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("toml.{}.tmp", std::process::id()));
    std::fs::write(&tmp, store.to_text()?).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

/// Each display's base curve, as a backend uses it.
pub struct Calibration {
    preserve: bool,
    /// Displays seen so far; `None` = identity base.
    bases: HashMap<String, Option<Ramp>>,
}

impl Calibration {
    pub fn new(preserve: bool) -> Calibration {
        Calibration {
            preserve,
            bases: HashMap::new(),
        }
    }

    /// With `false`, every ramp is built on identity, as if no display were calibrated.
    pub fn set_preserve(&mut self, on: bool) {
        if self.preserve != on {
            log::info!("preserve calibration: {on}");
        }
        self.preserve = on;
    }

    pub fn knows(&self, display: &str) -> bool {
        self.bases.contains_key(display)
    }

    /// Decide the base of displays seen for the first time from the ramps currently on them.
    pub fn capture(&mut self, displays: Vec<(String, Option<Ramp>)>) {
        let path = store_path();
        let mut store = load_store(&path);
        let before = store.clone();
        for (name, current) in displays {
            let base = store.base_for(&name, current.as_ref());
            match (&current, &base) {
                (None, _) => log::warn!("calibration: couldn't read {name}'s ramp; using neutral"),
                (Some(_), Some(_)) if before.displays.get(&name) == base.as_ref() => {
                    log::info!("calibration: {name}: keeping the saved curve")
                }
                (Some(_), Some(_)) => log::info!("calibration: {name}: captured a new curve"),
                (Some(c), None) if c.size().is_some_and(|n| c != &Ramp::identity(n)) => {
                    log::info!("calibration: {name}: found a plain tint, treating it as neutral")
                }
                (Some(_), None) => log::info!("calibration: {name}: no calibration curve"),
            }
            self.bases.insert(name, base);
        }
        if store != before
            && let Err(e) = save_store(&path, &store)
        {
            log::warn!("saving calibration curves: {e:#}");
        }
    }

    /// The ramp for `setting` on `display`, `size` entries per channel.
    pub fn ramp(&self, display: &str, setting: ColorSetting, size: usize) -> Ramp {
        let base = self
            .bases
            .get(display)
            .filter(|_| self.preserve)
            .and_then(Option::as_ref);
        Ramp::build_on(base, setting, size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A made-up calibration curve: a slight gamma tweak per channel, white point a bit cool.
    fn calibration(size: usize, tweak: f64) -> Ramp {
        let curve = |gamma: f64, top: f64| -> Vec<u16> {
            (0..size)
                .map(|i| {
                    let x = i as f64 / (size - 1) as f64;
                    (x.powf(gamma) * top * 65535.0).round() as u16
                })
                .collect()
        };
        Ramp {
            red: curve(1.0 + tweak, 0.97),
            green: curve(1.0 + tweak / 3.0, 1.0),
            blue: curve(1.0 - tweak, 0.99),
        }
    }

    fn warm(k: u32, brightness: f32) -> ColorSetting {
        ColorSetting {
            temperature: k,
            brightness,
        }
    }

    /// Drop the low byte, like a driver with 8-bit gamma tables.
    fn quantize(r: &Ramp) -> Ramp {
        let q = |c: &[u16]| c.iter().map(|v| v & 0xff00).collect();
        Ramp {
            red: q(&r.red),
            green: q(&r.green),
            blue: q(&r.blue),
        }
    }

    #[test]
    fn neutral_or_plain_tint_without_saved_state_is_identity() {
        for current in [
            Ramp::identity(256),
            Ramp::build(warm(3400, 1.0), 256),
            Ramp::build(warm(1200, 0.4), 1024),
            quantize(&Ramp::build(warm(2700, 0.8), 256)),
        ] {
            assert_eq!(decide(&current, None), Decision::Identity);
        }
    }

    #[test]
    fn a_real_curve_is_captured() {
        let cal = calibration(256, 0.08);
        assert_eq!(decide(&cal, None), Decision::Capture);
        // Even a mild one.
        assert_eq!(decide(&calibration(256, 0.03), None), Decision::Capture);
    }

    #[test]
    fn leftover_tint_on_the_saved_base_keeps_the_saved_base() {
        let saved = calibration(256, 0.08);
        for s in [
            warm(6500, 1.0),
            warm(3400, 1.0),
            warm(1900, 0.5),
            warm(1000, 1.0),
        ] {
            let current = Ramp::build_on(Some(&saved), s, 256);
            assert_eq!(decide(&current, Some(&saved)), Decision::KeepSaved, "{s:?}");
            assert_eq!(
                decide(&quantize(&current), Some(&saved)),
                Decision::KeepSaved
            );
        }
    }

    #[test]
    fn saved_base_matches_across_ramp_sizes() {
        let saved = calibration(256, 0.08);
        let current = Ramp::build_on(Some(&saved), warm(3000, 0.9), 1024);
        assert_eq!(decide(&current, Some(&saved)), Decision::KeepSaved);
    }

    #[test]
    fn a_different_curve_replaces_the_saved_one() {
        let saved = calibration(256, 0.08);
        let new = calibration(256, -0.06);
        assert_eq!(decide(&new, Some(&saved)), Decision::Capture);
    }

    #[test]
    fn calibration_wiped_since_last_run_is_identity() {
        let saved = calibration(256, 0.08);
        let current = Ramp::build(warm(3400, 1.0), 256);
        assert_eq!(decide(&current, Some(&saved)), Decision::Identity);
    }

    #[test]
    fn garbage_is_never_captured() {
        let bad = Ramp {
            red: vec![1, 2, 3],
            green: vec![1, 2],
            blue: vec![],
        };
        assert_eq!(decide(&bad, None), Decision::Identity);
        let black = Ramp {
            red: vec![0; 256],
            green: vec![0; 256],
            blue: vec![0; 256],
        };
        assert_eq!(decide(&black, None), Decision::Identity);
    }

    #[test]
    fn store_records_each_decision() {
        let mut store = Store::default();
        let cal = calibration(256, 0.08);
        assert_eq!(store.base_for("A", Some(&cal)), Some(cal.clone()));
        assert_eq!(store.displays.get("A"), Some(&cal));

        // Next start after a crash at 2700K: the tint is recognised, the base kept.
        let tinted = Ramp::build_on(Some(&cal), warm(2700, 1.0), 256);
        assert_eq!(store.base_for("A", Some(&tinted)), Some(cal.clone()));
        assert_eq!(store.displays.get("A"), Some(&cal));

        // Calibration removed: back to identity, and the entry goes.
        let plain = Ramp::build(warm(2700, 1.0), 256);
        assert_eq!(store.base_for("A", Some(&plain)), None);
        assert!(store.displays.is_empty());

        // Unreadable ramps leave the store alone.
        store.displays.insert("B".into(), cal.clone());
        assert_eq!(store.base_for("B", None), None);
        assert_eq!(store.displays.get("B"), Some(&cal));
    }

    #[test]
    fn store_round_trips_through_text() {
        let mut store = Store::default();
        store
            .displays
            .insert(r"\\.\DISPLAY1".into(), calibration(256, 0.08));
        store
            .displays
            .insert("HDMI-1".into(), calibration(1024, 0.05));
        let text = store.to_text().unwrap();
        assert_eq!(Store::parse(&text).unwrap(), store);
        assert_eq!(Store::parse("").unwrap(), Store::default());
    }

    #[test]
    fn preserve_off_builds_on_identity() {
        let mut c = Calibration::new(true);
        let cal = calibration(256, 0.08);
        c.bases.insert("A".into(), Some(cal.clone()));
        let s = warm(3400, 1.0);
        assert_eq!(c.ramp("A", s, 256), Ramp::build_on(Some(&cal), s, 256));
        assert_eq!(c.ramp("unknown", s, 256), Ramp::build(s, 256));
        c.set_preserve(false);
        assert_eq!(c.ramp("A", s, 256), Ramp::build(s, 256));
    }
}
