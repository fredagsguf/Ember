//! Global hotkeys written as text, e.g. `Ctrl+Alt+End`. Platform-neutral: the tray front-ends
//! map [`Key`] to Windows virtual-key codes or X11 keysyms and do the registering.

use anyhow::{Result, bail};
use std::fmt;

/// Modifier keys held together with the [`Key`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// The Windows key; Super/Meta on Linux.
    pub win: bool,
}

impl Modifiers {
    pub fn any(&self) -> bool {
        self.ctrl || self.alt || self.shift || self.win
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// `'A'..='Z'`
    Letter(char),
    /// `0..=9`
    Digit(u8),
    /// `1..=24`
    F(u8),
    End,
    Home,
    Insert,
    Delete,
    PageUp,
    PageDown,
    Pause,
    ScrollLock,
    Left,
    Right,
    Up,
    Down,
    Space,
}

/// Named keys, as written in the config. The first spelling is the one [`Display`] uses.
const NAMED: &[(Key, &[&str])] = &[
    (Key::End, &["End"]),
    (Key::Home, &["Home"]),
    (Key::Insert, &["Insert", "Ins"]),
    (Key::Delete, &["Delete", "Del"]),
    (Key::PageUp, &["PageUp", "PgUp"]),
    (Key::PageDown, &["PageDown", "PgDn"]),
    (Key::Pause, &["Pause", "Break"]),
    (Key::ScrollLock, &["ScrollLock"]),
    (Key::Left, &["Left", "ArrowLeft"]),
    (Key::Right, &["Right", "ArrowRight"]),
    (Key::Up, &["Up", "ArrowUp"]),
    (Key::Down, &["Down", "ArrowDown"]),
    (Key::Space, &["Space"]),
];

impl Key {
    /// Keys that no app normally uses, so they're fine as a hotkey on their own.
    fn works_alone(&self) -> bool {
        matches!(self, Key::F(13..) | Key::Pause | Key::ScrollLock)
    }

    fn parse(s: &str) -> Option<Key> {
        let mut chars = s.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            return match c.to_ascii_uppercase() {
                c @ 'A'..='Z' => Some(Key::Letter(c)),
                c @ '0'..='9' => Some(Key::Digit(c as u8 - b'0')),
                _ => None,
            };
        }
        if let Some(n) = s
            .strip_prefix(['F', 'f'])
            .and_then(|n| n.parse::<u8>().ok())
        {
            return (1..=24).contains(&n).then_some(Key::F(n));
        }
        NAMED
            .iter()
            .find(|(_, names)| names.iter().any(|n| n.eq_ignore_ascii_case(s)))
            .map(|&(k, _)| k)
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Key::Letter(c) => write!(f, "{c}"),
            Key::Digit(d) => write!(f, "{d}"),
            Key::F(n) => write!(f, "F{n}"),
            k => {
                let (_, names) = NAMED.iter().find(|(n, _)| n == k).expect("all named");
                f.write_str(names[0])
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    pub mods: Modifiers,
    pub key: Key,
}

impl Hotkey {
    /// Fails if `key` needs a modifier (most keys do, or typing them would trigger it).
    pub fn new(mods: Modifiers, key: Key) -> Result<Hotkey> {
        if !mods.any() && !key.works_alone() {
            bail!("{key} needs a modifier (Ctrl, Alt, Shift or Win), e.g. \"Ctrl+Alt+{key}\"");
        }
        Ok(Hotkey { mods, key })
    }

    /// Parse the config value: an empty string means no hotkey.
    pub fn parse_config(s: &str) -> Result<Option<Hotkey>> {
        if s.trim().is_empty() {
            Ok(None)
        } else {
            s.parse().map(Some)
        }
    }
}

impl std::str::FromStr for Hotkey {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Hotkey> {
        let mut mods = Modifiers::default();
        let mut key = None;
        for part in s.split('+').map(str::trim) {
            let flag = match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => &mut mods.ctrl,
                "alt" => &mut mods.alt,
                "shift" => &mut mods.shift,
                "win" | "super" | "meta" => &mut mods.win,
                "" => bail!("hotkey {s:?} has an empty part; write it like \"Ctrl+Alt+End\""),
                _ => {
                    let Some(k) = Key::parse(part) else {
                        bail!("unknown key {part:?} in hotkey {s:?}");
                    };
                    if key.replace(k).is_some() {
                        bail!("hotkey {s:?} has more than one key; use one key plus modifiers");
                    }
                    continue;
                }
            };
            if std::mem::replace(flag, true) {
                bail!("{part} appears twice in hotkey {s:?}");
            }
        }
        let Some(key) = key else {
            bail!("hotkey {s:?} has no key, only modifiers");
        };
        Hotkey::new(mods, key)
    }
}

impl fmt::Display for Hotkey {
    /// Modifiers in the order Windows writes them (Ctrl+Win+Alt+Shift), then the key.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let m = &self.mods;
        for (on, name) in [
            (m.ctrl, "Ctrl"),
            (m.win, "Win"),
            (m.alt, "Alt"),
            (m.shift, "Shift"),
        ] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        write!(f, "{}", self.key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hk(s: &str) -> Hotkey {
        s.parse().unwrap_or_else(|e| panic!("{s}: {e:#}"))
    }

    #[test]
    fn parses_common_hotkeys() {
        let h = hk("Ctrl+Alt+End");
        assert!(h.mods.ctrl && h.mods.alt && !h.mods.shift && !h.mods.win);
        assert_eq!(h.key, Key::End);
        assert_eq!(hk("Ctrl+Shift+F12").key, Key::F(12));
        assert_eq!(hk("Alt+Pause").key, Key::Pause);
        let w = hk("Win+Shift+E");
        assert!(w.mods.win && w.mods.shift && !w.mods.ctrl);
        assert_eq!(w.key, Key::Letter('E'));
    }

    #[test]
    fn is_case_insensitive_and_accepts_aliases() {
        assert_eq!(hk("control+alt+end"), hk("Ctrl+Alt+End"));
        assert_eq!(hk("CTRL + ALT + END"), hk("Ctrl+Alt+End"));
        assert_eq!(hk("super+e"), hk("Win+E"));
        assert_eq!(hk("Meta+e"), hk("Win+E"));
        assert_eq!(hk("Ctrl+pgdn"), hk("Ctrl+PageDown"));
        assert_eq!(hk("Alt+ArrowLeft").key, Key::Left);
        assert_eq!(hk("Ctrl+Alt+7").key, Key::Digit(7));
        assert_eq!(hk("Ctrl+Space").key, Key::Space);
    }

    #[test]
    fn some_keys_work_without_a_modifier() {
        assert_eq!(hk("F13").key, Key::F(13));
        assert_eq!(hk("F24").key, Key::F(24));
        assert_eq!(hk("Pause").key, Key::Pause);
        assert_eq!(hk("ScrollLock").key, Key::ScrollLock);
        assert!("End".parse::<Hotkey>().is_err());
        assert!("F12".parse::<Hotkey>().is_err());
        assert!("A".parse::<Hotkey>().is_err());
    }

    #[test]
    fn rejects_typos() {
        for bad in [
            "Ctrl+Alt+Ennd",
            "Ctrl+Alt",
            "Ctrl+Alt+End+Home",
            "Ctrl+Ctrl+End",
            "Ctrl++End",
            "Ctrl+F25",
            "Ctrl+F0",
            "Hyper+End",
            "Ctrl+Å",
        ] {
            assert!(bad.parse::<Hotkey>().is_err(), "{bad} should be rejected");
        }
        let e = "Ctrl+Alt+Ennd".parse::<Hotkey>().unwrap_err().to_string();
        assert!(e.contains("Ennd"), "{e}");
    }

    #[test]
    fn display_round_trips() {
        for s in [
            "Ctrl+Alt+End",
            "Ctrl+Shift+F12",
            "Alt+Pause",
            "Win+Shift+E",
            "Ctrl+Win+Alt+Shift+PageUp",
            "F13",
            "ScrollLock",
            "Ctrl+Alt+0",
            "Alt+Space",
            "Ctrl+Down",
        ] {
            let h = hk(s);
            assert_eq!(h.to_string(), s);
            assert_eq!(hk(&h.to_string()), h);
        }
        // Normalised spelling and modifier order.
        assert_eq!(hk("shift+super+e").to_string(), "Win+Shift+E");
        assert_eq!(hk("alt+control+del").to_string(), "Ctrl+Alt+Delete");
    }

    #[test]
    fn empty_config_value_means_no_hotkey() {
        assert_eq!(Hotkey::parse_config("").unwrap(), None);
        assert_eq!(Hotkey::parse_config("  ").unwrap(), None);
        assert_eq!(
            Hotkey::parse_config("Ctrl+Alt+End").unwrap(),
            Some(hk("Ctrl+Alt+End"))
        );
        assert!(Hotkey::parse_config("nope").is_err());
    }
}
