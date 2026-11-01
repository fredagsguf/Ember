//! Windows backend: GDI `SetDeviceGammaRamp` on every attached display.
//! The ramp found on each display at startup (`GetDeviceGammaRamp`) is kept as its calibration
//! base; see `calibration.rs`. The ramp is read again before every write, so a calibration curve
//! loaded after Ember started (Windows colour management, DisplayCAL's loader) is picked up.
//!
//! Windows refuses ramps that deviate "too far" from identity (roughly below ~3400-4000K)
//! unless HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\ICM\GdiIcmGammaRange (DWORD) = 256.
//! `ember` reports a hint when that happens; see README.

use super::Backend;
use super::calibration::Calibration;
use crate::color::{ColorSetting, Ramp};
use crate::config::{self, Display as DisplayConfig};
use anyhow::{Result, bail};
use std::ptr::null;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Gdi::{
    CreateDCW, DISPLAY_DEVICE_ATTACHED_TO_DESKTOP, DISPLAY_DEVICEW, DeleteDC, EnumDisplayDevicesW,
    HDC,
};
use windows_sys::Win32::UI::ColorSystem::{GetDeviceGammaRamp, SetDeviceGammaRamp};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

type GdiRamp = [u16; 3 * 256];

struct Display {
    /// Device name such as `\\.\DISPLAY1`; also the key for its saved calibration curve.
    name: String,
    /// The monitor's model as Windows names it, e.g. "DELL U2720Q", for the settings window.
    monitor: Option<String>,
    hdc: HDC,
}

pub struct Gdi {
    displays: Vec<Display>,
    calibration: Calibration,
    settings: Vec<DisplayConfig>,
    warned_range: bool,
}

fn wide_to_string(w: &[u16]) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

/// The model of the (first) monitor on display adapter output `device` (`\\.\DISPLAY1`).
fn monitor_name(device: &[u16]) -> Option<String> {
    let mut dd: DISPLAY_DEVICEW = unsafe { std::mem::zeroed() };
    dd.cb = std::mem::size_of::<DISPLAY_DEVICEW>() as u32;
    if unsafe { EnumDisplayDevicesW(device.as_ptr(), 0, &mut dd, 0) } == 0 {
        return None;
    }
    let name = wide_to_string(&dd.DeviceString);
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}

fn to_gdi(ramp: &Ramp) -> GdiRamp {
    let mut buf: GdiRamp = [0; 768];
    buf[..256].copy_from_slice(&ramp.red);
    buf[256..512].copy_from_slice(&ramp.green);
    buf[512..].copy_from_slice(&ramp.blue);
    buf
}

/// The ramp currently loaded on `hdc` (by Windows colour management, a calibration loader, or us).
fn read_ramp(hdc: HDC) -> Option<Ramp> {
    let mut buf: GdiRamp = [0; 768];
    if unsafe { GetDeviceGammaRamp(hdc, buf.as_mut_ptr().cast()) } == 0 {
        return None;
    }
    Some(Ramp {
        red: buf[..256].to_vec(),
        green: buf[256..512].to_vec(),
        blue: buf[512..].to_vec(),
    })
}

impl Gdi {
    pub fn open(preserve_calibration: bool) -> Result<Gdi> {
        let mut displays = Vec::new();
        let mut i = 0;
        loop {
            let mut dd: DISPLAY_DEVICEW = unsafe { std::mem::zeroed() };
            dd.cb = std::mem::size_of::<DISPLAY_DEVICEW>() as u32;
            if unsafe { EnumDisplayDevicesW(null(), i, &mut dd, 0) } == 0 {
                break;
            }
            i += 1;
            if dd.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP == 0 {
                continue;
            }
            let hdc = unsafe { CreateDCW(null(), dd.DeviceName.as_ptr(), null(), null()) };
            if hdc.is_null() {
                log::warn!("could not open display {}", wide_to_string(&dd.DeviceName));
                continue;
            }
            displays.push(Display {
                name: wide_to_string(&dd.DeviceName),
                monitor: monitor_name(&dd.DeviceName),
                hdc,
            });
        }
        if displays.is_empty() {
            bail!("no displays found");
        }
        log::info!("windows: controlling {} display(s)", displays.len());
        let mut calibration = Calibration::new(preserve_calibration);
        calibration.capture(
            displays
                .iter()
                .map(|d| (d.name.clone(), read_ramp(d.hdc)))
                .collect(),
        );
        Ok(Gdi {
            displays,
            calibration,
            settings: Vec::new(),
            warned_range: false,
        })
    }
}

impl Backend for Gdi {
    fn name(&self) -> &'static str {
        "windows-gdi"
    }

    fn apply(&mut self, setting: ColorSetting) -> Result<()> {
        let mut failed = Vec::new();
        for d in &self.displays {
            // Reading the ramp is a cheap local call, so do it every time (fade frames too).
            if let Some(current) = read_ramp(d.hdc) {
                self.calibration.check(&d.name, &current);
            }
            let ramp = if config::tints_display(&self.settings, &d.name) {
                self.calibration.ramp(&d.name, setting, 256)
            } else {
                self.calibration.untinted_ramp(&d.name, 256)
            };
            if unsafe { SetDeviceGammaRamp(d.hdc, to_gdi(&ramp).as_ptr().cast()) } == 0 {
                failed.push(d.name.clone());
            } else {
                self.calibration.wrote(&d.name, ramp);
            }
        }
        if !failed.is_empty() {
            if !self.warned_range {
                self.warned_range = true;
                log::warn!(
                    "Windows rejected the gamma ramp for {failed:?} at {}K. If this happens only at low \
                     temperatures, set the registry value GdiIcmGammaRange=256 (see README) and sign out/in.",
                    setting.temperature
                );
            }
            bail!("SetDeviceGammaRamp failed on {failed:?}");
        }
        Ok(())
    }

    fn restore(&mut self) -> Result<()> {
        self.apply(ColorSetting::NEUTRAL)
    }

    fn set_preserve_calibration(&mut self, on: bool) {
        self.calibration.set_preserve(on);
    }

    fn set_app_rule_active(&mut self, on: bool) {
        self.calibration.set_app_rule_active(on);
    }

    fn set_displays(&mut self, displays: &[DisplayConfig]) {
        self.settings = displays.to_vec();
    }

    fn displays(&mut self) -> Vec<(String, String)> {
        self.displays
            .iter()
            .map(|d| {
                let label = d.monitor.clone().unwrap_or_else(|| d.name.clone());
                (d.name.clone(), label)
            })
            .collect()
    }

    fn focused_pid(&mut self) -> Option<u32> {
        let hwnd: HWND = unsafe { GetForegroundWindow() };
        if hwnd.is_null() {
            return None;
        }
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        (pid != 0).then_some(pid)
    }
}

impl Drop for Gdi {
    fn drop(&mut self) {
        for d in &self.displays {
            unsafe { DeleteDC(d.hdc) };
        }
    }
}
