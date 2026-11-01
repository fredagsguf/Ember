//! Colour temperature maths: Kelvin -> RGB channel multipliers -> gamma ramps.

/// Neutral white point. At this temperature Ember applies no tint.
pub const NEUTRAL_K: u32 = 6500;
pub const MIN_K: u32 = 1000;
pub const MAX_K: u32 = 10000;

/// What should be on screen right now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorSetting {
    pub temperature: u32,
    /// 0.1 ..= 1.0
    pub brightness: f32,
}

impl ColorSetting {
    pub const NEUTRAL: ColorSetting = ColorSetting {
        temperature: NEUTRAL_K,
        brightness: 1.0,
    };

    pub fn clamped(self) -> Self {
        ColorSetting {
            temperature: self.temperature.clamp(MIN_K, MAX_K),
            brightness: self.brightness.clamp(0.1, 1.0),
        }
    }
}

/// Raw black-body approximation (Tanner Helland's curve fit), returns 0..=255-ish floats.
fn helland(kelvin: f64) -> [f64; 3] {
    let t = kelvin / 100.0;
    let r = if t <= 66.0 {
        255.0
    } else {
        329.698_727_446 * (t - 60.0).powf(-0.133_204_759_2)
    };
    let g = if t <= 66.0 {
        99.470_802_586_1 * t.ln() - 161.119_568_166_1
    } else {
        288.122_169_528_3 * (t - 60.0).powf(-0.075_514_849_2)
    };
    let b = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        138.517_731_223_1 * (t - 10.0).ln() - 305.044_792_730_7
    };
    [
        r.clamp(0.0, 255.0),
        g.clamp(0.0, 255.0),
        b.clamp(0.0, 255.0),
    ]
}

/// Per-channel multipliers in 0..=1, normalised so that 6500K is exactly (1, 1, 1).
pub fn temperature_to_rgb(kelvin: u32) -> [f64; 3] {
    let k = kelvin.clamp(MIN_K, MAX_K) as f64;
    if (k - NEUTRAL_K as f64).abs() < f64::EPSILON {
        return [1.0; 3];
    }
    let white = helland(NEUTRAL_K as f64);
    let raw = helland(k);
    let mut m = [raw[0] / white[0], raw[1] / white[1], raw[2] / white[2]];
    // Above 6500K blue can exceed 1.0; scale everything down so nothing clips.
    let max = m.iter().cloned().fold(0.0_f64, f64::max);
    if max > 1.0 {
        m.iter_mut().for_each(|c| *c /= max);
    }
    m
}

/// A gamma ramp with `size` entries per channel (16-bit values).
#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Ramp {
    pub red: Vec<u16>,
    pub green: Vec<u16>,
    pub blue: Vec<u16>,
}

impl Ramp {
    /// The tint for `setting` on top of a neutral (identity) ramp.
    pub fn build(setting: ColorSetting, size: usize) -> Ramp {
        let identity: Vec<f64> = (0..size)
            .map(|i| {
                if size > 1 {
                    i as f64 / (size - 1) as f64
                } else {
                    1.0
                }
            })
            .collect();
        tint([&identity, &identity, &identity], setting)
    }

    /// The tint for `setting` on top of `base` (the display's calibration curve), with `size`
    /// entries per channel: `base[i] × multiplier × brightness`. `None` means an identity base
    /// and gives exactly [`Ramp::build`].
    pub fn build_on(base: Option<&Ramp>, setting: ColorSetting, size: usize) -> Ramp {
        let Some(base) = base else {
            return Ramp::build(setting, size);
        };
        let [r, g, b] = base.channels().map(|c| resample(c, size));
        tint([&r, &g, &b], setting)
    }

    /// The neutral ramp with `size` entries.
    pub fn identity(size: usize) -> Ramp {
        Ramp::build(ColorSetting::NEUTRAL, size)
    }

    /// Entries per channel, or `None` if the channels disagree or are too short to be a curve.
    pub fn size(&self) -> Option<usize> {
        let n = self.red.len();
        (n >= 2 && self.green.len() == n && self.blue.len() == n).then_some(n)
    }

    pub fn channels(&self) -> [&[u16]; 3] {
        [&self.red, &self.green, &self.blue]
    }
}

/// Scale each channel's curve (values in 0..=1) by the setting's multiplier and brightness.
fn tint(curves: [&[f64]; 3], setting: ColorSetting) -> Ramp {
    let s = setting.clamped();
    let mult = temperature_to_rgb(s.temperature);
    let b = s.brightness as f64;
    let channel = |curve: &[f64], m: f64| -> Vec<u16> {
        curve
            .iter()
            .map(|&x| (x * m * b * 65535.0).round().clamp(0.0, 65535.0) as u16)
            .collect()
    };
    Ramp {
        red: channel(curves[0], mult[0]),
        green: channel(curves[1], mult[1]),
        blue: channel(curves[2], mult[2]),
    }
}

/// `channel` stretched or squeezed to `size` entries by linear interpolation, as 0..=1 values.
/// Gamma ramp sizes differ between displays (X11 CRTCs report anything from 256 to 4096+).
pub fn resample(channel: &[u16], size: usize) -> Vec<f64> {
    let v = |i: usize| channel[i] as f64 / 65535.0;
    let n = channel.len();
    if n == size {
        return (0..n).map(v).collect();
    }
    if n == 0 {
        return vec![0.0; size];
    }
    if n == 1 || size == 1 {
        return vec![v(n - 1); size];
    }
    (0..size)
        .map(|i| {
            let pos = i as f64 * (n - 1) as f64 / (size - 1) as f64;
            let lo = (pos.floor() as usize).min(n - 2);
            let t = pos - lo as f64;
            v(lo) * (1.0 - t) + v(lo + 1) * t
        })
        .collect()
}

/// Linear interpolation between two settings, `t` in 0..=1.
pub fn lerp(a: ColorSetting, b: ColorSetting, t: f64) -> ColorSetting {
    let t = t.clamp(0.0, 1.0);
    ColorSetting {
        temperature: (a.temperature as f64 + (b.temperature as f64 - a.temperature as f64) * t)
            .round() as u32,
        brightness: (a.brightness as f64 + (b.brightness as f64 - a.brightness as f64) * t) as f32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_is_identity() {
        assert_eq!(temperature_to_rgb(6500), [1.0, 1.0, 1.0]);
        let r = Ramp::build(ColorSetting::NEUTRAL, 256);
        assert_eq!(r.red[0], 0);
        assert_eq!(r.red[255], 65535);
        assert_eq!(r.red, r.blue);
    }

    #[test]
    fn warm_reduces_blue_more_than_green() {
        let [r, g, b] = temperature_to_rgb(3400);
        assert!((r - 1.0).abs() < 1e-9);
        assert!(g < 1.0 && b < g, "got {r} {g} {b}");
    }

    #[test]
    fn cool_reduces_red() {
        let [r, _g, b] = temperature_to_rgb(9000);
        assert!(r < b);
        assert!((b - 1.0).abs() < 1e-9);
    }

    #[test]
    fn ramp_is_monotonic() {
        let r = Ramp::build(
            ColorSetting {
                temperature: 2700,
                brightness: 0.8,
            },
            1024,
        );
        assert!(r.green.windows(2).all(|w| w[0] <= w[1]));
        assert!(*r.red.last().unwrap() <= (0.8 * 65535.0) as u16 + 1);
    }

    /// A made-up calibration curve: a slight gamma tweak per channel, white point a bit cool.
    fn calibration(size: usize) -> Ramp {
        let curve = |gamma: f64, top: f64| -> Vec<u16> {
            (0..size)
                .map(|i| {
                    let x = i as f64 / (size - 1) as f64;
                    (x.powf(gamma) * top * 65535.0).round() as u16
                })
                .collect()
        };
        Ramp {
            red: curve(1.08, 0.97),
            green: curve(1.02, 1.0),
            blue: curve(0.95, 0.99),
        }
    }

    const SETTINGS: [ColorSetting; 4] = [
        ColorSetting::NEUTRAL,
        ColorSetting {
            temperature: 3400,
            brightness: 1.0,
        },
        ColorSetting {
            temperature: 1900,
            brightness: 0.55,
        },
        ColorSetting {
            temperature: 9000,
            brightness: 0.8,
        },
    ];

    #[test]
    fn identity_base_gives_the_plain_ramp() {
        for s in SETTINGS {
            assert_eq!(Ramp::build_on(None, s, 1024), Ramp::build(s, 1024));
            // At 256 entries the identity ramp is exact (i × 257), so the result is too.
            let id = Ramp::identity(256);
            assert_eq!(Ramp::build_on(Some(&id), s, 256), Ramp::build(s, 256));
        }
    }

    #[test]
    fn base_is_scaled_per_channel() {
        let base = calibration(256);
        for s in SETTINGS {
            let out = Ramp::build_on(Some(&base), s, 256);
            let m = temperature_to_rgb(s.temperature);
            let b = s.brightness as f64;
            for (c, (o, src)) in out.channels().iter().zip(base.channels()).enumerate() {
                for (&got, &want) in o.iter().zip(src) {
                    let want = want as f64 * m[c] * b;
                    assert!((got as f64 - want).abs() <= 0.51, "{got} vs {want}");
                }
            }
        }
    }

    #[test]
    fn neutral_on_a_base_is_the_base() {
        let base = calibration(1024);
        assert_eq!(
            Ramp::build_on(Some(&base), ColorSetting::NEUTRAL, 1024),
            base
        );
    }

    #[test]
    fn base_result_stays_in_range() {
        let full = Ramp {
            red: vec![65535; 256],
            green: vec![65535; 256],
            blue: vec![65535; 256],
        };
        for s in SETTINGS {
            let out = Ramp::build_on(Some(&full), s, 256);
            let top = (s.brightness as f64 * 65535.0).round() as u16;
            assert!(out.channels().iter().all(|c| c.iter().all(|&v| v <= top)));
        }
    }

    #[test]
    fn base_is_resampled_to_the_display_size() {
        let base = calibration(256);
        let s = SETTINGS[1];
        let m = temperature_to_rgb(s.temperature);
        for size in [16, 1024, 4096] {
            let out = Ramp::build_on(Some(&base), s, size);
            assert_eq!(out.size(), Some(size));
            assert!(out.green.windows(2).all(|w| w[0] <= w[1]));
            // The ends land exactly on the base's ends.
            assert_eq!(out.red[0], 0);
            let want = (base.blue[255] as f64 * m[2]).round() as u16;
            assert!(out.blue[size - 1].abs_diff(want) <= 1);
        }
        // Resampling identity gives identity, give or take rounding.
        let id = resample(&Ramp::identity(256).red, 1024);
        for (a, &b) in id.iter().zip(&Ramp::identity(1024).red) {
            assert!(((a * 65535.0).round() as u16).abs_diff(b) <= 1);
        }
    }
}
