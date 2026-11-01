//! KDE Plasma on Wayland. KWin doesn't let clients set gamma ramps, so Ember drives KWin's own
//! Night Light over D-Bus instead: KWin keeps choosing the colour and its schedule (System
//! Settings → Night Light), and Ember suspends it with `inhibit()` whenever Ember itself would show
//! neutral colours (a disable rule matches, Ember is paused, or it's daytime by Ember's schedule).
//!
//! `preview(temperature)` exists too, but it pops up an on-screen notice on every call and only
//! lasts 15 seconds, so it isn't usable for holding a colour all evening.
//!
//! Inhibitions belong to our D-Bus connection: KWin drops them if Ember exits or crashes.

use super::Backend;
use super::x11;
use crate::color::{ColorSetting, NEUTRAL_K};
use anyhow::{Context, Result, anyhow, bail};
use zbus::blocking::Connection;
use zbus::zvariant::OwnedValue;

const SERVICE: &str = "org.kde.KWin";
/// (object path, interface): Plasma 6 first, then the Plasma 5.18+ name of the same API.
const INTERFACES: &[(&str, &str)] = &[
    ("/org/kde/KWin/NightLight", "org.kde.KWin.NightLight"),
    ("/ColorCorrect", "org.kde.kwin.ColorCorrect"),
];

pub struct NightLight {
    conn: Connection,
    path: &'static str,
    interface: &'static str,
    /// Held while Night Light is suspended.
    cookie: Option<u32>,
    /// KWin publishes the active XWayland window (games under Proton/Wine, most Steam games) via
    /// `_NET_ACTIVE_WINDOW`; native Wayland windows can't be identified.
    focus: Option<x11::Focus>,
}

/// Ember wants "no tint", so KWin's Night Light should be suspended.
fn wants_inhibit(setting: ColorSetting) -> bool {
    let s = setting.clamped();
    s.temperature == NEUTRAL_K && s.brightness >= 1.0
}

impl NightLight {
    pub fn open() -> Result<NightLight> {
        let conn = Connection::session().context("connecting to the D-Bus session bus")?;
        let mut errors = Vec::new();
        for &(path, interface) in INTERFACES {
            let mut me = NightLight {
                conn: conn.clone(),
                path,
                interface,
                cookie: None,
                focus: None,
            };
            match me.property_bool("available") {
                Ok(true) => {}
                Ok(false) => bail!(
                    "KWin says Night Light isn't available on this system (the graphics driver may \
                     not support gamma ramps)"
                ),
                Err(e) => {
                    errors.push(format!("{interface}: {e:#}"));
                    continue;
                }
            }
            if !me.property_bool("enabled").unwrap_or(true) {
                bail!(
                    "KDE's Night Light is switched off. On Plasma Wayland Ember works through it: \
                     turn it on in System Settings → Display & Monitor → Night Light, then restart \
                     Ember"
                );
            }
            me.focus = match x11::Focus::connect() {
                Ok(f) => Some(f),
                Err(e) => {
                    log::warn!(
                        "kwin: no XWayland connection ({e:#}); rules with when = \"focused\" won't work"
                    );
                    None
                }
            };
            log::info!(
                "kwin: driving KWin Night Light via {interface}. Colours and schedule come from \
                 System Settings; \"focused\" rules only see X11/XWayland apps"
            );
            return Ok(me);
        }
        Err(anyhow!(errors.join("; ")))
            .context("KWin's Night Light D-Bus interface isn't reachable")
    }

    fn call<B, R>(&self, interface: &str, method: &str, body: &B) -> Result<R>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
        R: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
    {
        let reply = self
            .conn
            .call_method(Some(SERVICE), self.path, Some(interface), method, body)
            .with_context(|| format!("calling {interface}.{method}"))?;
        Ok(reply.body().deserialize()?)
    }

    fn property_bool(&self, name: &str) -> Result<bool> {
        let v: OwnedValue = self.call(
            "org.freedesktop.DBus.Properties",
            "Get",
            &(self.interface, name),
        )?;
        Ok(bool::try_from(v)?)
    }

    fn uninhibit(&mut self) -> Result<()> {
        if let Some(cookie) = self.cookie.take() {
            self.call::<_, ()>(self.interface, "uninhibit", &(cookie,))?;
            log::debug!("kwin: night light resumed");
        }
        Ok(())
    }
}

impl Backend for NightLight {
    fn name(&self) -> &'static str {
        "kwin-night-light"
    }

    fn apply(&mut self, setting: ColorSetting) -> Result<()> {
        if !wants_inhibit(setting) {
            return self.uninhibit();
        }
        if self.cookie.is_none() {
            let cookie: u32 = self.call(self.interface, "inhibit", &())?;
            self.cookie = Some(cookie);
            log::debug!("kwin: night light suspended");
        }
        Ok(())
    }

    /// Hand control back to KWin's own schedule.
    fn restore(&mut self) -> Result<()> {
        self.uninhibit()
    }

    fn focused_pid(&mut self) -> Option<u32> {
        self.focus.as_ref()?.focused_pid()
    }

    /// Slider values can't be shown on screen: KWin picks the colour itself.
    fn live_preview(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_neutral_suspends_night_light() {
        assert!(wants_inhibit(ColorSetting::NEUTRAL));
        let warm = ColorSetting {
            temperature: 3400,
            brightness: 1.0,
        };
        assert!(!wants_inhibit(warm));
        let dim = ColorSetting {
            temperature: NEUTRAL_K,
            brightness: 0.8,
        };
        assert!(!wants_inhibit(dim));
    }
}
