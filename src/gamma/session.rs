//! Which Linux display server and desktop Ember is running under, from the environment.

#[derive(Debug, Clone, PartialEq)]
pub enum Desktop {
    Kde,
    Gnome,
    /// Anything else, as named by `XDG_CURRENT_DESKTOP` (empty if unset).
    Other(String),
}

/// A Wayland session is one with a Wayland socket to talk to, or that says it is one.
/// `DISPLAY` is ignored: Wayland desktops set it too (for XWayland), but XWayland can't change
/// the real screen's gamma or see native windows.
pub fn is_wayland(wayland_display: Option<&str>, session_type: Option<&str>) -> bool {
    wayland_display.is_some_and(|d| !d.trim().is_empty())
        || session_type.is_some_and(|t| t.trim().eq_ignore_ascii_case("wayland"))
}

/// `XDG_CURRENT_DESKTOP` is a colon-separated list, e.g. `KDE`, `ubuntu:GNOME`, `sway`.
pub fn desktop(xdg_current_desktop: Option<&str>) -> Desktop {
    let raw = xdg_current_desktop.unwrap_or("").trim();
    let parts = || raw.split(':').map(|p| p.trim().to_ascii_lowercase());
    if parts().any(|p| p == "kde" || p == "plasma") {
        Desktop::Kde
    } else if parts().any(|p| p.starts_with("gnome")) {
        Desktop::Gnome
    } else {
        Desktop::Other(raw.to_string())
    }
}

/// Why Ember can't run on this Wayland desktop, and what the user can do instead.
/// On KDE this is a prefix for the error from KWin.
pub fn unsupported_message(desktop: &Desktop) -> String {
    match desktop {
        Desktop::Gnome => {
            "GNOME on Wayland doesn't let apps change the screen colours. Use GNOME's \
             built-in Night Light (Settings → Displays → Night Light) instead, or log in to an X11 \
             session if your distribution still offers one."
                .to_string()
        }
        Desktop::Kde => "on Plasma Wayland Ember works through KDE's Night Light (Plasma 5.18 or \
             newer), which failed"
            .to_string(),
        Desktop::Other(name) => {
            let name = if name.is_empty() {
                "this Wayland compositor".to_string()
            } else {
                format!("this Wayland compositor ({name})")
            };
            format!(
                "{name} doesn't let apps change the screen colours. On Wayland Ember supports \
                 wlroots-based compositors with wlr-gamma-control (sway, Hyprland, river, Wayfire, \
                 labwc…) and KDE Plasma; it also works in X11 sessions."
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wayland_detection() {
        assert!(is_wayland(Some("wayland-0"), None));
        assert!(is_wayland(None, Some("wayland")));
        assert!(is_wayland(Some("wayland-1"), Some("x11")));
        assert!(!is_wayland(None, Some("x11")));
        assert!(!is_wayland(Some(""), Some("tty")));
        assert!(!is_wayland(None, None));
    }

    #[test]
    fn desktop_detection() {
        assert_eq!(desktop(Some("KDE")), Desktop::Kde);
        assert_eq!(desktop(Some("ubuntu:GNOME")), Desktop::Gnome);
        assert_eq!(desktop(Some("GNOME-Classic:GNOME")), Desktop::Gnome);
        assert_eq!(desktop(Some("sway")), Desktop::Other("sway".into()));
        assert_eq!(desktop(None), Desktop::Other(String::new()));
    }

    #[test]
    fn messages_point_somewhere_useful() {
        assert!(unsupported_message(&Desktop::Gnome).contains("Night Light"));
        assert!(unsupported_message(&Desktop::Kde).contains("Night Light"));
        let other = unsupported_message(&Desktop::Other("weston".into()));
        assert!(other.contains("weston") && other.contains("sway"));
    }
}
