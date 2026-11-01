//! Windows tray icon via `tray-icon` + `muda`, driven by a plain Win32 message loop.

use anyhow::{Context, Result};
use ember::daemon::Handle;
use ember::hotkey::{Hotkey, Key, Modifiers};
use ember::tray_model::{self, Action, Anchor, DISABLE_OPTIONS, View};
use std::path::PathBuf;
use tray_icon::menu::{
    CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu,
};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    self as kbd, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN,
    RegisterHotKey, UnregisterHotKey, VIRTUAL_KEY,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MB_ICONERROR, MB_OK, MSG, MessageBoxW, MsgWaitForMultipleObjects, PM_REMOVE,
    PeekMessageW, QS_ALLINPUT, TranslateMessage, WM_HOTKEY, WM_QUIT,
};

/// Our only `RegisterHotKey` id; `WM_HOTKEY` carries it in `wParam`.
const HOTKEY_ID: i32 = 1;

pub fn message_box(text: &str) {
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (t, c) = (wide(text), wide("Ember"));
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            t.as_ptr(),
            c.as_ptr(),
            MB_OK | MB_ICONERROR,
        )
    };
}

fn virtual_key(key: Key) -> VIRTUAL_KEY {
    match key {
        // Letters and digits are their ASCII codes.
        Key::Letter(c) => c as VIRTUAL_KEY,
        Key::Digit(d) => kbd::VK_0 + d as VIRTUAL_KEY,
        Key::F(n) => kbd::VK_F1 + (n as VIRTUAL_KEY - 1),
        Key::End => kbd::VK_END,
        Key::Home => kbd::VK_HOME,
        Key::Insert => kbd::VK_INSERT,
        Key::Delete => kbd::VK_DELETE,
        Key::PageUp => kbd::VK_PRIOR,
        Key::PageDown => kbd::VK_NEXT,
        Key::Pause => kbd::VK_PAUSE,
        Key::ScrollLock => kbd::VK_SCROLL,
        Key::Left => kbd::VK_LEFT,
        Key::Right => kbd::VK_RIGHT,
        Key::Up => kbd::VK_UP,
        Key::Down => kbd::VK_DOWN,
        Key::Space => kbd::VK_SPACE,
    }
}

fn hotkey_modifiers(m: Modifiers) -> HOT_KEY_MODIFIERS {
    let mut out = MOD_NOREPEAT; // holding the keys down shouldn't toggle on and off
    for (on, flag) in [
        (m.ctrl, MOD_CONTROL),
        (m.alt, MOD_ALT),
        (m.shift, MOD_SHIFT),
        (m.win, MOD_WIN),
    ] {
        if on {
            out |= flag;
        }
    }
    out
}

/// The pause/resume hotkey, registered for this thread (no window), so `WM_HOTKEY` arrives in
/// the message queue the tray loop drains.
#[derive(Default)]
struct GlobalHotkey {
    /// What the config asks for; only retried when it changes.
    wanted: Option<Hotkey>,
    registered: bool,
}

impl GlobalHotkey {
    fn set(&mut self, hotkey: Option<Hotkey>) {
        if hotkey == self.wanted {
            return;
        }
        self.wanted = hotkey;
        if std::mem::take(&mut self.registered) {
            unsafe { UnregisterHotKey(std::ptr::null_mut(), HOTKEY_ID) };
        }
        let Some(h) = hotkey else {
            log::info!("toggle hotkey off");
            return;
        };
        let ok = unsafe {
            RegisterHotKey(
                std::ptr::null_mut(),
                HOTKEY_ID,
                hotkey_modifiers(h.mods),
                virtual_key(h.key) as u32,
            )
        } != 0;
        if ok {
            log::info!("toggle hotkey: {h}");
        } else {
            log::warn!(
                "could not register the toggle hotkey {h} (another app may be using it): {}",
                std::io::Error::last_os_error()
            );
        }
        self.registered = ok;
    }

    /// The hotkey, if it's actually registered.
    fn active(&self) -> Option<Hotkey> {
        self.wanted.filter(|_| self.registered)
    }
}

/// "Enabled", with the hotkey right-aligned like a menu accelerator.
fn enabled_label(hotkey: Option<Hotkey>) -> String {
    match hotkey {
        Some(h) => format!("Enabled\t{h}"),
        None => "Enabled".into(),
    }
}

fn icon(active: bool) -> Result<Icon> {
    const SIZE: u32 = 32;
    Icon::from_rgba(tray_model::icon_rgba(SIZE, active), SIZE, SIZE).context("building tray icon")
}

pub fn run(daemon: Handle, config_path: PathBuf) -> Result<()> {
    // Report the tray icon's position in real pixels, the same units the settings window
    // (winit) uses; otherwise Windows scales it on high-DPI displays.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let status = MenuItem::new("Starting…", false, None);
    let enabled = CheckMenuItem::new("Enabled", true, true, None);
    let disable_items: Vec<(MenuItem, u64)> = DISABLE_OPTIONS
        .iter()
        .map(|&(m, label)| (MenuItem::new(label, true, None), m))
        .collect();
    let disable_sub = Submenu::new("Disable for", true);
    for (item, _) in &disable_items {
        disable_sub.append(item)?;
    }
    let app = CheckMenuItem::new("Disable for current app", false, false, None);
    let autostart = CheckMenuItem::new("Start with Windows", true, false, None);
    let settings = MenuItem::new("Settings…", true, None);
    let open_cfg = MenuItem::new("Open config file", true, None);
    let quit = MenuItem::new("Quit Ember", true, None);

    let menu = Menu::new();
    menu.append_items(&[
        &status,
        &PredefinedMenuItem::separator(),
        &enabled,
        &disable_sub,
        &app,
        &PredefinedMenuItem::separator(),
        &settings,
        &autostart,
        &open_cfg,
        &PredefinedMenuItem::separator(),
        &quit,
    ])?;

    // Left-click opens the settings window above the icon; right-click shows the menu.
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip("Ember")
        .with_icon(icon(false)?)
        .build()
        .context("creating tray icon")?;
    // Where the icon was when last clicked, so "Settings…" in the menu opens next to it too.
    let mut anchor: Option<Anchor> = None;

    let action_for = |id: &MenuId, anchor: Option<Anchor>| -> Option<Action> {
        if id == enabled.id() {
            Some(Action::ToggleEnabled)
        } else if id == app.id() {
            Some(Action::ToggleAppRule)
        } else if id == autostart.id() {
            Some(Action::ToggleAutostart)
        } else if id == settings.id() {
            Some(Action::OpenSettings(anchor))
        } else if id == open_cfg.id() {
            Some(Action::OpenConfig)
        } else if id == quit.id() {
            Some(Action::Quit)
        } else {
            disable_items
                .iter()
                .find(|(i, _)| i.id() == id)
                .map(|&(_, m)| Action::DisableFor(m))
        }
    };

    let mut last: Option<View> = None;
    let mut hotkey = GlobalHotkey::default();
    loop {
        // Sleep until a window message arrives or 250ms pass, then drain the queue.
        unsafe { MsgWaitForMultipleObjects(0, std::ptr::null(), 0, 250, QS_ALLINPUT) };
        let mut actions = Vec::new();
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        while unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
            if msg.message == WM_QUIT {
                return Ok(());
            }
            // A thread message (no window), so there's nothing to dispatch it to.
            if msg.message == WM_HOTKEY && msg.wParam == HOTKEY_ID as usize {
                actions.push(Action::ToggleEnabled);
                continue;
            }
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click {
                rect,
                button,
                button_state: MouseButtonState::Up,
                ..
            } = ev
            {
                anchor = Some(Anchor {
                    x: rect.position.x.round() as i32,
                    y: rect.position.y.round() as i32,
                    width: rect.size.width as i32,
                    height: rect.size.height as i32,
                });
                if button == MouseButton::Left {
                    actions.push(Action::OpenSettings(anchor));
                }
            }
        }
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            actions.extend(action_for(ev.id(), anchor));
        }
        for action in actions {
            match tray_model::perform(action, &daemon, &config_path) {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                Err(e) => message_box(&format!("{e:#}")),
            }
            // muda flips check items itself on click; force a resync from the real state.
            last = None;
        }

        let v = tray_model::view(&daemon.state());
        // Follows config reloads, including edits from the settings window.
        hotkey.set(v.hotkey);
        if last.as_ref() != Some(&v) {
            status.set_text(&v.status);
            enabled.set_text(enabled_label(hotkey.active()));
            enabled.set_checked(v.enabled);
            autostart.set_checked(v.autostart);
            match &v.app_rule {
                Some((label, checked)) => {
                    app.set_text(label);
                    app.set_checked(*checked);
                    app.set_enabled(true);
                }
                None => {
                    app.set_text("Disable for current app (switch to an app first)");
                    app.set_checked(false);
                    app.set_enabled(false);
                }
            }
            let _ = tray.set_tooltip(Some(&v.tooltip));
            if last.as_ref().is_none_or(|l| l.active != v.active) {
                let _ = tray.set_icon(Some(icon(v.active)?));
            }
            last = Some(v);
        }
    }
}
