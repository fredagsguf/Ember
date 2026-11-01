//! Decides the "normal" (no rule active) colour setting for a given time.

use crate::color::{ColorSetting, lerp};
use crate::config::{Config, Schedule, ScheduleMode, parse_hhmm};
use chrono::{DateTime, Datelike, Local, TimeDelta, Timelike, Utc};

/// Sun elevation (degrees) at/above which it's full day, and at/below which it's full night.
const SOLAR_DAY_ELEVATION: f64 = 3.0;
const SOLAR_NIGHT_ELEVATION: f64 = -6.0;

/// Bedtime ends at noon, well after any morning.
const BEDTIME_END_MINUTES: f64 = 12.0 * 60.0;
/// How long the step into bedtime takes.
const BEDTIME_FADE_MINUTES: f64 = 30.0;
const DAY: f64 = 1440.0;

/// 0.0 = full day, 1.0 = full night.
pub fn night_factor(cfg: &Config, now: DateTime<Local>) -> f64 {
    let s = &cfg.schedule;
    match s.mode {
        ScheduleMode::AlwaysNight => 1.0,
        ScheduleMode::Solar => {
            let el = |t: DateTime<Utc>| {
                crate::solar::elevation(t, s.latitude.unwrap_or(0.0), s.longitude.unwrap_or(0.0))
            };
            let now = now.with_timezone(&Utc);
            // Shift the evening by the sunset offset and the morning by the sunrise offset: look
            // at where the sun was that many minutes ago.
            let setting = el(now + TimeDelta::minutes(1)) < el(now);
            let offset = if setting {
                s.sunset_offset_minutes
            } else {
                s.sunrise_offset_minutes
            };
            let el = el(now - TimeDelta::minutes(offset as i64));
            ((SOLAR_DAY_ELEVATION - el) / (SOLAR_DAY_ELEVATION - SOLAR_NIGHT_ELEVATION))
                .clamp(0.0, 1.0)
        }
        ScheduleMode::Fixed => {
            // Last night may still be going on (or fading out); tonight may have begun.
            let today = now.date_naive();
            let now_m = minutes_of_day(now);
            [1, 0]
                .into_iter()
                .filter_map(|back| {
                    let day = today - TimeDelta::days(back);
                    let (start, end) = s.times_for(day.weekday());
                    let (start, end) = (parse_hhmm(start).ok()?, parse_hhmm(end).ok()?);
                    let len = (minutes(end) - minutes(start)).rem_euclid(DAY);
                    let start = minutes(start) - DAY * back as f64;
                    Some(window_factor(
                        start,
                        len,
                        now_m,
                        s.transition_minutes as f64,
                    ))
                })
                .fold(0.0, f64::max)
        }
    }
}

fn minutes(t: chrono::NaiveTime) -> f64 {
    (t.hour() * 60 + t.minute()) as f64
}

fn minutes_of_day(now: DateTime<Local>) -> f64 {
    minutes(now.time()) + now.second() as f64 / 60.0
}

/// One night that starts at minute `start` (relative to today's midnight, so last night's is
/// negative) and lasts `len` minutes: it fades in over `transition` minutes from `start`, and
/// at its end starts fading back out over `transition` minutes.
fn window_factor(start: f64, len: f64, now: f64, transition: f64) -> f64 {
    let t = transition.max(0.0);
    let ramp = |x: f64| {
        if t == 0.0 {
            1.0
        } else {
            (x / t).clamp(0.0, 1.0)
        }
    };
    let since = now - start;
    if since < 0.0 {
        0.0
    } else if since < len {
        ramp(since)
    } else {
        1.0 - ramp(since - len)
    }
}

/// The same night every day, from `start` to `end` (minutes of the day).
#[cfg(test)]
fn fixed_factor(start: f64, end: f64, now: f64, transition: f64) -> f64 {
    let len = (end - start).rem_euclid(DAY);
    let last_night = window_factor(start - DAY, len, now, transition);
    last_night.max(window_factor(start, len, now, transition))
}

/// 0.0 = plain night, 1.0 = the bedtime colour. Bedtime runs from `schedule.bedtime` to noon.
pub fn bedtime_factor(s: &Schedule, now: DateTime<Local>) -> f64 {
    let Ok(start) = parse_hhmm(&s.bedtime) else {
        return 0.0;
    };
    let start = minutes(start);
    let len = (BEDTIME_END_MINUTES - start).rem_euclid(DAY);
    let now = minutes_of_day(now);
    window_factor(start - DAY, len, now, BEDTIME_FADE_MINUTES).max(window_factor(
        start,
        len,
        now,
        BEDTIME_FADE_MINUTES,
    ))
}

/// For solar mode: today's local times when the evening fade starts/ends and the morning
/// fade starts/ends. `None` for a phase that doesn't happen (polar day/night).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SolarTimes {
    pub evening: Option<(chrono::NaiveTime, chrono::NaiveTime)>,
    pub morning: Option<(chrono::NaiveTime, chrono::NaiveTime)>,
}

/// [`solar_times`] for the schedule's location, moved by its sunset and sunrise offsets.
pub fn schedule_solar_times(s: &Schedule, day: chrono::NaiveDate) -> SolarTimes {
    let t = solar_times(s.latitude.unwrap_or(0.0), s.longitude.unwrap_or(0.0), day);
    let shift = |p: Option<(chrono::NaiveTime, chrono::NaiveTime)>, m: i32| {
        let d = TimeDelta::minutes(m as i64);
        p.map(|(a, b)| (a.overflowing_add_signed(d).0, b.overflowing_add_signed(d).0))
    };
    SolarTimes {
        evening: shift(t.evening, s.sunset_offset_minutes),
        morning: shift(t.morning, s.sunrise_offset_minutes),
    }
}

pub fn solar_times(lat: f64, lon: f64, day: chrono::NaiveDate) -> SolarTimes {
    use chrono::TimeZone;
    let at = |m: u32| {
        let t = day.and_hms_opt(m / 60, m % 60, 0).expect("valid time");
        let local = Local
            .from_local_datetime(&t)
            .earliest()
            .unwrap_or_else(|| Local.from_utc_datetime(&t));
        (
            t.time(),
            crate::solar::elevation(local.with_timezone(&Utc), lat, lon),
        )
    };
    let samples: Vec<_> = (0..1440).map(at).collect();
    let cross = |threshold: f64, falling: bool| {
        samples.windows(2).find_map(|w| {
            let (a, b) = (w[0].1, w[1].1);
            let hit = if falling {
                a >= threshold && b < threshold
            } else {
                a < threshold && b >= threshold
            };
            hit.then_some(w[1].0)
        })
    };
    let pair = |a: Option<chrono::NaiveTime>, b: Option<chrono::NaiveTime>| a.zip(b);
    SolarTimes {
        evening: pair(
            cross(SOLAR_DAY_ELEVATION, true),
            cross(SOLAR_NIGHT_ELEVATION, true),
        ),
        morning: pair(
            cross(SOLAR_NIGHT_ELEVATION, false),
            cross(SOLAR_DAY_ELEVATION, false),
        ),
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
    let bedtime = ColorSetting {
        temperature: c.bedtime_temperature,
        brightness: c.bedtime_brightness,
    };
    let night = lerp(night, bedtime, bedtime_factor(&cfg.schedule, now));
    lerp(day, night, night_factor(cfg, now))
}

/// The next thing the schedule does, for the tray's tooltip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NextChange {
    /// A fade towards warmer (or dimmer) starts at this time.
    WarmsAt(DateTime<Local>),
    /// A fade back towards day starts at this time.
    CoolsAt(DateTime<Local>),
    /// Fading warmer right now, done at this time.
    WarmingUntil(DateTime<Local>),
    /// Fading back to day right now, done at this time.
    CoolingUntil(DateTime<Local>),
}

/// When the scheduled colour next starts or stops changing, looking a day and a half ahead
/// a minute at a time. `None` if it never changes (always night without bedtime).
pub fn next_change(cfg: &Config, now: DateTime<Local>) -> Option<NextChange> {
    const STEP: TimeDelta = TimeDelta::minutes(1);
    let at = |t: DateTime<Local>| scheduled_setting(cfg, t);
    // Warmer or dimmer counts as warming.
    let warms = |a: ColorSetting, b: ColorSetting| {
        b.temperature < a.temperature
            || (b.temperature == a.temperature && b.brightness < a.brightness)
    };
    let (mut t, mut prev) = (now, at(now));
    let next = at(now + STEP);
    let changing = next != prev;
    let warming = warms(prev, next);
    for _ in 0..36 * 60 {
        let after = at(t + STEP);
        if changing && after == prev {
            return Some(if warming {
                NextChange::WarmingUntil(t)
            } else {
                NextChange::CoolingUntil(t)
            });
        }
        if !changing && after != prev {
            return Some(if warms(prev, after) {
                NextChange::WarmsAt(t)
            } else {
                NextChange::CoolsAt(t)
            });
        }
        (t, prev) = (t + STEP, after);
    }
    None
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

    use crate::config::DayTimes;
    use chrono::{TimeZone, Weekday};

    /// Local time on Friday 2026-11-06 (no DST change nearby), `day_offset` days later.
    fn friday(day_offset: u32, h: u32, m: u32) -> DateTime<Local> {
        let t = Local
            .with_ymd_and_hms(2026, 11, 6 + day_offset, h, m, 0)
            .unwrap();
        if day_offset == 0 {
            assert_eq!(t.weekday(), Weekday::Fri);
        }
        t
    }

    fn fixed(start: &str, end: &str) -> Config {
        let mut cfg = Config::default();
        cfg.schedule.mode = ScheduleMode::Fixed;
        cfg.schedule.night_start = start.into();
        cfg.schedule.night_end = end.into();
        cfg.schedule.transition_minutes = 30;
        cfg
    }

    #[test]
    fn a_later_friday_night_runs_into_saturday_morning() {
        let mut cfg = fixed("21:00", "07:00");
        cfg.schedule.days.fri = Some(DayTimes {
            night_start: "23:00".into(),
            night_end: "09:00".into(),
        });
        // Thursday night ends as usual on Friday morning.
        assert_eq!(night_factor(&cfg, friday(0, 6, 0)), 1.0);
        assert_eq!(night_factor(&cfg, friday(0, 8, 0)), 0.0);
        // Friday evening starts later...
        assert_eq!(night_factor(&cfg, friday(0, 22, 0)), 0.0);
        assert_eq!(night_factor(&cfg, friday(0, 23, 30)), 1.0);
        // ...and lasts until 09:00 on Saturday.
        assert_eq!(night_factor(&cfg, friday(1, 8, 0)), 1.0);
        assert_eq!(night_factor(&cfg, friday(1, 9, 30)), 0.0);
        // Saturday evening is back to normal.
        assert_eq!(night_factor(&cfg, friday(1, 21, 30)), 1.0);
    }

    #[test]
    fn bedtime_deepens_the_night_until_noon() {
        let mut cfg = fixed("21:00", "07:00");
        cfg.schedule.bedtime = "23:00".into();
        cfg.color.night_temperature = 3400;
        cfg.color.bedtime_temperature = 2700;
        assert_eq!(scheduled_setting(&cfg, friday(0, 22, 0)).temperature, 3400);
        assert_eq!(scheduled_setting(&cfg, friday(0, 23, 15)).temperature, 3050);
        assert_eq!(scheduled_setting(&cfg, friday(1, 2, 0)).temperature, 2700);
        // The morning fade goes from the bedtime colour to day.
        assert_eq!(scheduled_setting(&cfg, friday(1, 7, 15)).temperature, 4600);
        assert_eq!(scheduled_setting(&cfg, friday(1, 9, 0)).temperature, 6500);
        // No bedtime: plain night.
        cfg.schedule.bedtime.clear();
        assert_eq!(scheduled_setting(&cfg, friday(1, 2, 0)).temperature, 3400);
    }

    #[test]
    fn sunset_offset_starts_the_evening_later() {
        let mut cfg = Config::default();
        cfg.schedule.mode = ScheduleMode::Solar;
        cfg.schedule.latitude = Some(55.68);
        cfg.schedule.longitude = Some(12.57);
        let day = friday(0, 12, 0).date_naive();
        let plain = schedule_solar_times(&cfg.schedule, day).evening.unwrap().0;
        cfg.schedule.sunset_offset_minutes = 30;
        let later = schedule_solar_times(&cfg.schedule, day).evening.unwrap().0;
        assert_eq!(later - plain, TimeDelta::minutes(30));
        // At the old start the evening hasn't begun yet; 30 minutes later it has.
        let at = |t: chrono::NaiveTime| Local.from_local_datetime(&day.and_time(t)).unwrap();
        assert_eq!(night_factor(&cfg, at(plain + TimeDelta::minutes(2))), 0.0);
        assert!(night_factor(&cfg, at(later + TimeDelta::minutes(5))) > 0.0);
    }

    #[test]
    fn next_change_is_found() {
        let cfg = fixed("21:00", "07:00");
        assert_eq!(
            next_change(&cfg, friday(0, 12, 0)),
            Some(NextChange::WarmsAt(friday(0, 21, 0)))
        );
        assert_eq!(
            next_change(&cfg, friday(0, 21, 10)),
            Some(NextChange::WarmingUntil(friday(0, 21, 30)))
        );
        assert_eq!(
            next_change(&cfg, friday(0, 23, 0)),
            Some(NextChange::CoolsAt(friday(1, 7, 0)))
        );
        let mut always = Config::default();
        always.schedule.mode = ScheduleMode::AlwaysNight;
        assert_eq!(next_change(&always, friday(0, 12, 0)), None);
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
