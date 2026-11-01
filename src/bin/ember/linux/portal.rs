//! The toggle hotkey on Wayland, through the XDG desktop portal's GlobalShortcuts interface
//! (KDE Plasma 6, Hyprland, GNOME 48+; not sway). Wayland apps can't grab keys, so Ember asks the
//! desktop to bind a shortcut, suggesting the configured key. The desktop may ask the user to
//! confirm it or pick another key, and signals `Activated` whenever it's pressed.
//!
//! Portal calls that need the user answer later: the method returns a request object path and
//! the result arrives as a `Response` signal on it. We subscribe to all portal signals before
//! making any call, so no response can be missed.

use super::EmberTray;
use anyhow::{Context, Result, bail};
use ember::daemon::Handle;
use ember::hotkey::{Hotkey, Key};
use ember::tray_model::{self, Action};
use std::collections::HashMap;
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::Duration;
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Type;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};
use zbus::{MatchRule, Message};

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SHORTCUTS: &str = "org.freedesktop.portal.GlobalShortcuts";
const REQUEST: &str = "org.freedesktop.portal.Request";
/// Must match `ember.desktop` in `contrib/`: desktops use it to name and remember our shortcut.
const APP_ID: &str = "ember";
const SHORTCUT_ID: &str = "toggle";
const DESCRIPTION: &str = "Pause or resume Ember";

/// A list of shortcuts as the portal sends them: `a(sa{sv})`, id and properties.
type Shortcuts = Vec<(String, HashMap<String, OwnedValue>)>;

/// The hotkey in the XDG shortcuts format the portal takes as `preferred_trigger`, e.g.
/// `CTRL+ALT+End`: xkbcommon modifier names, then the key's xkb keysym name (letters in lower
/// case, as shortcuts are on the keyboard's base level).
fn xdg_trigger(h: Hotkey) -> String {
    let m = h.mods;
    let key = match h.key {
        Key::Letter(c) => c.to_ascii_lowercase().to_string(),
        Key::PageUp => "Page_Up".into(),
        Key::PageDown => "Page_Down".into(),
        Key::ScrollLock => "Scroll_Lock".into(),
        Key::Space => "space".into(),
        // Digits, F1–F24, End, Home, Insert, Delete, Pause and the arrows are spelt the same.
        k => k.to_string(),
    };
    [
        (m.ctrl, "CTRL"),
        (m.alt, "ALT"),
        (m.shift, "SHIFT"),
        (m.win, "LOGO"),
    ]
    .into_iter()
    .filter(|&(on, _)| on)
    .map(|(_, name)| name)
    .chain([key.as_str()])
    .collect::<Vec<_>>()
    .join("+")
}

/// `session_handle` from a `CreateSession` response. The spec says it's an object path, but
/// portals send it as a string.
fn session_handle(results: &HashMap<String, OwnedValue>) -> Option<String> {
    match unwrap_variant(results.get("session_handle")?) {
        Value::Str(s) => Some(s.to_string()),
        Value::ObjectPath(p) => Some(p.to_string()),
        _ => None,
    }
}

/// The value inside a `v` (variant), however deeply it's wrapped.
fn unwrap_variant<'a>(mut v: &'a Value<'a>) -> &'a Value<'a> {
    while let Value::Value(inner) = v {
        v = inner;
    }
    v
}

/// `shortcuts` from a `BindShortcuts` response: the ones the desktop actually bound.
fn bound_shortcuts(results: &HashMap<String, OwnedValue>) -> Result<Shortcuts> {
    match results.get("shortcuts") {
        None => Ok(Vec::new()),
        Some(v) => Ok(Shortcuts::try_from(unwrap_variant(v).try_to_owned()?)?),
    }
}

/// How to trigger our shortcut, as the desktop describes it (`""` if it doesn't say), or `None`
/// if it isn't bound.
fn toggle_trigger(shortcuts: &Shortcuts) -> Option<String> {
    let (_, props) = shortcuts.iter().find(|(id, _)| id == SHORTCUT_ID)?;
    Some(
        match props.get("trigger_description").map(|v| unwrap_variant(v)) {
            Some(Value::Str(s)) => s.to_string(),
            _ => String::new(),
        },
    )
}

/// The bound trigger as a [`Hotkey`] for the tray menu, if the desktop's description reads as
/// one (KDE says e.g. `Ctrl+Alt+End`).
fn shown_hotkey(trigger_description: &str) -> Option<Hotkey> {
    trigger_description.trim().parse().ok()
}

/// Errors meaning the portal doesn't have this method or interface at all.
fn is_unknown_method(e: &zbus::Error) -> bool {
    use zbus::fdo::Error as Fdo;
    match e {
        zbus::Error::MethodError(name, _, _) => matches!(
            name.as_str(),
            "org.freedesktop.DBus.Error.UnknownMethod"
                | "org.freedesktop.DBus.Error.UnknownInterface"
                | "org.freedesktop.DBus.Error.UnknownObject"
        ),
        zbus::Error::FDO(e) => matches!(
            **e,
            Fdo::UnknownMethod(_) | Fdo::UnknownInterface(_) | Fdo::UnknownObject(_)
        ),
        _ => false,
    }
}

/// Which call a pending request belongs to.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Step {
    CreateSession,
    BindShortcuts,
}

struct Portal {
    conn: Connection,
    next_token: u32,
    /// The hotkey we're binding or have bound.
    hotkey: Option<Hotkey>,
    /// Request object path we're waiting for a `Response` on.
    pending: Option<(String, Step)>,
    session: Option<String>,
    /// `CreateSession` requests given up on after a config change, whose sessions to close.
    abandoned: Vec<String>,
}

impl Portal {
    fn call<B>(&self, path: &str, interface: &str, method: &str, body: &B) -> zbus::Result<Message>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
    {
        self.conn
            .call_method(Some(PORTAL), path, Some(interface), method, body)
    }

    /// A fresh `handle_token` (the last element of the request or session object path).
    fn token(&mut self) -> String {
        self.next_token += 1;
        format!("ember{}", self.next_token)
    }

    /// Host apps (not Flatpak) tell newer portals (1.19+) who they are before anything else, so
    /// the desktop can name the shortcut and remember it. Older portals don't have this.
    fn register(&self) {
        if std::path::Path::new("/.flatpak-info").exists() {
            return; // Flatpak identifies sandboxed apps itself
        }
        let options = HashMap::<&str, Value>::new();
        match self.call(
            PORTAL_PATH,
            "org.freedesktop.host.portal.Registry",
            "Register",
            &(APP_ID, options),
        ) {
            Ok(_) => log::debug!("portal: registered as {APP_ID:?}"),
            Err(e) if is_unknown_method(&e) => {
                log::debug!("portal: no app registry (xdg-desktop-portal older than 1.19)")
            }
            Err(e) => log::warn!("portal: could not register as {APP_ID:?}: {e}"),
        }
    }

    fn version(&self) -> Result<u32> {
        let reply = self.call(
            PORTAL_PATH,
            "org.freedesktop.DBus.Properties",
            "Get",
            &(SHORTCUTS, "version"),
        )?;
        let v: OwnedValue = reply.body().deserialize()?;
        Ok(u32::try_from(v)?)
    }

    /// Follow a config change: drop the current shortcut and bind the new one, if any. A session
    /// can only bind once, so changing the key means a new session.
    fn set(&mut self, hotkey: Option<Hotkey>, tray: &ksni::blocking::Handle<EmberTray>) {
        if let Some((request, Step::CreateSession)) = self.pending.take() {
            self.abandoned.push(request);
        }
        if let Some(session) = self.session.take() {
            self.close(&session);
            tray.update(|t| t.hotkey_bound = None);
        }
        self.hotkey = hotkey;
        let Some(h) = hotkey else {
            log::info!("toggle hotkey off");
            return;
        };
        if let Err(e) = self.create_session() {
            log::warn!("could not set up the toggle hotkey {h} with the desktop portal: {e:#}");
        }
    }

    /// Closing a session unbinds its shortcuts.
    fn close(&self, session: &str) {
        if let Err(e) = self.call(session, "org.freedesktop.portal.Session", "Close", &()) {
            log::debug!("portal: closing shortcut session {session}: {e}");
        }
    }

    fn create_session(&mut self) -> Result<()> {
        let token = self.token();
        let options = HashMap::from([
            ("handle_token", Value::from(token.as_str())),
            ("session_handle_token", Value::from(token.as_str())),
        ]);
        let reply = self
            .call(PORTAL_PATH, SHORTCUTS, "CreateSession", &(options,))
            .context("CreateSession")?;
        let request: OwnedObjectPath = reply.body().deserialize()?;
        self.pending = Some((request.to_string(), Step::CreateSession));
        Ok(())
    }

    fn bind(&mut self, session: &str, h: Hotkey) -> Result<()> {
        let trigger = xdg_trigger(h);
        let shortcuts = vec![(
            SHORTCUT_ID,
            HashMap::from([
                ("description", Value::from(DESCRIPTION)),
                ("preferred_trigger", Value::from(trigger.as_str())),
            ]),
        )];
        let token = self.token();
        let options = HashMap::from([("handle_token", Value::from(token.as_str()))]);
        let session_path = ObjectPath::try_from(session)?;
        let reply = self
            .call(
                PORTAL_PATH,
                SHORTCUTS,
                "BindShortcuts",
                // No parent window: Ember has none open (the settings window is another process).
                &(session_path, shortcuts, "", options),
            )
            .context("BindShortcuts")?;
        let request: OwnedObjectPath = reply.body().deserialize()?;
        log::debug!("portal: asked to bind {h} as {trigger}");
        self.pending = Some((request.to_string(), Step::BindShortcuts));
        Ok(())
    }

    /// Our shortcut is bound (or rebound in the desktop's settings) as `trigger`.
    fn bound(&self, trigger: &str, tray: &ksni::blocking::Handle<EmberTray>) {
        if trigger.is_empty() {
            log::info!(
                "toggle hotkey bound by the desktop portal; assign a key to \"{DESCRIPTION}\" in \
                 your desktop's settings (Hyprland: bind = CTRL ALT, End, global, \
                 {APP_ID}:{SHORTCUT_ID})"
            );
        } else {
            log::info!("toggle hotkey: {trigger} (via the desktop portal)");
        }
        let shown = shown_hotkey(trigger);
        tray.update(|t| t.hotkey_bound = shown);
    }

    /// Handle one portal signal. Returns false once the tray is gone.
    fn handle(&mut self, msg: &Message, tray: &ksni::blocking::Handle<EmberTray>) -> Result<bool> {
        let header = msg.header();
        let (Some(path), Some(interface), Some(member)) =
            (header.path(), header.interface(), header.member())
        else {
            return Ok(true);
        };
        match (interface.as_str(), member.as_str()) {
            (REQUEST, "Response") => {
                let ours = |request: &String| request.as_str() == path.as_str();
                let step = self.pending.take_if(|(request, _)| ours(request));
                let abandoned = self.abandoned.iter().position(ours);
                if step.is_none() && abandoned.is_none() {
                    return Ok(true); // someone else's request, or a bind we've given up on
                }
                let (code, results): (u32, HashMap<String, OwnedValue>) =
                    msg.body().deserialize()?;
                if let Some(i) = abandoned {
                    self.abandoned.remove(i);
                    if let Some(session) = session_handle(&results).filter(|_| code == 0) {
                        self.close(&session);
                    }
                } else if let Some((_, step)) = step {
                    self.response(step, code, &results, tray)?;
                }
            }
            (SHORTCUTS, "Activated") => {
                let (session, id, _, _): (
                    OwnedObjectPath,
                    String,
                    u64,
                    HashMap<String, OwnedValue>,
                ) = msg.body().deserialize()?;
                if self.session.as_deref() == Some(session.as_str())
                    && id == SHORTCUT_ID
                    && tray.update(|t| t.act(Action::ToggleEnabled)).is_none()
                {
                    return Ok(false); // tray service is gone
                }
            }
            (SHORTCUTS, "ShortcutsChanged") => {
                let (session, shortcuts): (OwnedObjectPath, Shortcuts) =
                    msg.body().deserialize()?;
                if self.session.as_deref() == Some(session.as_str()) {
                    match toggle_trigger(&shortcuts) {
                        Some(trigger) => self.bound(&trigger, tray),
                        None => {
                            log::info!("toggle hotkey removed in the desktop's settings");
                            tray.update(|t| t.hotkey_bound = None);
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(true)
    }

    fn response(
        &mut self,
        step: Step,
        code: u32,
        results: &HashMap<String, OwnedValue>,
        tray: &ksni::blocking::Handle<EmberTray>,
    ) -> Result<()> {
        let Some(h) = self.hotkey else {
            return Ok(());
        };
        // 0 = done, 1 = the user cancelled, 2 = ended some other way.
        match (step, code) {
            (Step::CreateSession, 0) => {
                let session =
                    session_handle(results).context("CreateSession gave no session handle")?;
                self.session = Some(session.clone());
                self.bind(&session, h)?;
            }
            (Step::CreateSession, _) => {
                bail!("the desktop portal refused a shortcut session (response {code})")
            }
            (Step::BindShortcuts, 0) => match toggle_trigger(&bound_shortcuts(results)?) {
                Some(trigger) => self.bound(&trigger, tray),
                None => log::warn!("the desktop didn't bind the toggle hotkey {h}"),
            },
            (Step::BindShortcuts, 1) => log::info!(
                "toggle hotkey {h} not set: cancelled in the desktop's dialog (restart Ember to be \
                 asked again)"
            ),
            (Step::BindShortcuts, _) => {
                log::warn!("the desktop portal didn't bind the toggle hotkey {h} (response {code})")
            }
        }
        Ok(())
    }
}

/// Toggle Ember when the desktop reports our shortcut, following config changes. Returns when
/// the tray goes away. Without a portal that has GlobalShortcuts it logs and returns.
pub fn watch_hotkey(daemon: &Handle, tray: &ksni::blocking::Handle<EmberTray>) -> Result<()> {
    let conn = match Connection::session() {
        Ok(c) => c,
        Err(e) => {
            log::info!("no toggle hotkey: can't connect to the D-Bus session bus ({e})");
            return Ok(());
        }
    };
    let mut portal = Portal {
        conn,
        next_token: 0,
        hotkey: None,
        pending: None,
        session: None,
        abandoned: Vec::new(),
    };
    portal.register();
    match portal.version() {
        Ok(v) => log::debug!("portal: GlobalShortcuts version {v}"),
        Err(e) => {
            log::info!(
                "no toggle hotkey: this desktop doesn't offer global shortcuts to apps (the XDG \
                 desktop portal has no GlobalShortcuts, e.g. on sway); pause from the tray \
                 instead ({e:#})"
            );
            return Ok(());
        }
    }

    // Request responses come from objects under the portal path, shortcut signals from the
    // portal object itself. A thread hands them over so this one can also watch the config.
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .path_namespace(PORTAL_PATH)?
        .build();
    let signals = MessageIterator::for_match_rule(rule, &portal.conn, None)
        .context("subscribing to desktop portal signals")?;
    let (tx, rx) = channel();
    std::thread::Builder::new()
        .name("ember-portal".into())
        .spawn(move || {
            for msg in signals {
                match msg {
                    Ok(m) => {
                        if tx.send(m).is_err() {
                            return; // hotkey thread is done
                        }
                    }
                    Err(e) => log::debug!("portal: bad signal: {e}"),
                }
            }
        })
        .context("spawning portal signal thread")?;

    let mut wanted: Option<Hotkey> = None;
    loop {
        let h = tray_model::hotkey(&daemon.state());
        if h != wanted {
            wanted = h;
            portal.set(h, tray);
        }
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(msg) => match portal.handle(&msg, tray) {
                Ok(true) => {}
                Ok(false) => return Ok(()),
                Err(e) => log::warn!("toggle hotkey: {e:#}"),
            },
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => bail!("lost the D-Bus connection"),
        }
        if tray.is_closed() {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::{LE, serialized::Context, to_bytes};

    fn hk(s: &str) -> Hotkey {
        s.parse().unwrap()
    }

    /// `results` from a `Response` signal, through real D-Bus encoding.
    fn results(entries: HashMap<&str, Value>) -> HashMap<String, OwnedValue> {
        let data = to_bytes(Context::new_dbus(LE, 0), &(0u32, entries)).unwrap();
        let ((_, results), _): ((u32, HashMap<String, OwnedValue>), _) =
            data.deserialize().unwrap();
        results
    }

    #[test]
    fn triggers_use_the_xdg_shortcuts_format() {
        assert_eq!(xdg_trigger(hk("Ctrl+Alt+End")), "CTRL+ALT+End");
        assert_eq!(xdg_trigger(hk("Win+Shift+E")), "SHIFT+LOGO+e");
        assert_eq!(xdg_trigger(hk("Ctrl+Shift+F12")), "CTRL+SHIFT+F12");
        assert_eq!(xdg_trigger(hk("Ctrl+PageDown")), "CTRL+Page_Down");
        assert_eq!(xdg_trigger(hk("Alt+PgUp")), "ALT+Page_Up");
        assert_eq!(xdg_trigger(hk("Ctrl+Alt+7")), "CTRL+ALT+7");
        assert_eq!(xdg_trigger(hk("Alt+Space")), "ALT+space");
        assert_eq!(xdg_trigger(hk("Ctrl+Delete")), "CTRL+Delete");
        assert_eq!(xdg_trigger(hk("Super+Left")), "LOGO+Left");
        assert_eq!(xdg_trigger(hk("F13")), "F13");
        assert_eq!(xdg_trigger(hk("ScrollLock")), "Scroll_Lock");
        assert_eq!(xdg_trigger(hk("Pause")), "Pause");
    }

    #[test]
    fn reads_the_session_handle_as_string_or_path() {
        let s = "/org/freedesktop/portal/desktop/session/1_42/ember1";
        let r = results(HashMap::from([("session_handle", Value::from(s))]));
        assert_eq!(session_handle(&r).as_deref(), Some(s));
        let path = ObjectPath::try_from(s).unwrap();
        let r = results(HashMap::from([("session_handle", Value::from(path))]));
        assert_eq!(session_handle(&r).as_deref(), Some(s));
        assert_eq!(session_handle(&results(HashMap::new())), None);
    }

    #[test]
    fn reads_what_the_desktop_bound() {
        let shortcut = |id: &'static str, trigger: Option<&'static str>| {
            let mut props = HashMap::from([("description", Value::from(DESCRIPTION))]);
            if let Some(t) = trigger {
                props.insert("trigger_description", Value::from(t));
            }
            (id, props)
        };
        let bound = |list: Vec<(&'static str, HashMap<&'static str, Value<'static>>)>| {
            let r = results(HashMap::from([("shortcuts", Value::new(list))]));
            toggle_trigger(&bound_shortcuts(&r).unwrap())
        };
        assert_eq!(
            bound(vec![shortcut("toggle", Some("Ctrl+Alt+End"))]).as_deref(),
            Some("Ctrl+Alt+End")
        );
        assert_eq!(bound(vec![shortcut("toggle", None)]).as_deref(), Some(""));
        assert_eq!(bound(vec![shortcut("other", Some("Ctrl+A"))]), None);
        assert_eq!(bound(vec![]), None);
        assert_eq!(
            toggle_trigger(&bound_shortcuts(&results(HashMap::new())).unwrap()),
            None
        );
    }

    #[test]
    fn shows_triggers_that_read_as_hotkeys() {
        assert_eq!(shown_hotkey("Ctrl+Alt+End"), Some(hk("Ctrl+Alt+End")));
        assert_eq!(shown_hotkey("Meta+Shift+E"), Some(hk("Win+Shift+E")));
        assert_eq!(shown_hotkey(""), None);
        assert_eq!(shown_hotkey("Press Ctrl+Alt+End"), None);
    }

    #[test]
    fn recognises_missing_methods() {
        let unknown = zbus::Error::FDO(Box::new(zbus::fdo::Error::UnknownMethod("x".into())));
        assert!(is_unknown_method(&unknown));
        let denied = zbus::Error::FDO(Box::new(zbus::fdo::Error::AccessDenied("x".into())));
        assert!(!is_unknown_method(&denied));
    }
}
