//! Decides the "normal" (no rule active) colour setting for a given time.

use crate::color::{ColorSetting, lerp};
use crate::config::{Config, ScheduleMode, parse_hhmm};
use chrono::{DateTime, Local, Timelike, Utc};

/// Sun elevation (degrees) at/above which it's full day, and at/below which it's full night.
const SOLAR_DAY_ELEVATION: f64 = 3.0;
const SOLAR_NIGHT_ELEVATION: f64 = -6.0;

/// 0.0 = full day, 1.0 = full night.
pub fn night_factor(cfg: &Config, now: DateTime<Local>) -> f64 {
    let s = &cfg.schedule;
    match s.mode {
        ScheduleMode::AlwaysNight => 1.0,
        ScheduleMode::Solar => {
            let el = crate::solar::elevation(
                now.with_timezone(&Utc),
                s.latitude.unwrap_or(0.0),
                s.longitude.unwrap_or(0.0),
            );
            ((SOLAR_DAY_ELEVATION - el) / (SOLAR_DAY_ELEVATION - SOLAR_NIGHT_ELEVATION))
                .clamp(0.0, 1.0)
        }
        ScheduleMode::Fixed => {
            let (Ok(start), Ok(end)) = (parse_hhmm(&s.night_start), parse_hhmm(&s.night_end))
            else {
                return 0.0;
            };
            let mins = |t: chrono::NaiveTime| (t.hour() * 60 + t.minute()) as f64;
            let now_m = now.hour() as f64 * 60.0 + now.minute() as f64 + now.second() as f64 / 60.0;
            fixed_factor(mins(start), mins(end), now_m, s.transition_minutes as f64)
        }
    }
}

/// Night begins fading in at `start`, reaching full night after `transition` minutes;
/// at `end` it starts fading back out to day over `transition` minutes.
fn fixed_factor(start: f64, end: f64, now: f64, transition: f64) -> f64 {
    const DAY: f64 = 1440.0;
    let night_len = (end - start).rem_euclid(DAY);
    let since_start = (now - start).rem_euclid(DAY);
    let t = transition.max(0.0);
    let ramp = |x: f64| {
        if t == 0.0 {
            1.0
        } else {
            (x / t).clamp(0.0, 1.0)
        }
    };
    if since_start < night_len {
        ramp(since_start)
    } else {
        // Note: if the fade-out overlaps the next fade-in, fade-in wins (handled above).
        1.0 - ramp(since_start - night_len)
    }
}

pub fn scheduled_setting(cfg: &Config, now: DateTime<Local>) -> ColorSetting {
    let c = &cfg.color;
    let day = ColorSetting {
        temperature: c.day_temperature,
        brightness: c.day_brightness,
    };
    let night = ColorSetting {
        temperature: c.night_temperature,
        brightness: c.night_brightness,
    };
    lerp(day, night, night_factor(cfg, now))
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: f64 = 60.0;

    #[test]
    fn fixed_overnight_window() {
        let (s, e, t) = (21.0 * H, 7.0 * H, 60.0);
        assert_eq!(fixed_factor(s, e, 12.0 * H, t), 0.0);
        assert_eq!(fixed_factor(s, e, 21.0 * H, t), 0.0);
        assert!((fixed_factor(s, e, 21.5 * H, t) - 0.5).abs() < 1e-9);
        assert_eq!(fixed_factor(s, e, 23.0 * H, t), 1.0);
        assert_eq!(fixed_factor(s, e, 3.0 * H, t), 1.0);
        assert_eq!(fixed_factor(s, e, 7.0 * H, t), 1.0);
        assert!((fixed_factor(s, e, 7.5 * H, t) - 0.5).abs() < 1e-9);
        assert_eq!(fixed_factor(s, e, 8.0 * H, t), 0.0);
    }

    #[test]
    fn fixed_instant_transition() {
        assert_eq!(fixed_factor(21.0 * H, 7.0 * H, 21.0 * H, 0.0), 1.0);
        assert_eq!(fixed_factor(21.0 * H, 7.0 * H, 7.0 * H, 0.0), 0.0);
    }

    #[test]
    fn always_night() {
        let mut cfg = Config::default();
        cfg.schedule.mode = ScheduleMode::AlwaysNight;
        assert_eq!(
            scheduled_setting(&cfg, Local::now()).temperature,
            cfg.color.night_temperature
        );
    }
}
