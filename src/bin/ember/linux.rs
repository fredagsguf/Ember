//! Linux tray icon via the StatusNotifierItem D-Bus protocol (KDE, XFCE, Cinnamon, sway/waybar,
//! GNOME with the AppIndicator extension).

use anyhow::{Result, anyhow};
use ember::daemon::Handle;
use ember::tray_model::{self, Action, DISABLE_OPTIONS, View};
use ksni::blocking::TrayMethods;
use ksni::menu::{CheckmarkItem, MenuItem, StandardItem, SubMenu};
use std::path::PathBuf;
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::time::Duration;

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
    for p in rgba.chunks_exact(4) {
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
                activate: Box::new(|t: &mut Self| t.act(Action::OpenSettings)),
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
