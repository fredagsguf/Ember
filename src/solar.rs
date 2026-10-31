//! Solar elevation (NOAA algorithm). Accurate to well under a degree, which is
//! plenty for deciding "is it day or night".

use chrono::{DateTime, Timelike, Utc};

/// Elevation of the sun above the horizon in degrees, for the given instant and location.
/// `lat` is north-positive, `lon` is east-positive.
pub fn elevation(at: DateTime<Utc>, lat: f64, lon: f64) -> f64 {
    let jd = at.timestamp() as f64 / 86400.0 + 2_440_587.5;
    let jc = (jd - 2_451_545.0) / 36525.0;

    let l0 = (280.46646 + jc * (36000.76983 + jc * 0.0003032)).rem_euclid(360.0);
    let m = 357.52911 + jc * (35999.05029 - 0.0001537 * jc);
    let e = 0.016708634 - jc * (0.000042037 + 0.0000001267 * jc);
    let mr = m.to_radians();
    let c = mr.sin() * (1.914602 - jc * (0.004817 + 0.000014 * jc))
        + (2.0 * mr).sin() * (0.019993 - 0.000101 * jc)
        + (3.0 * mr).sin() * 0.000289;
    let true_long = l0 + c;
    let omega = (125.04 - 1934.136 * jc).to_radians();
    let app_long = (true_long - 0.00569 - 0.00478 * omega.sin()).to_radians();
    let mean_obliq =
        23.0 + (26.0 + (21.448 - jc * (46.815 + jc * (0.00059 - jc * 0.001813))) / 60.0) / 60.0;
    let obliq = (mean_obliq + 0.00256 * omega.cos()).to_radians();
    let decl = (obliq.sin() * app_long.sin()).asin();

    let y = (obliq / 2.0).tan().powi(2);
    let l0r = l0.to_radians();
    let eq_time = 4.0
        * (y * (2.0 * l0r).sin() - 2.0 * e * mr.sin() + 4.0 * e * y * mr.sin() * (2.0 * l0r).cos()
            - 0.5 * y * y * (4.0 * l0r).sin()
            - 1.25 * e * e * (2.0 * mr).sin())
        .to_degrees();

    let minutes = at.hour() as f64 * 60.0 + at.minute() as f64 + at.second() as f64 / 60.0;
    let tst = (minutes + eq_time + 4.0 * lon).rem_euclid(1440.0);
    let hour_angle = if tst / 4.0 < 0.0 {
        tst / 4.0 + 180.0
    } else {
        tst / 4.0 - 180.0
    }
    .to_radians();

    let latr = lat.to_radians();
    let cos_zenith =
        (latr.sin() * decl.sin() + latr.cos() * decl.cos() * hour_angle.cos()).clamp(-1.0, 1.0);
    90.0 - cos_zenith.acos().to_degrees()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn equator_equinox_noon_is_overhead() {
        let t = Utc.with_ymd_and_hms(2024, 3, 20, 12, 7, 0).unwrap();
        assert!(elevation(t, 0.0, 0.0) > 88.0);
    }

    #[test]
    fn midnight_is_dark() {
        let t = Utc.with_ymd_and_hms(2024, 6, 1, 0, 0, 0).unwrap();
        assert!(elevation(t, 51.5, 0.0) < -10.0);
    }

    #[test]
    fn oslo_winter_noon_is_low() {
        // Oslo 59.91N 10.75E, winter solstice: max elevation ~6.6 degrees.
        let t = Utc.with_ymd_and_hms(2024, 12, 21, 11, 20, 0).unwrap();
        let el = elevation(t, 59.91, 10.75);
        assert!((5.5..7.5).contains(&el), "got {el}");
    }
}
