//! Windows backend: GDI `SetDeviceGammaRamp` on every attached display.
//!
//! Windows refuses ramps that deviate "too far" from identity (roughly below ~3400-4000K)
//! unless HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\ICM\GdiIcmGammaRange (DWORD) = 256.
//! `ember` reports a hint when that happens; see README.

use super::Backend;
use crate::color::{ColorSetting, Ramp};
use anyhow::{Result, bail};
use std::ptr::null;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Gdi::{
    CreateDCW, DISPLAY_DEVICE_ATTACHED_TO_DESKTOP, DISPLAY_DEVICEW, DeleteDC, EnumDisplayDevicesW,
    HDC,
};
use windows_sys::Win32::UI::ColorSystem::SetDeviceGammaRamp;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

type GdiRamp = [u16; 3 * 256];

struct Display {
    name: String,
    hdc: HDC,
}

pub struct Gdi {
    displays: Vec<Display>,
    warned_range: bool,
}

fn wide_to_string(w: &[u16]) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

impl Gdi {
    pub fn open() -> Result<Gdi> {
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
                hdc,
            });
        }
        if displays.is_empty() {
            bail!("no displays found");
        }
        log::info!("windows: controlling {} display(s)", displays.len());
        Ok(Gdi {
            displays,
            warned_range: false,
        })
    }
}

impl Backend for Gdi {
    fn name(&self) -> &'static str {
        "windows-gdi"
    }

    fn apply(&mut self, setting: ColorSetting) -> Result<()> {
        let ramp = Ramp::build(setting, 256);
        let mut buf: GdiRamp = [0; 768];
        buf[..256].copy_from_slice(&ramp.red);
        buf[256..512].copy_from_slice(&ramp.green);
        buf[512..].copy_from_slice(&ramp.blue);
        let mut failed = Vec::new();
        for d in &self.displays {
            if unsafe { SetDeviceGammaRamp(d.hdc, buf.as_ptr().cast()) } == 0 {
                failed.push(d.name.clone());
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
