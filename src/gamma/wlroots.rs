//! Wayland backend for wlroots-based compositors (sway, Hyprland, river, Wayfire, labwc…):
//! `wlr-gamma-control-unstable-v1` for the ramps (same mechanism as gammastep/wlsunset) and
//! `wlr-foreign-toplevel-management-unstable-v1` for the focused window's app_id.

use super::Backend;
use crate::color::{ColorSetting, Ramp};
use anyhow::{Context, Result, bail};
use rustix::fs::MemfdFlags;
use std::collections::HashMap;
use std::fs::File;
use std::io::{ErrorKind, Seek, Write};
use std::os::fd::AsFd;
use wayland_client::backend::{ObjectId, WaylandError};
use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop, event_created_child,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_handle_v1::{
    self, ZwlrForeignToplevelHandleV1,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_manager_v1::{
    self, ZwlrForeignToplevelManagerV1,
};
use wayland_protocols_wlr::gamma_control::v1::client::zwlr_gamma_control_manager_v1::ZwlrGammaControlManagerV1;
use wayland_protocols_wlr::gamma_control::v1::client::zwlr_gamma_control_v1::{
    self, ZwlrGammaControlV1,
};

struct Output {
    /// Registry name of the `wl_output` global, so it can be dropped when unplugged.
    global: u32,
    wl: WlOutput,
    /// Connector name such as "DP-1" (wl_output v4), for log messages.
    name: Option<String>,
    control: Option<ZwlrGammaControlV1>,
    /// Ramp length announced by the compositor; nothing can be sent before it arrives.
    size: Option<usize>,
    /// The compositor refused gamma control (another client has it, or the output has no gamma).
    /// Not retried until the output is plugged in again.
    failed: bool,
    /// What we last sent, so unchanged settings aren't re-sent every tick.
    applied: Option<ColorSetting>,
}

impl Output {
    fn label(&self) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| format!("#{}", self.global))
    }
}

/// Toplevel state is double-buffered: changes arrive one by one and take effect on `done`.
#[derive(Default)]
struct Toplevel {
    app_id: Option<String>,
    activated: bool,
    pending_app_id: Option<String>,
    pending_activated: Option<bool>,
}

struct State {
    gamma: ZwlrGammaControlManagerV1,
    outputs: Vec<Output>,
    toplevels: HashMap<ObjectId, Toplevel>,
}

impl State {
    fn add_output(
        &mut self,
        registry: &WlRegistry,
        global: u32,
        version: u32,
        qh: &QueueHandle<Self>,
    ) {
        let wl = registry.bind::<WlOutput, _, _>(global, version.min(4), qh, global);
        self.outputs.push(Output {
            global,
            wl,
            name: None,
            control: None,
            size: None,
            failed: false,
            applied: None,
        });
    }

    fn remove_output(&mut self, global: u32) {
        let Some(i) = self.outputs.iter().position(|o| o.global == global) else {
            return;
        };
        let o = self.outputs.remove(i);
        log::info!("wayland: output {} removed", o.label());
        if let Some(c) = o.control {
            c.destroy();
        }
        if o.wl.version() >= 3 {
            o.wl.release();
        }
    }

    /// Ask for a gamma control on every output that lacks one. Returns true if any were requested
    /// (their `gamma_size` arrives with the next roundtrip).
    fn ensure_controls(&mut self, qh: &QueueHandle<Self>) -> bool {
        let mut created = false;
        for o in self
            .outputs
            .iter_mut()
            .filter(|o| o.control.is_none() && !o.failed)
        {
            o.control = Some(self.gamma.get_gamma_control(&o.wl, qh, o.global));
            o.size = None;
            o.applied = None;
            created = true;
        }
        created
    }

    fn focused_app_id(&self) -> Option<String> {
        self.toplevels
            .values()
            .find(|t| t.activated)
            .and_then(|t| t.app_id.as_deref())
            .and_then(app_id_name)
    }
}

/// The ramp file wlr-gamma-control expects: `size` red, then `size` green, then `size` blue
/// 16-bit entries in native byte order.
fn ramp_bytes(r: &Ramp) -> Vec<u8> {
    r.red
        .iter()
        .chain(&r.green)
        .chain(&r.blue)
        .flat_map(|v| v.to_ne_bytes())
        .collect()
}

fn ramp_file(r: &Ramp) -> Result<File> {
    let fd = rustix::fs::memfd_create("ember-gamma", MemfdFlags::CLOEXEC)
        .context("creating a memfd for the gamma ramp")?;
    let mut f = File::from(fd);
    f.write_all(&ramp_bytes(r))?;
    // The compositor read()s from the (shared) file offset, so hand the file over rewound.
    f.rewind()?;
    Ok(f)
}

/// `state` is an array of native-endian u32 enum values.
fn is_activated(state: &[u8]) -> bool {
    let activated = zwlr_foreign_toplevel_handle_v1::State::Activated as u32;
    state
        .as_chunks::<4>()
        .0
        .iter()
        .any(|&c| u32::from_ne_bytes(c) == activated)
}

/// The name rules match against. Some apps report their desktop file name, `foo.desktop`.
fn app_id_name(app_id: &str) -> Option<String> {
    let id = app_id.trim();
    let id = id.strip_suffix(".desktop").unwrap_or(id);
    (!id.is_empty()).then(|| id.to_string())
}

pub struct Wlr {
    queue: EventQueue<State>,
    qh: QueueHandle<State>,
    globals: GlobalList,
    state: State,
    /// Bound lazily on the first focus query, so a backend that only previews colours (the
    /// settings window) doesn't receive a stream of window-title events it never reads.
    toplevel_manager: Option<Option<ZwlrForeignToplevelManagerV1>>,
}

impl Wlr {
    /// `Ok(None)` if the compositor doesn't offer wlr-gamma-control (it isn't wlroots-based).
    pub fn open() -> Result<Option<Wlr>> {
        let conn = Connection::connect_to_env()
            .context("connecting to the Wayland compositor (is WAYLAND_DISPLAY set?)")?;
        let (globals, queue) =
            registry_queue_init::<State>(&conn).context("listing Wayland globals")?;
        let qh = queue.handle();
        let Ok(gamma) = globals.bind::<ZwlrGammaControlManagerV1, _, _>(&qh, 1..=1, ()) else {
            return Ok(None);
        };
        let mut state = State {
            gamma,
            outputs: Vec::new(),
            toplevels: HashMap::new(),
        };
        let outputs: Vec<(u32, u32)> = globals.contents().with_list(|list| {
            list.iter()
                .filter(|g| g.interface == WlOutput::interface().name)
                .map(|g| (g.name, g.version))
                .collect()
        });
        for (global, version) in outputs {
            state.add_output(globals.registry(), global, version, &qh);
        }
        state.ensure_controls(&qh);
        let mut me = Wlr {
            queue,
            qh,
            globals,
            state,
            toplevel_manager: None,
        };
        me.queue
            .roundtrip(&mut me.state)
            .context("talking to the Wayland compositor")?;

        let outputs = &me.state.outputs;
        if !outputs.is_empty() && outputs.iter().all(|o| o.failed) {
            bail!(
                "the compositor refused gamma control on every output. Another program is probably \
                 already adjusting the screen colours (gammastep, wlsunset, redshift, another Ember…); \
                 quit it and start Ember again"
            );
        }
        let usable = outputs.iter().filter(|o| o.size.is_some()).count();
        log::info!("wayland: controlling {usable} output(s) via wlr-gamma-control");
        Ok(Some(me))
    }

    /// Read whatever the compositor has sent since last time, without blocking.
    fn pump(&mut self) -> Result<()> {
        self.queue
            .flush()
            .context("writing to the Wayland compositor")?;
        if let Some(guard) = self.queue.prepare_read() {
            match guard.read() {
                Ok(_) => {}
                Err(WaylandError::Io(e)) if e.kind() == ErrorKind::WouldBlock => {}
                Err(e) => return Err(e).context("reading from the Wayland compositor"),
            }
        }
        self.queue.dispatch_pending(&mut self.state)?;
        Ok(())
    }

    fn bind_toplevel_manager(&mut self) -> Result<()> {
        if self.toplevel_manager.is_some() {
            return Ok(());
        }
        let manager = self
            .globals
            .bind::<ZwlrForeignToplevelManagerV1, _, _>(&self.qh, 1..=3, ())
            .ok();
        if manager.is_none() {
            log::warn!(
                "wayland: compositor lacks wlr-foreign-toplevel-management, so rules with \
                 when = \"focused\" can't see the focused app"
            );
        }
        self.toplevel_manager = Some(manager);
        // Receive the existing windows straight away.
        self.queue.roundtrip(&mut self.state)?;
        Ok(())
    }
}

impl Backend for Wlr {
    fn name(&self) -> &'static str {
        "wlr-gamma-control"
    }

    fn apply(&mut self, setting: ColorSetting) -> Result<()> {
        self.pump()?;
        // Outputs plugged in since last time, or controls dropped by restore().
        if self.state.ensure_controls(&self.qh) {
            self.queue.roundtrip(&mut self.state)?;
        }
        for o in &mut self.state.outputs {
            let (Some(control), Some(size)) = (&o.control, o.size) else {
                continue;
            };
            if size == 0 || o.applied == Some(setting) {
                continue;
            }
            // The fd is duplicated when the request is queued, so the file can be dropped after.
            let file = ramp_file(&Ramp::build(setting, size))?;
            control.set_gamma(file.as_fd());
            o.applied = Some(setting);
        }
        self.queue.flush()?;
        let outputs = &self.state.outputs;
        if !outputs.is_empty() && outputs.iter().all(|o| o.control.is_none()) {
            bail!("no output accepts gamma changes (another program has taken control of them)");
        }
        Ok(())
    }

    /// Destroying a gamma control makes the compositor restore that output's original ramp.
    fn restore(&mut self) -> Result<()> {
        for o in &mut self.state.outputs {
            if let Some(c) = o.control.take() {
                c.destroy();
            }
            o.size = None;
            o.applied = None;
        }
        self.sync()
    }

    fn sync(&mut self) -> Result<()> {
        self.queue.roundtrip(&mut self.state)?;
        Ok(())
    }

    fn focused_pid(&mut self) -> Option<u32> {
        None
    }

    fn focused_app_id(&mut self) -> Option<String> {
        if let Err(e) = self.bind_toplevel_manager().and_then(|()| self.pump()) {
            log::debug!("wayland: reading window list failed: {e:#}");
        }
        self.state.focused_app_id()
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == WlOutput::interface().name => {
                log::info!("wayland: output added");
                state.add_output(registry, name, version, qh);
            }
            wl_registry::Event::GlobalRemove { name } => state.remove_output(name),
            _ => {}
        }
    }
}

impl Dispatch<WlOutput, u32> for State {
    fn event(
        state: &mut Self,
        _: &WlOutput,
        event: wl_output::Event,
        global: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event
            && let Some(o) = state.outputs.iter_mut().find(|o| o.global == *global)
        {
            o.name = Some(name);
        }
    }
}

delegate_noop!(State: ZwlrGammaControlManagerV1);

impl Dispatch<ZwlrGammaControlV1, u32> for State {
    fn event(
        state: &mut Self,
        control: &ZwlrGammaControlV1,
        event: zwlr_gamma_control_v1::Event,
        global: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(o) = state
            .outputs
            .iter_mut()
            .find(|o| o.global == *global && o.control.as_ref() == Some(control))
        else {
            return;
        };
        match event {
            zwlr_gamma_control_v1::Event::GammaSize { size } => o.size = Some(size as usize),
            zwlr_gamma_control_v1::Event::Failed => {
                log::warn!(
                    "wayland: output {} refused gamma control (another program such as gammastep \
                     or wlsunset may be using it, or the output doesn't support gamma)",
                    o.label()
                );
                control.destroy();
                o.control = None;
                o.size = None;
                o.failed = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrForeignToplevelManagerV1,
        event: zwlr_foreign_toplevel_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } => {
                state.toplevels.insert(toplevel.id(), Toplevel::default());
            }
            zwlr_foreign_toplevel_manager_v1::Event::Finished => {
                log::warn!("wayland: compositor stopped sending the window list");
                state.toplevels.clear();
            }
            _ => {}
        }
    }

    event_created_child!(State, ZwlrForeignToplevelManagerV1, [
        zwlr_foreign_toplevel_manager_v1::EVT_TOPLEVEL_OPCODE => (ZwlrForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for State {
    fn event(
        state: &mut Self,
        handle: &ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_foreign_toplevel_handle_v1::Event;
        if let Event::Closed = event {
            state.toplevels.remove(&handle.id());
            handle.destroy();
            return;
        }
        let t = state.toplevels.entry(handle.id()).or_default();
        match event {
            Event::AppId { app_id } => t.pending_app_id = Some(app_id),
            Event::State { state: flags } => t.pending_activated = Some(is_activated(&flags)),
            Event::Done => {
                if let Some(id) = t.pending_app_id.take() {
                    t.app_id = Some(id);
                }
                if let Some(a) = t.pending_activated.take() {
                    t.activated = a;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramp_file_layout_is_red_green_blue_planes() {
        let r = Ramp {
            red: vec![1, 2],
            green: vec![3, 4],
            blue: vec![5, 0xABCD],
        };
        let b = ramp_bytes(&r);
        assert_eq!(b.len(), 2 * 3 * 2);
        let words: Vec<u16> = b
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| u16::from_ne_bytes(c))
            .collect();
        assert_eq!(words, [1, 2, 3, 4, 5, 0xABCD]);
    }

    #[test]
    fn ramp_size_matches_gamma_size() {
        let r = Ramp::build(ColorSetting::NEUTRAL, 1024);
        assert_eq!(ramp_bytes(&r).len(), 1024 * 3 * 2);
    }

    #[test]
    fn activated_state_is_found() {
        let flags = |v: &[u32]| -> Vec<u8> { v.iter().flat_map(|x| x.to_ne_bytes()).collect() };
        assert!(is_activated(&flags(&[0, 2])));
        assert!(is_activated(&flags(&[2])));
        assert!(!is_activated(&flags(&[0, 1, 3])));
        assert!(!is_activated(&[]));
    }

    #[test]
    fn app_ids_become_rule_names() {
        assert_eq!(
            app_id_name("steam_app_730").as_deref(),
            Some("steam_app_730")
        );
        assert_eq!(
            app_id_name("org.mozilla.firefox.desktop").as_deref(),
            Some("org.mozilla.firefox")
        );
        assert_eq!(app_id_name("  "), None);
    }
}
