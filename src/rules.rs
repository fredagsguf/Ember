//! Per-application rules: "when cs2 is focused, turn Ember off".

use crate::color::ColorSetting;
use crate::config::{Rule, When};
use std::collections::HashSet;
use std::ffi::OsString;
use std::path::Path;

/// Normalise a process name for comparison: lowercase, no ".exe" suffix, no path.
pub fn normalize(name: &str) -> String {
    let base = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .trim()
        .to_lowercase();
    base.strip_suffix(".exe").map(str::to_owned).unwrap_or(base)
}

/// What the rule engine needs to know about the system right now.
#[derive(Debug, Default, Clone)]
pub struct Snapshot {
    /// Normalised names of all running processes.
    pub running: HashSet<String>,
    /// Normalised name of the process owning the foreground window, if known.
    pub focused: Option<String>,
    /// Title of the foreground window, if known.
    pub title: Option<String>,
    /// The foreground window covers its whole screen.
    pub fullscreen: bool,
    /// The foreground window belongs to a game started by Steam.
    pub steam_focused: bool,
    /// A game started by Steam is running.
    pub steam_running: bool,
}

/// Steam's own processes, which live next to its games but aren't one.
const STEAM_CLIENT: &[&str] = &[
    "steam",
    "steamwebhelper",
    "steamservice",
    "steamerrorreporter",
    "gameoverlayui",
    "gameoverlayui64",
    // Compiles a game's shaders in the background, with that game's SteamAppId.
    "fossilize_replay",
];

/// Is this process (normalised `name`, executable path, environment) a game started by Steam?
/// Games are installed under `steamapps/common`, and Steam gives what it launches a
/// `SteamAppId` (or, for non-Steam shortcuts, `SteamGameId`). The environment catches Proton
/// games inside Steam's container, whose executable path the host can't see.
pub fn is_steam_game(name: &str, exe: Option<&Path>, environ: &[OsString]) -> bool {
    if STEAM_CLIENT.contains(&name) {
        return false;
    }
    let installed = exe.is_some_and(|p| {
        p.to_string_lossy()
            .replace('\\', "/")
            .to_lowercase()
            .contains("/steamapps/common/")
    });
    let launched = environ.iter().any(|v| {
        let v = v.to_string_lossy();
        v.strip_prefix("SteamAppId=")
            .or_else(|| v.strip_prefix("SteamGameId="))
            .is_some_and(|id| !id.is_empty() && id != "0")
    });
    installed || launched
}

/// Linux truncates process names (`comm`) to 15 bytes.
const LINUX_COMM_LEN: usize = 15;

/// Does a rule pattern (already normalised) match an observed process name (normalised)?
fn name_matches(pattern: &str, actual: &str) -> bool {
    pattern == actual || (actual.len() == LINUX_COMM_LEN && pattern.starts_with(actual))
}

pub fn matching_rule<'a>(rules: &'a [Rule], snap: &Snapshot) -> Option<&'a Rule> {
    rules.iter().find(|rule| matches(rule, snap))
}

/// Every kind of condition the rule sets must hold; within a list, any entry will do.
fn matches(rule: &Rule, snap: &Snapshot) -> bool {
    if !rule.has_conditions() {
        return false;
    }
    let focused = rule.when == When::Focused;
    let mut procs = rule
        .processes
        .iter()
        .map(|p| normalize(p))
        .filter(|p| !p.is_empty())
        .peekable();
    if procs.peek().is_some()
        && !procs.any(|p| {
            if focused {
                snap.focused.as_deref().is_some_and(|f| name_matches(&p, f))
            } else {
                snap.running.contains(&p) || snap.running.iter().any(|r| name_matches(&p, r))
            }
        })
    {
        return false;
    }
    let mut titles = rule.titles().peekable();
    if titles.peek().is_some() {
        let Some(title) = snap.title.as_deref().map(str::to_lowercase) else {
            return false;
        };
        if !titles.any(|t| title.contains(&t.to_lowercase())) {
            return false;
        }
    }
    if rule.fullscreen && !(focused && snap.fullscreen) {
        return false;
    }
    let steam = if focused {
        snap.steam_focused
    } else {
        snap.steam_running
    };
    !rule.steam_games || steam
}

/// The colour a matching rule wants. No overrides at all means "disable Ember".
pub fn rule_setting(rule: &Rule, scheduled: ColorSetting) -> ColorSetting {
    match (rule.temperature, rule.brightness) {
        (None, None) => ColorSetting::NEUTRAL,
        (t, b) => ColorSetting {
            temperature: t.unwrap_or(scheduled.temperature),
            brightness: b.unwrap_or(scheduled.brightness),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(names: &[&str], when: When) -> Rule {
        Rule {
            processes: names.iter().map(|s| s.to_string()).collect(),
            when,
            ..Rule::default()
        }
    }

    fn focused(name: &str, title: &str) -> Snapshot {
        Snapshot {
            focused: Some(name.into()),
            title: Some(title.into()),
            ..Snapshot::default()
        }
    }

    #[test]
    fn title_rule_matches_text_in_the_title() {
        let r = Rule {
            titles: vec!["YouTube".into(), "Netflix".into()],
            ..Rule::default()
        };
        let rules = vec![r];
        assert!(
            matching_rule(
                &rules,
                &focused("firefox", "Lofi - youtube — Mozilla Firefox")
            )
            .is_some()
        );
        assert!(matching_rule(&rules, &focused("firefox", "GitHub — Mozilla Firefox")).is_none());
        assert!(matching_rule(&rules, &Snapshot::default()).is_none());
    }

    #[test]
    fn processes_and_titles_must_both_match() {
        let mut r = rule(&["firefox"], When::Focused);
        r.titles = vec!["YouTube".into()];
        let rules = vec![r];
        assert!(matching_rule(&rules, &focused("firefox", "Video - YouTube")).is_some());
        assert!(matching_rule(&rules, &focused("chrome", "Video - YouTube")).is_none());
        assert!(matching_rule(&rules, &focused("firefox", "Mail")).is_none());
    }

    #[test]
    fn fullscreen_rule_needs_a_fullscreen_focused_window() {
        let r = Rule {
            fullscreen: true,
            ..Rule::default()
        };
        let rules = vec![r];
        let mut snap = focused("game", "Game");
        assert!(matching_rule(&rules, &snap).is_none());
        snap.fullscreen = true;
        assert!(matching_rule(&rules, &snap).is_some());
    }

    #[test]
    fn steam_rule_follows_when() {
        let focused_rule = Rule {
            steam_games: true,
            ..Rule::default()
        };
        let running_rule = Rule {
            when: When::Running,
            ..focused_rule.clone()
        };
        let snap = Snapshot {
            steam_running: true,
            ..Snapshot::default()
        };
        assert!(matching_rule(std::slice::from_ref(&focused_rule), &snap).is_none());
        assert!(matching_rule(std::slice::from_ref(&running_rule), &snap).is_some());
        let snap = Snapshot {
            steam_focused: true,
            ..Snapshot::default()
        };
        assert!(matching_rule(&[focused_rule], &snap).is_some());
    }

    #[test]
    fn rule_without_conditions_never_matches() {
        assert!(matching_rule(&[Rule::default()], &focused("x", "y")).is_none());
    }

    #[test]
    fn steam_games_are_found_by_path_or_environment() {
        let env = |v: &[&str]| v.iter().map(OsString::from).collect::<Vec<_>>();
        let win = Path::new(
            r"C:\Program Files (x86)\Steam\steamapps\common\Counter-Strike Global Offensive\game\bin\win64\cs2.exe",
        );
        assert!(is_steam_game("cs2", Some(win), &[]));
        let linux = Path::new("/home/me/.local/share/Steam/steamapps/common/Hades/Hades");
        assert!(is_steam_game("hades", Some(linux), &[]));
        assert!(is_steam_game(
            "wine64-preloader",
            None,
            &env(&["HOME=/home/me", "SteamAppId=1145360"])
        ));
        assert!(!is_steam_game(
            "bash",
            Some(Path::new("/usr/bin/bash")),
            &env(&["SteamAppId=0"])
        ));
        let client = Path::new(r"C:\Program Files (x86)\Steam\steam.exe");
        assert!(!is_steam_game(
            "steam",
            Some(client),
            &env(&["SteamAppId=1145360"])
        ));
    }

    #[test]
    fn normalize_handles_exe_case_and_paths() {
        assert_eq!(normalize("CS2.EXE"), "cs2");
        assert_eq!(normalize("C:\\Games\\cs2.exe"), "cs2");
        assert_eq!(normalize("/usr/bin/cs2"), "cs2");
    }

    #[test]
    fn focused_rule_needs_focus() {
        let rules = vec![rule(&["cs2.exe"], When::Focused)];
        let mut snap = Snapshot::default();
        snap.running.insert("cs2".into());
        assert!(matching_rule(&rules, &snap).is_none());
        snap.focused = Some("cs2".into());
        assert!(matching_rule(&rules, &snap).is_some());
    }

    #[test]
    fn running_rule_and_first_match_wins() {
        let mut a = rule(&["mpv"], When::Running);
        a.temperature = Some(5000);
        let b = rule(&["mpv"], When::Running);
        let rules = vec![a, b];
        let mut snap = Snapshot::default();
        snap.running.insert("mpv".into());
        let r = matching_rule(&rules, &snap).unwrap();
        assert_eq!(r.temperature, Some(5000));
    }

    #[test]
    fn truncated_linux_names_match() {
        let rules = vec![rule(&["averyverylonggamename"], When::Running)];
        let mut snap = Snapshot::default();
        snap.running.insert("averyverylongga".into()); // 15 chars, as in /proc/<pid>/comm
        assert!(matching_rule(&rules, &snap).is_some());
    }

    #[test]
    fn disable_rule_is_neutral() {
        let r = rule(&["cs2"], When::Focused);
        let s = ColorSetting {
            temperature: 3000,
            brightness: 0.7,
        };
        assert_eq!(rule_setting(&r, s), ColorSetting::NEUTRAL);
    }
}
