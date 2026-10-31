//! Per-application rules: "when cs2 is focused, turn Ember off".

use crate::color::ColorSetting;
use crate::config::{Rule, When};
use std::collections::HashSet;

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
}

/// Linux truncates process names (`comm`) to 15 bytes.
const LINUX_COMM_LEN: usize = 15;

/// Does a rule pattern (already normalised) match an observed process name (normalised)?
fn name_matches(pattern: &str, actual: &str) -> bool {
    pattern == actual || (actual.len() == LINUX_COMM_LEN && pattern.starts_with(actual))
}

pub fn matching_rule<'a>(rules: &'a [Rule], snap: &Snapshot) -> Option<&'a Rule> {
    rules.iter().find(|rule| {
        rule.processes
            .iter()
            .map(|p| normalize(p))
            .any(|p| match rule.when {
                When::Running => {
                    snap.running.contains(&p) || snap.running.iter().any(|r| name_matches(&p, r))
                }
                When::Focused => snap.focused.as_deref().is_some_and(|f| name_matches(&p, f)),
            })
    })
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
            name: None,
            processes: names.iter().map(|s| s.to_string()).collect(),
            when,
            temperature: None,
            brightness: None,
        }
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
