//! Linux/X11 backend: RandR per-CRTC gamma ramps (same mechanism as redshift/xsct).
//! The ramp found on each CRTC the first time it drives an output (`GetCrtcGamma`) is kept as
//! that output's calibration base; see `calibration.rs`. The ramps are read again before writing
//! (at most once every [`CHECK_EVERY`]), so a calibration curve loaded after Ember started
//! (xcalib, dispwin, the desktop's colour manager) is picked up.

use super::Backend;
use super::calibration::Calibration;
use crate::color::{ColorSetting, Ramp};
use crate::config::{self, Display};
use anyhow::{Context, Result};
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::randr::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{Atom, AtomEnum, ConnectionExt as _, Window};
use x11rb::rust_connection::RustConnection;

/// Focused-window lookup via `_NET_ACTIVE_WINDOW` + `_NET_WM_PID`. Also used on Wayland desktops
/// whose compositor publishes the active XWayland window this way (KWin, Mutter); native Wayland
/// windows show up there as "no window".
pub struct Focus {
    conn: RustConnection,
    root: Window,
    net_active_window: Atom,
    net_wm_pid: Atom,
}

impl Focus {
    pub fn connect() -> Result<Focus> {
        let (conn, screen) =
            x11rb::connect(None).context("connecting to the X server (is DISPLAY set?)")?;
        let root = conn.setup().roots[screen].root;
        let atom = |n: &[u8]| -> Result<Atom> { Ok(conn.intern_atom(false, n)?.reply()?.atom) };
        let net_active_window = atom(b"_NET_ACTIVE_WINDOW")?;
        let net_wm_pid = atom(b"_NET_WM_PID")?;
        Ok(Focus {
            conn,
            root,
            net_active_window,
            net_wm_pid,
        })
    }

    fn property_u32(&self, window: Window, prop: Atom, ty: AtomEnum) -> Option<u32> {
        let reply = self
            .conn
            .get_property(false, window, prop, ty, 0, 1)
            .ok()?
            .reply()
            .ok()?;
        reply.value32()?.next()
    }

    pub fn focused_pid(&self) -> Option<u32> {
        let win = self.property_u32(self.root, self.net_active_window, AtomEnum::WINDOW)?;
        if win == 0 {
            return None;
        }
        self.property_u32(win, self.net_wm_pid, AtomEnum::CARDINAL)
    }
}

/// How often to look for ramps changed by another program. Reading them costs a roundtrip to the
/// X server per CRTC; this skips that on fade frames (every 30 ms) but not on the regular poll
/// (every second or more), which is soon enough to catch a calibration loader.
const CHECK_EVERY: Duration = Duration::from_millis(900);

pub struct RandR {
    focus: Focus,
    calibration: Calibration,
    settings: Vec<Display>,
    last_check: Option<Instant>,
}

struct Crtc {
    id: randr::Crtc,
    gamma_size: usize,
    /// Output name(s) such as `HDMI-1`, the key for its saved calibration curve.
    /// `None` if the CRTC isn't driving anything.
    name: Option<String>,
}

impl RandR {
    pub fn open(preserve_calibration: bool) -> Result<RandR> {
        let focus = Focus::connect()?;
        focus
            .conn
            .randr_query_version(1, 3)?
            .reply()
            .context("X server lacks RandR 1.3")?;
        let mut me = RandR {
            focus,
            calibration: Calibration::new(preserve_calibration),
            settings: Vec::new(),
            last_check: None,
        };
        let crtcs = me.crtcs()?;
        log::info!("x11: controlling {} CRTC(s)", crtcs.len());
        me.capture_new(&crtcs);
        Ok(me)
    }

    fn conn(&self) -> &RustConnection {
        &self.focus.conn
    }

    /// Current CRTCs with their gamma size. Re-queried each time so hot-plugged monitors work.
    fn crtcs(&self) -> Result<Vec<Crtc>> {
        let res = self
            .conn()
            .randr_get_screen_resources_current(self.focus.root)?
            .reply()?;
        let mut out = Vec::with_capacity(res.crtcs.len());
        for &id in &res.crtcs {
            let gamma_size = self.conn().randr_get_crtc_gamma_size(id)?.reply()?.size as usize;
            if gamma_size == 0 {
                continue;
            }
            let name = self.output_names(id, res.config_timestamp)?;
            out.push(Crtc {
                id,
                gamma_size,
                name,
            });
        }
        Ok(out)
    }

    /// The names of the outputs `crtc` drives, joined with `+` (cloned outputs share a CRTC).
    fn output_names(&self, crtc: randr::Crtc, timestamp: u32) -> Result<Option<String>> {
        let info = self.conn().randr_get_crtc_info(crtc, timestamp)?.reply()?;
        let mut names = Vec::with_capacity(info.outputs.len());
        for &output in &info.outputs {
            let o = self
                .conn()
                .randr_get_output_info(output, timestamp)?
                .reply()?;
            names.push(String::from_utf8_lossy(&o.name).into_owned());
        }
        Ok((!names.is_empty()).then(|| names.join("+")))
    }

    /// The ramp currently loaded on `crtc` (by xcalib/dispwin, the desktop, or us).
    fn read_ramp(&self, crtc: randr::Crtc) -> Option<Ramp> {
        let g = self.conn().randr_get_crtc_gamma(crtc).ok()?.reply().ok()?;
        Some(Ramp {
            red: g.red,
            green: g.green,
            blue: g.blue,
        })
    }

    /// Pick the calibration base of outputs we haven't seen yet (at startup, or hot-plugged).
    fn capture_new(&mut self, crtcs: &[Crtc]) {
        let new: Vec<(String, Option<Ramp>)> = crtcs
            .iter()
            .filter_map(|c| {
                let name = c.name.as_ref().filter(|n| !self.calibration.knows(n))?;
                Some((name.clone(), self.read_ramp(c.id)))
            })
            .collect();
        if !new.is_empty() {
            self.calibration.capture(new);
        }
    }

    /// Adopt calibration curves loaded by another program since we last wrote; see
    /// `Calibration::check`. All requests go out before the first reply is awaited, so this costs
    /// one roundtrip however many CRTCs there are.
    fn check_changed(&mut self, crtcs: &[Crtc]) {
        let conn = &self.focus.conn;
        let pending: Vec<_> = crtcs
            .iter()
            .filter_map(|c| Some((c.name.as_ref()?, conn.randr_get_crtc_gamma(c.id).ok()?)))
            .collect();
        for (name, cookie) in pending {
            if let Ok(g) = cookie.reply() {
                let current = Ramp {
                    red: g.red,
                    green: g.green,
                    blue: g.blue,
                };
                self.calibration.check(name, &current);
            }
        }
    }
}

impl Backend for RandR {
    fn name(&self) -> &'static str {
        "x11-randr"
    }

    fn apply(&mut self, setting: ColorSetting) -> Result<()> {
        let crtcs = self.crtcs()?;
        self.capture_new(&crtcs);
        if self.last_check.is_none_or(|t| t.elapsed() >= CHECK_EVERY) {
            self.last_check = Some(Instant::now());
            self.check_changed(&crtcs);
        }
        for c in crtcs {
            let r = match &c.name {
                Some(name) if config::tints_display(&self.settings, name) => {
                    self.calibration.ramp(name, setting, c.gamma_size)
                }
                Some(name) => self.calibration.untinted_ramp(name, c.gamma_size),
                None => Ramp::build(setting, c.gamma_size),
            };
            self.conn()
                .randr_set_crtc_gamma(c.id, &r.red, &r.green, &r.blue)?;
            if let Some(name) = &c.name {
                self.calibration.wrote(name, r);
            }
        }
        self.conn().flush()?;
        Ok(())
    }

    fn restore(&mut self) -> Result<()> {
        self.apply(ColorSetting::NEUTRAL)?;
        self.sync()
    }

    fn set_preserve_calibration(&mut self, on: bool) {
        self.calibration.set_preserve(on);
    }

    fn set_app_rule_active(&mut self, on: bool) {
        self.calibration.set_app_rule_active(on);
    }

    fn set_displays(&mut self, displays: &[Display]) {
        self.settings = displays.to_vec();
    }

    /// One entry per CRTC in use, named after its output(s) like the config expects.
    fn displays(&mut self) -> Vec<(String, String)> {
        match self.crtcs() {
            Ok(crtcs) => crtcs
                .into_iter()
                .filter_map(|c| c.name)
                .map(|n| (n.clone(), n))
                .collect(),
            Err(e) => {
                log::debug!("x11: listing outputs failed: {e:#}");
                Vec::new()
            }
        }
    }

    fn sync(&mut self) -> Result<()> {
        self.conn().get_input_focus()?.reply()?;
        Ok(())
    }

    fn focused_pid(&mut self) -> Option<u32> {
        self.focus.focused_pid()
    }
}
