//! Windows tray icon via `tray-icon` + `muda`, driven by a plain Win32 message loop.

use anyhow::{Context, Result};
use ember::daemon::Handle;
use ember::tray_model::{self, Action, DISABLE_OPTIONS, View};
use std::path::PathBuf;
use tray_icon::menu::{
    CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu,
};
use tray_icon::{Icon, TrayIconBuilder};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MB_ICONERROR, MB_OK, MSG, MessageBoxW, MsgWaitForMultipleObjects, PM_REMOVE,
    PeekMessageW, QS_ALLINPUT, TranslateMessage, WM_QUIT,
};

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

fn icon(active: bool) -> Result<Icon> {
    const SIZE: u32 = 32;
    Icon::from_rgba(tray_model::icon_rgba(SIZE, active), SIZE, SIZE).context("building tray icon")
}

pub fn run(daemon: Handle, config_path: PathBuf) -> Result<()> {
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
    let open_cfg = MenuItem::new("Edit settings…", true, None);
    let quit = MenuItem::new("Quit Ember", true, None);

    let menu = Menu::new();
    menu.append_items(&[
        &status,
        &PredefinedMenuItem::separator(),
        &enabled,
        &disable_sub,
        &app,
        &PredefinedMenuItem::separator(),
        &autostart,
        &open_cfg,
        &PredefinedMenuItem::separator(),
        &quit,
    ])?;

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Ember")
        .with_icon(icon(false)?)
        .build()
        .context("creating tray icon")?;

    let action_for = |id: &MenuId| -> Option<Action> {
        if id == enabled.id() {
            Some(Action::ToggleEnabled)
        } else if id == app.id() {
            Some(Action::ToggleAppRule)
        } else if id == autostart.id() {
            Some(Action::ToggleAutostart)
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
    loop {
        // Sleep until a window message arrives or 250ms pass, then drain the queue.
        unsafe { MsgWaitForMultipleObjects(0, std::ptr::null(), 0, 250, QS_ALLINPUT) };
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        while unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
            if msg.message == WM_QUIT {
                return Ok(());
            }
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            let Some(action) = action_for(ev.id()) else {
                continue;
            };
            match tray_model::perform(action, &daemon, &config_path) {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                Err(e) => message_box(&format!("{e:#}")),
            }
            // muda flips check items itself on click; force a resync from the real state.
            last = None;
        }

        let v = tray_model::view(&daemon.state());
        if last.as_ref() != Some(&v) {
            status.set_text(&v.status);
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
