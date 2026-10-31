//! Linux/X11 backend: RandR per-CRTC gamma ramps (same mechanism as redshift/xsct).

use super::Backend;
use crate::color::{ColorSetting, Ramp};
use anyhow::{Context, Result};
use x11rb::connection::Connection;
use x11rb::protocol::randr::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{Atom, AtomEnum, ConnectionExt as _, Window};
use x11rb::rust_connection::RustConnection;

pub struct RandR {
    conn: RustConnection,
    root: Window,
    net_active_window: Atom,
    net_wm_pid: Atom,
}

impl RandR {
    pub fn open() -> Result<RandR> {
        let (conn, screen) =
            x11rb::connect(None).context("connecting to the X server (is DISPLAY set?)")?;
        let root = conn.setup().roots[screen].root;
        conn.randr_query_version(1, 3)?
            .reply()
            .context("X server lacks RandR 1.3")?;
        let atom = |n: &[u8]| -> Result<Atom> { Ok(conn.intern_atom(false, n)?.reply()?.atom) };
        let net_active_window = atom(b"_NET_ACTIVE_WINDOW")?;
        let net_wm_pid = atom(b"_NET_WM_PID")?;
        let me = RandR {
            conn,
            root,
            net_active_window,
            net_wm_pid,
        };
        let n = me.crtcs()?.len();
        log::info!("x11: controlling {n} CRTC(s)");
        Ok(me)
    }

    /// Current CRTCs with their gamma size. Re-queried each time so hot-plugged monitors work.
    fn crtcs(&self) -> Result<Vec<(randr::Crtc, usize)>> {
        let res = self
            .conn
            .randr_get_screen_resources_current(self.root)?
            .reply()?;
        let mut out = Vec::with_capacity(res.crtcs.len());
        for &crtc in &res.crtcs {
            let size = self.conn.randr_get_crtc_gamma_size(crtc)?.reply()?.size as usize;
            if size == 0 {
                continue;
            }
            out.push((crtc, size));
        }
        Ok(out)
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
}

impl Backend for RandR {
    fn name(&self) -> &'static str {
        "x11-randr"
    }

    fn apply(&mut self, setting: ColorSetting) -> Result<()> {
        for (crtc, size) in self.crtcs()? {
            let r = Ramp::build(setting, size);
            self.conn
                .randr_set_crtc_gamma(crtc, &r.red, &r.green, &r.blue)?;
        }
        self.conn.flush()?;
        Ok(())
    }

    fn restore(&mut self) -> Result<()> {
        self.apply(ColorSetting::NEUTRAL)?;
        self.sync()
    }

    fn sync(&mut self) -> Result<()> {
        self.conn.get_input_focus()?.reply()?;
        Ok(())
    }

    fn focused_pid(&mut self) -> Option<u32> {
        let win = self.property_u32(self.root, self.net_active_window, AtomEnum::WINDOW)?;
        if win == 0 {
            return None;
        }
        self.property_u32(win, self.net_wm_pid, AtomEnum::CARDINAL)
    }
}
