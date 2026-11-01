//! Which window is focused on KDE Plasma (Wayland), native Wayland windows included. KWin only
//! shows its window list to privileged clients, so Ember loads a small KWin script instead (the
//! approach kdotool and ActivityWatch use): KWin runs it, and it calls back into Ember over D-Bus
//! with the PID of every newly activated window.
//!
//! The callback object lives on Ember's session-bus connection and only accepts calls from KWin.
//! The script is loaded under a fixed plugin name and unloaded on exit and before loading, so a
//! copy left behind by a crash never stacks up with a new one.

use super::FocusedWindow;
use anyhow::{Context, Result, bail};
use std::sync::{Arc, Mutex};
use zbus::blocking::Connection;
use zbus::message::Header;

const KWIN: &str = "org.kde.KWin";
const SCRIPTING_PATH: &str = "/Scripting";
const SCRIPTING_INTERFACE: &str = "org.kde.kwin.Scripting";
const SCRIPT_INTERFACE: &str = "org.kde.kwin.Script";
/// Fixed, so `unloadScript` finds a copy left behind by a previous run.
const PLUGIN: &str = "ember-focus";
/// Our callback object, on Ember's own connection.
const OBJECT_PATH: &str = "/io/github/fredagsguf/Ember/Focus";
const INTERFACE: &str = "io.github.fredagsguf.Ember.Focus";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plasma {
    Five,
    Six,
}

/// Where KWin exports a loaded script (with its `run()`/`stop()` methods).
fn script_object_path(plasma: Plasma, id: i32) -> String {
    match plasma {
        Plasma::Six => format!("/Scripting/Script{id}"),
        Plasma::Five => format!("/{id}"),
    }
}

/// `s` as a double-quoted JavaScript string literal.
fn js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The KWin script, calling `ActiveWindowChanged` on `service`. One script serves both Plasma
/// versions: Plasma 6 calls windows "windows" (`windowActivated`, `activeWindow`), Plasma 5 calls
/// them "clients". Plain ES5, since Plasma 5 runs scripts in QtScript. Everything is sent as a
/// string because KWin doesn't say whether a JS number goes out as an int or a double. The
/// focused window is reported again when its title changes or it enters or leaves fullscreen.
fn script_text(service: &str) -> String {
    format!(
        r#"// Loaded by Ember (https://github.com/fredagsguf/Ember) to see which window is focused.
var current = null;
function send() {{
    var w = current;
    callDBus({service}, {path}, {interface}, "ActiveWindowChanged",
        w ? String(w.pid) : "0",
        w ? String(w.resourceClass) : "",
        w ? String(w.caption) : "",
        w && w.fullScreen ? "1" : "0");
}}
function report(w) {{
    if (current) {{
        try {{
            current.captionChanged.disconnect(send);
            current.fullScreenChanged.disconnect(send);
        }} catch (e) {{}}
    }}
    current = w;
    if (w) {{
        try {{
            w.captionChanged.connect(send);
            w.fullScreenChanged.connect(send);
        }} catch (e) {{}}
    }}
    send();
}}
if (workspace.windowActivated) {{
    workspace.windowActivated.connect(report);
    report(workspace.activeWindow);
}} else {{
    workspace.clientActivated.connect(report);
    report(workspace.activeClient);
}}
"#,
        service = js_string(service),
        path = js_string(OBJECT_PATH),
        interface = js_string(INTERFACE),
    )
}

/// The PID the script sent, or `None` for "no window" (nothing focused, or KWin doesn't know
/// the owner, which it reports as 0 or -1).
fn parse_pid(pid: &str) -> Option<u32> {
    pid.trim()
        .parse::<i64>()
        .ok()
        .and_then(|p| u32::try_from(p).ok())
        .filter(|&p| p != 0)
}

/// One report from the script.
#[derive(Debug, Clone, Default)]
struct Report {
    pid: Option<u32>,
    window: FocusedWindow,
}

/// Last report from the script: `None` until the first one arrives.
type Reported = Arc<Mutex<Option<Report>>>;

struct Receiver {
    reported: Reported,
    /// KWin's unique bus name: anyone else on the session bus is ignored.
    kwin: String,
}

#[zbus::interface(name = "io.github.fredagsguf.Ember.Focus")]
impl Receiver {
    fn active_window_changed(
        &self,
        #[zbus(header)] header: Header<'_>,
        pid: String,
        resource_class: String,
        caption: String,
        fullscreen: String,
    ) {
        if header.sender().map(|s| s.as_str()) != Some(self.kwin.as_str()) {
            log::debug!("kwin: ignoring a focus report that didn't come from KWin");
            return;
        }
        let pid = parse_pid(&pid);
        log::debug!("kwin: active window {resource_class:?} (pid {pid:?})");
        let window = FocusedWindow {
            title: (pid.is_some() && !caption.is_empty()).then_some(caption),
            fullscreen: pid.is_some() && fullscreen == "1",
        };
        if let Ok(mut r) = self.reported.lock() {
            *r = Some(Report { pid, window });
        }
    }
}

/// A loaded and running focus script. Unloads it when dropped.
pub struct Script {
    conn: Connection,
    reported: Reported,
}

impl Script {
    /// Load the script into KWin and start it. Fails if KWin's scripting isn't reachable.
    pub fn load(conn: &Connection) -> Result<Script> {
        let kwin: String = call(conn, "/org/freedesktop/DBus", "GetNameOwner", &(KWIN,))
            .context("finding KWin on the session bus")?;
        let service = conn
            .unique_name()
            .context("our D-Bus connection has no name")?
            .to_string();
        let reported = Reported::default();
        conn.object_server()
            .at(
                OBJECT_PATH,
                Receiver {
                    reported: reported.clone(),
                    kwin,
                },
            )
            .context("registering the focus callback")?;
        let me = Script {
            conn: conn.clone(),
            reported,
        };
        // On failure `me` is dropped, which unloads whatever got loaded.
        let plasma = me.start(&service)?;
        let plasma = match plasma {
            Plasma::Six => "Plasma 6",
            Plasma::Five => "Plasma 5",
        };
        log::info!("kwin: a KWin script reports the focused window ({plasma} API)");
        Ok(me)
    }

    fn start(&self, service: &str) -> Result<Plasma> {
        let dir = crate::config::state_dir();
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let file = dir.join("kwin-focus.js");
        std::fs::write(&file, script_text(service))
            .with_context(|| format!("writing {}", file.display()))?;
        let file = file.to_str().context("state directory path isn't UTF-8")?;

        self.unload();
        let id: i32 = self
            .scripting("loadScript", &(file, PLUGIN))
            .context("loading the script (is KWin scripting available?)")?;
        if id < 0 {
            bail!("KWin refused to load the script");
        }
        let mut errors = Vec::new();
        for plasma in [Plasma::Six, Plasma::Five] {
            let path = script_object_path(plasma, id);
            match self.conn.call_method(
                Some(KWIN),
                path.as_str(),
                Some(SCRIPT_INTERFACE),
                "run",
                &(),
            ) {
                Ok(_) => return Ok(plasma),
                Err(e) => errors.push(format!("{path}: {e}")),
            }
        }
        bail!("starting the script failed: {}", errors.join("; "))
    }

    /// `None` until the script has reported, then the focused window's PID (`None` = no window).
    pub fn focused_pid(&self) -> Option<Option<u32>> {
        Some(self.reported.lock().ok()?.as_ref()?.pid)
    }

    /// `None` until the script has reported, then the focused window's title and state.
    pub fn focused_window(&self) -> Option<FocusedWindow> {
        Some(self.reported.lock().ok()?.as_ref()?.window.clone())
    }

    fn scripting<B, R>(&self, method: &str, body: &B) -> Result<R>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
        R: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
    {
        let reply = self
            .conn
            .call_method(
                Some(KWIN),
                SCRIPTING_PATH,
                Some(SCRIPTING_INTERFACE),
                method,
                body,
            )
            .with_context(|| format!("calling {SCRIPTING_INTERFACE}.{method}"))?;
        Ok(reply.body().deserialize()?)
    }

    fn unload(&self) {
        match self.scripting::<_, bool>("unloadScript", &(PLUGIN,)) {
            Ok(true) => log::debug!("kwin: unloaded focus script"),
            Ok(false) => {}
            Err(e) => log::debug!("kwin: unloading focus script failed: {e:#}"),
        }
    }
}

/// A call to the bus daemon itself.
fn call<B, R>(conn: &Connection, path: &str, method: &str, body: &B) -> Result<R>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
    R: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
{
    let reply = conn.call_method(
        Some("org.freedesktop.DBus"),
        path,
        Some("org.freedesktop.DBus"),
        method,
        body,
    )?;
    Ok(reply.body().deserialize()?)
}

impl Drop for Script {
    fn drop(&mut self) {
        self.unload();
        let _ = self.conn.object_server().remove::<Receiver, _>(OBJECT_PATH);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::object_server::Interface;

    #[test]
    fn script_object_paths() {
        assert_eq!(script_object_path(Plasma::Six, 3), "/Scripting/Script3");
        assert_eq!(script_object_path(Plasma::Five, 3), "/3");
    }

    #[test]
    fn script_handles_both_plasma_versions() {
        let s = script_text(":1.42");
        assert!(s.contains(r#"callDBus(":1.42", "/io/github/fredagsguf/Ember/Focus""#));
        assert!(s.contains(r#""io.github.fredagsguf.Ember.Focus", "ActiveWindowChanged""#));
        assert!(s.contains("workspace.windowActivated.connect(report)"));
        assert!(s.contains("report(workspace.activeWindow)"));
        assert!(s.contains("workspace.clientActivated.connect(report)"));
        assert!(s.contains("report(workspace.activeClient)"));
        assert!(s.contains("w.captionChanged.connect(send)"));
        assert!(s.contains("w.fullScreenChanged.connect(send)"));
        // QtScript (Plasma 5) predates ES6.
        assert!(!s.contains("const ") && !s.contains("let ") && !s.contains("=>"));
    }

    #[test]
    fn js_strings_are_escaped() {
        assert_eq!(js_string(r#"a"b\c"#), r#""a\"b\\c""#);
        assert_eq!(js_string("x\ny"), r#""x\ny""#);
    }

    #[test]
    fn pids() {
        assert_eq!(parse_pid("1234"), Some(1234));
        assert_eq!(parse_pid(" 77 "), Some(77));
        assert_eq!(parse_pid("0"), None);
        assert_eq!(parse_pid("-1"), None);
        assert_eq!(parse_pid(""), None);
        assert_eq!(parse_pid("undefined"), None);
    }

    #[test]
    fn interface_name_matches_script() {
        assert_eq!(Receiver::name().as_str(), INTERFACE);
    }
}
