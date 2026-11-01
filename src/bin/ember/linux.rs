//! Linux tray icon via the StatusNotifierItem D-Bus protocol (KDE, XFCE, Cinnamon, sway/waybar,
//! GNOME with the AppIndicator extension).

use anyhow::{Context, Result, anyhow};
use ember::daemon::Handle;
use ember::hotkey::{Hotkey, Key, Modifiers};
use ember::tray_model::{self, Action, DISABLE_OPTIONS, View};
use ksni::blocking::TrayMethods;
use ksni::menu::{CheckmarkItem, MenuItem, StandardItem, SubMenu};
use std::path::PathBuf;
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::time::Duration;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{ConnectionExt as _, GrabMode, Keycode, Keysym, ModMask, Window};
use x11rb::rust_connection::RustConnection;

struct EmberTray {
    view: View,
    daemon: Handle,
    config_path: PathBuf,
    quit: Sender<()>,
}

impl EmberTray {
    fn act(&mut self, action: Action) {
        match tray_model::perform(action, &self.daemon, &self.config_path) {
            Ok(true) => {
                let _ = self.quit.send(());
            }
            Ok(false) => self.view = tray_model::view(&self.daemon.state()),
            Err(e) => log::error!("{e:#}"),
        }
    }
}

fn argb(size: u32, active: bool) -> ksni::Icon {
    let rgba = tray_model::icon_rgba(size, active);
    let mut data = Vec::with_capacity(rgba.len());
    for p in rgba.as_chunks::<4>().0 {
        data.extend_from_slice(&[p[3], p[0], p[1], p[2]]);
    }
    ksni::Icon {
        width: size as i32,
        height: size as i32,
        data,
    }
}

impl ksni::Tray for EmberTray {
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        "ember".into()
    }
    fn title(&self) -> String {
        "Ember".into()
    }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        [22, 32, 48]
            .iter()
            .map(|&s| argb(s, self.view.active))
            .collect()
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Ember".into(),
            description: self.view.status.clone(),
            ..Default::default()
        }
    }
    fn menu(&self) -> Vec<MenuItem<Self>> {
        let v = &self.view;
        let mut items: Vec<MenuItem<Self>> = vec![
            StandardItem {
                label: v.status.clone(),
                enabled: false,
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            CheckmarkItem {
                label: "Enabled".into(),
                checked: v.enabled,
                shortcut: v.hotkey.map(dbus_shortcut).into_iter().collect(),
                activate: Box::new(|t: &mut Self| t.act(Action::ToggleEnabled)),
                ..Default::default()
            }
            .into(),
            SubMenu {
                label: "Disable for".into(),
                submenu: DISABLE_OPTIONS
                    .iter()
                    .map(|&(m, label)| {
                        StandardItem {
                            label: label.into(),
                            activate: Box::new(move |t: &mut Self| t.act(Action::DisableFor(m))),
                            ..Default::default()
                        }
                        .into()
                    })
                    .collect(),
                ..Default::default()
            }
            .into(),
        ];
        items.push(match &v.app_rule {
            Some((label, checked)) => CheckmarkItem {
                label: label.replace('_', "__"),
                checked: *checked,
                activate: Box::new(|t: &mut Self| t.act(Action::ToggleAppRule)),
                ..Default::default()
            }
            .into(),
            None => StandardItem {
                label: "Disable for current app (switch to an app first)".into(),
                enabled: false,
                ..Default::default()
            }
            .into(),
        });
        items.extend([
            MenuItem::Separator,
            StandardItem {
                label: "Settings…".into(),
                activate: Box::new(|t: &mut Self| t.act(Action::OpenSettings(None))),
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: "Start at login".into(),
                checked: v.autostart,
                activate: Box::new(|t: &mut Self| t.act(Action::ToggleAutostart)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Open config file".into(),
                activate: Box::new(|t: &mut Self| t.act(Action::OpenConfig)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit Ember".into(),
                activate: Box::new(|t: &mut Self| t.act(Action::Quit)),
                ..Default::default()
            }
            .into(),
        ]);
        items
    }
}

pub fn run(daemon: Handle, config_path: PathBuf) -> Result<()> {
    let (quit_tx, quit_rx) = channel();
    let view = tray_model::view(&daemon.state());
    let tray = EmberTray {
        view,
        daemon: daemon.clone(),
        config_path,
        quit: quit_tx,
    };
    let handle = tray.spawn().map_err(|e| {
        anyhow!(
            "could not create a tray icon ({e}). Your desktop needs a StatusNotifierItem tray \
             (on GNOME: install the \"AppIndicator and KStatusNotifierItem Support\" extension)."
        )
    })?;
    {
        let (daemon, tray) = (daemon.clone(), handle.clone());
        std::thread::Builder::new()
            .name("ember-hotkey".into())
            .spawn(move || {
                if let Err(e) = watch_hotkey(&daemon, &tray) {
                    log::warn!("toggle hotkey stopped working: {e:#}");
                }
            })
            .context("spawning hotkey thread")?;
    }

    loop {
        match quit_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                // Let the D-Bus reply to the "Quit" click go out before tearing the service down.
                std::thread::sleep(Duration::from_millis(150));
                break;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        let v = tray_model::view(&daemon.state());
        if handle
            .update(|t| (t.view != v).then(|| t.view = v))
            .is_none()
        {
            break; // tray service died
        }
    }
    handle.shutdown().wait();
    Ok(())
}

/// The hotkey as a dbusmenu shortcut, e.g. `["Control", "Alt", "End"]`, so the tray shows it
/// next to "Enabled".
fn dbus_shortcut(h: Hotkey) -> Vec<String> {
    let m = h.mods;
    [
        (m.ctrl, "Control"),
        (m.alt, "Alt"),
        (m.shift, "Shift"),
        (m.win, "Super"),
    ]
    .into_iter()
    .filter(|&(on, _)| on)
    .map(|(_, name)| name.to_string())
    .chain([h.key.to_string()])
    .collect()
}

/// X11 keysym for a key (values from X11/keysymdef.h).
fn keysym(key: Key) -> Keysym {
    match key {
        // Latin letters and digits are their (lower-case) ASCII codes.
        Key::Letter(c) => c.to_ascii_lowercase() as Keysym,
        Key::Digit(d) => b'0' as Keysym + d as Keysym,
        Key::F(n) => 0xffbe + (n as Keysym - 1),
        Key::End => 0xff57,
        Key::Home => 0xff50,
        Key::Insert => 0xff63,
        Key::Delete => 0xffff,
        Key::PageUp => 0xff55,
        Key::PageDown => 0xff56,
        Key::Pause => 0xff13,
        Key::ScrollLock => 0xff14,
        Key::Left => 0xff51,
        Key::Up => 0xff52,
        Key::Right => 0xff53,
        Key::Down => 0xff54,
        Key::Space => 0x20,
    }
}

/// Alt is Mod1 and Super is Mod4 on every common keyboard setup.
fn mod_mask(m: Modifiers) -> ModMask {
    let mut out = ModMask::from(0u16);
    for (on, flag) in [
        (m.ctrl, ModMask::CONTROL),
        (m.alt, ModMask::M1),
        (m.shift, ModMask::SHIFT),
        (m.win, ModMask::M4),
    ] {
        if on {
            out |= flag;
        }
    }
    out
}

/// Every keycode that produces `sym` on the current keyboard layout (at any shift level).
fn keycodes(conn: &RustConnection, sym: Keysym) -> Result<Vec<Keycode>> {
    let (min, max) = (conn.setup().min_keycode, conn.setup().max_keycode);
    let map = conn.get_keyboard_mapping(min, max - min + 1)?.reply()?;
    let per = map.keysyms_per_keycode as usize;
    if per == 0 {
        return Ok(Vec::new());
    }
    Ok(map
        .keysyms
        .chunks(per)
        .enumerate()
        .filter(|(_, syms)| syms.contains(&sym))
        .map(|(i, _)| min + i as Keycode)
        .collect())
}

/// Passive key grabs on the root window for the current hotkey.
struct Grab {
    root: Window,
    keycodes: Vec<Keycode>,
}

impl Grab {
    /// Grab `h` with and without CapsLock (Lock) and NumLock (Mod2), so it works whatever
    /// state those are in. Fails if another client already grabbed the combination.
    fn new(conn: &RustConnection, root: Window, h: Hotkey) -> Result<Grab> {
        let keycodes = keycodes(conn, keysym(h.key))?;
        if keycodes.is_empty() {
            anyhow::bail!("no key on this keyboard layout produces {}", h.key);
        }
        let grab = Grab { root, keycodes };
        let mods = mod_mask(h.mods);
        let locks = [
            ModMask::from(0u16),
            ModMask::LOCK,
            ModMask::M2,
            ModMask::LOCK | ModMask::M2,
        ];
        for &code in &grab.keycodes {
            for lock in locks {
                let r = conn
                    .grab_key(
                        false,
                        root,
                        mods | lock,
                        code,
                        GrabMode::ASYNC,
                        GrabMode::ASYNC,
                    )?
                    .check();
                if let Err(e) = r {
                    grab.release(conn);
                    return Err(e).context("another app may be using it");
                }
            }
        }
        Ok(grab)
    }

    fn release(&self, conn: &RustConnection) {
        for &code in &self.keycodes {
            let _ = conn.ungrab_key(code, self.root, ModMask::ANY);
        }
        let _ = conn.flush();
    }
}

/// Toggle Ember when the configured hotkey is pressed anywhere, following config changes.
/// Returns when the tray goes away. Without an X display (e.g. pure Wayland) it logs and returns.
fn watch_hotkey(daemon: &Handle, tray: &ksni::blocking::Handle<EmberTray>) -> Result<()> {
    let (conn, screen) = match x11rb::connect(None) {
        Ok(c) => c,
        Err(e) => {
            log::info!("no toggle hotkey: can't connect to an X server ({e})");
            return Ok(());
        }
    };
    let root = conn.setup().roots[screen].root;
    let mut wanted: Option<Hotkey> = None;
    let mut grab: Option<Grab> = None;
    let mut last_release = None;
    loop {
        let h = tray_model::hotkey(&daemon.state());
        if h != wanted {
            wanted = h;
            if let Some(g) = grab.take() {
                g.release(&conn);
            }
            match h.map(|h| (h, Grab::new(&conn, root, h))) {
                None => log::info!("toggle hotkey off"),
                Some((h, Ok(g))) => {
                    log::info!("toggle hotkey: {h}");
                    grab = Some(g);
                }
                Some((h, Err(e))) => log::warn!("could not grab the toggle hotkey {h}: {e:#}"),
            }
        }
        while let Some(ev) = conn.poll_for_event()? {
            match ev {
                Event::KeyRelease(e) => last_release = Some(e.time),
                Event::KeyPress(e)
                    if grab
                        .as_ref()
                        .is_some_and(|g| g.keycodes.contains(&e.detail)) =>
                {
                    // Holding the keys makes X auto-repeat them as a release and a press with
                    // the same timestamp; only a real press toggles.
                    let repeat = last_release == Some(e.time);
                    if !repeat && tray.update(|t| t.act(Action::ToggleEnabled)).is_none() {
                        return Ok(()); // tray service is gone
                    }
                }
                _ => {}
            }
        }
        if tray.is_closed() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
