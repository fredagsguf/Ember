//! Windows backend: GDI `SetDeviceGammaRamp` on every attached display.
//! The ramp found on each display at startup (`GetDeviceGammaRamp`) is kept as its calibration
//! base; see `calibration.rs`. The ramp is read again before every write, so a calibration curve
//! loaded after Ember started (Windows colour management, DisplayCAL's loader) is picked up.
//!
//! Windows refuses ramps that deviate "too far" from identity (roughly below ~3400-4000K)
//! unless HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\ICM\GdiIcmGammaRange (DWORD) = 256.
//! When that happens the tray and the settings window offer to set it
//! ([`allow_full_gamma_range`]); it takes effect after signing out and back in.

use super::calibration::Calibration;
use super::{Backend, FocusedWindow};
use crate::color::{ColorSetting, Ramp};
use crate::config::{self, Display as DisplayConfig};
use anyhow::{Result, bail};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_CANCELLED, ERROR_SUCCESS, GetLastError, HWND, RECT,
};
use windows_sys::Win32::Graphics::Gdi::{
    CreateDCW, DISPLAY_DEVICE_ATTACHED_TO_DESKTOP, DISPLAY_DEVICEW, DeleteDC, EnumDisplayDevicesW,
    GetMonitorInfoW, HDC, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW};
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
use windows_sys::Win32::UI::ColorSystem::{GetDeviceGammaRamp, SetDeviceGammaRamp};
use windows_sys::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetForegroundWindow, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId,
    SW_HIDE,
};

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
    /// The last warm colour was refused, and no warm colour has worked since.
    refused_warm: bool,
}

const ICM_KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\ICM";
const GAMMA_RANGE: &str = "GdiIcmGammaRange";
/// The `GdiIcmGammaRange` value that lifts the limit (f.lux sets the same).
const FULL_RANGE: u32 = 256;
/// Below this, a ramp that Windows accepts shows the limit isn't in the way.
const WARM_K: u32 = 4000;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Is `GdiIcmGammaRange` set so that apps may make the screen warmer than about 4000K?
/// (It takes effect only after signing out and back in.)
pub fn full_gamma_range_allowed() -> bool {
    let (key, value) = (wide(ICM_KEY), wide(GAMMA_RANGE));
    let mut data = 0u32;
    let mut size = size_of::<u32>() as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            (&mut data as *mut u32).cast(),
            &mut size,
        )
    };
    r == ERROR_SUCCESS && data == FULL_RANGE
}

/// Set `GdiIcmGammaRange` through Windows' admin prompt (it lives in HKLM). `Ok(false)` if the
/// prompt was declined.
pub fn allow_full_gamma_range() -> Result<bool> {
    let verb = wide("runas");
    let file = wide("reg.exe");
    let params = wide(&format!(
        r#"add "HKLM\{ICM_KEY}" /v {GAMMA_RANGE} /t REG_DWORD /d {FULL_RANGE} /f"#
    ));
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = params.as_ptr();
    info.nShow = SW_HIDE;
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        if unsafe { GetLastError() } == ERROR_CANCELLED {
            return Ok(false);
        }
        bail!(
            "couldn't start reg.exe: {}",
            std::io::Error::last_os_error()
        );
    }
    let mut code = 1u32;
    unsafe {
        WaitForSingleObject(info.hProcess, 30_000);
        GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
    }
    if code != 0 || !full_gamma_range_allowed() {
        bail!("reg.exe couldn't set {GAMMA_RANGE} (exit code {code})");
    }
    log::info!("{GAMMA_RANGE} set to {FULL_RANGE}; takes effect after signing out and back in");
    Ok(true)
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
            refused_warm: false,
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
        if failed.is_empty() {
            if setting.temperature < WARM_K {
                self.refused_warm = false;
            }
        } else if setting != ColorSetting::NEUTRAL {
            self.refused_warm = true;
        }
        if !failed.is_empty() {
            if !self.warned_range {
                self.warned_range = true;
                log::warn!(
                    "Windows rejected the gamma ramp for {failed:?} at {}K. If this happens only at low \
                     temperatures, \"Allow warmer colours\" in the tray menu fixes it.",
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

    fn warm_colours_refused(&self) -> bool {
        self.refused_warm
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

    fn focused_window(&mut self) -> FocusedWindow {
        let hwnd: HWND = unsafe { GetForegroundWindow() };
        if hwnd.is_null() {
            return FocusedWindow::default();
        }
        let mut buf = [0u16; 512];
        let n = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
        let title = (n > 0).then(|| String::from_utf16_lossy(&buf[..n as usize]));
        FocusedWindow {
            title,
            fullscreen: covers_its_monitor(hwnd),
        }
    }
}

/// Whether `hwnd` covers the whole monitor it's on: fullscreen or borderless fullscreen. The
/// desktop and the taskbar cover a screen too, but aren't apps.
fn covers_its_monitor(hwnd: HWND) -> bool {
    let mut class = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) };
    let class = String::from_utf16_lossy(&class[..n.max(0) as usize]);
    if matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd") {
        return false;
    }
    let mut window: RECT = unsafe { std::mem::zeroed() };
    if unsafe { GetWindowRect(hwnd, &mut window) } == 0 {
        return false;
    }
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<MONITORINFO>() as u32;
    if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return false;
    }
    let m = info.rcMonitor;
    window.left <= m.left
        && window.top <= m.top
        && window.right >= m.right
        && window.bottom >= m.bottom
}

impl Drop for Gdi {
    fn drop(&mut self) {
        for d in &self.displays {
            unsafe { DeleteDC(d.hdc) };
        }
    }
}
