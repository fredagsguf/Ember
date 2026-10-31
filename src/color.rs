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
#[derive(Debug, Clone, PartialEq)]
pub struct Ramp {
    pub red: Vec<u16>,
    pub green: Vec<u16>,
    pub blue: Vec<u16>,
}

impl Ramp {
    pub fn build(setting: ColorSetting, size: usize) -> Ramp {
        let s = setting.clamped();
        let mult = temperature_to_rgb(s.temperature);
        let b = s.brightness as f64;
        let channel = |m: f64| -> Vec<u16> {
            (0..size)
                .map(|i| {
                    let x = if size > 1 {
                        i as f64 / (size - 1) as f64
                    } else {
                        1.0
                    };
                    (x * m * b * 65535.0).round().clamp(0.0, 65535.0) as u16
                })
                .collect()
        };
        Ramp {
            red: channel(mult[0]),
            green: channel(mult[1]),
            blue: channel(mult[2]),
        }
    }
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
}
