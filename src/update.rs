//! Opt-in update check (Windows): ask GitHub for the latest release, and on the user's say-so
//! download its exe, check it against the release's SHA-256 file and put it in place of this one.
//!
//! Off unless `general.check_for_updates` is on. Nothing is downloaded until the user picks
//! "Update to …" in the tray menu.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

/// GitHub's newest release that is neither a draft nor a pre-release.
pub const LATEST_URL: &str = "https://api.github.com/repos/fredagsguf/Ember/releases/latest";

/// How often to ask while Ember runs. Far below GitHub's 60 unauthenticated calls an hour.
pub const CHECK_EVERY: Duration = Duration::from_secs(12 * 60 * 60);
/// After a failed check (offline, GitHub down), try again sooner than `CHECK_EVERY`.
const RETRY_AFTER: Duration = Duration::from_secs(60 * 60);
/// At startup, give the network a moment to come up after login.
const STARTUP_DELAY: Duration = Duration::from_secs(60);
/// After the user turns checks on, look right away.
const ENABLED_DELAY: Duration = Duration::from_secs(3);

/// `major.minor.patch`, compared field by field.
pub type Version = [u64; 3];

/// The running build's version.
pub fn current_version() -> Version {
    parse_version(env!("CARGO_PKG_VERSION")).expect("Cargo.toml version is major.minor.patch")
}

/// `"v0.2.1"` or `"0.2.1"`. `None` for pre-releases (`"v0.2.1-rc.1"`) and anything else.
pub fn parse_version(s: &str) -> Option<Version> {
    let s = s.trim();
    let s = s.strip_prefix('v').unwrap_or(s);
    let mut parts = s.split('.');
    let mut v = [0u64; 3];
    for slot in &mut v {
        let p = parts.next()?;
        if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *slot = p.parse().ok()?;
    }
    parts.next().is_none().then_some(v)
}

pub fn version_text(v: Version) -> String {
    format!("v{}.{}.{}", v[0], v[1], v[2])
}

/// A release with a Windows download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub tag: String,
    /// The release's page on GitHub.
    pub page: String,
    pub exe_name: String,
    pub exe_url: String,
    pub sha256_url: String,
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
}

/// Read GitHub's `releases/latest` answer. `Ok(None)` for a release we shouldn't offer
/// (a draft, a pre-release, a tag that isn't a version).
pub fn parse_release(json: &str) -> Result<Option<Release>> {
    let r: ApiRelease = serde_json::from_str(json).context("reading GitHub's release info")?;
    let Some(version) = parse_version(&r.tag_name).filter(|_| !r.draft && !r.prerelease) else {
        return Ok(None);
    };
    // The names release.yml gives the Windows build and its checksum.
    let exe_name = format!("ember-{}-windows-x64.exe", r.tag_name);
    let sha_name = format!("{exe_name}.sha256");
    let url = |name: &str| {
        r.assets
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.browser_download_url.clone())
    };
    let (Some(exe_url), Some(sha256_url)) = (url(&exe_name), url(&sha_name)) else {
        bail!("release {} has no Windows download", r.tag_name);
    };
    for u in [&exe_url, &sha256_url] {
        if !u.starts_with("https://github.com/") {
            bail!("unexpected download address {u}");
        }
    }
    Ok(Some(Release {
        version,
        tag: r.tag_name,
        page: r.html_url,
        exe_name,
        exe_url,
        sha256_url,
    }))
}

/// The hash in a `sha256sum`-style line (`<64 hex digits>  <file name>`) for `file_name`.
pub fn parse_sha256_file(text: &str, file_name: &str) -> Result<[u8; 32]> {
    for line in text.lines() {
        let mut words = line.split_whitespace();
        let (Some(hash), name) = (words.next(), words.next()) else {
            continue;
        };
        // The name may carry sha256sum's binary-mode `*`.
        if name.is_some_and(|n| n.trim_start_matches('*') != file_name) {
            continue;
        }
        return parse_hex32(hash).with_context(|| format!("bad SHA-256 {hash:?}"));
    }
    bail!("no SHA-256 for {file_name}")
}

fn parse_hex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).into()
}

/// Is `exe` the build the checksum describes, and a Windows program at all?
pub fn verify_download(exe: &[u8], expected: &[u8; 32]) -> Result<()> {
    if sha256(exe) != *expected {
        bail!("the download doesn't match its SHA-256 checksum");
    }
    if !exe.starts_with(b"MZ") {
        bail!("the download isn't a Windows program");
    }
    Ok(())
}

type CheckResult = Result<Option<Release>>;

/// Runs update checks on a background thread while they're turned on and remembers the newest
/// release found. The tray loop calls `poll` every pass.
pub struct Checker {
    fetch: fn() -> CheckResult,
    current: Version,
    next_check: Option<Instant>,
    pending: Option<Receiver<CheckResult>>,
    available: Option<Release>,
    created: Instant,
}

impl Checker {
    pub fn new() -> Checker {
        Checker::with_fetch(fetch_latest, current_version())
    }

    pub fn with_fetch(fetch: fn() -> CheckResult, current: Version) -> Checker {
        Checker {
            fetch,
            current,
            next_check: None,
            pending: None,
            available: None,
            created: Instant::now(),
        }
    }

    /// A newer release than this one, once a check has found it.
    pub fn available(&self) -> Option<&Release> {
        self.available.as_ref()
    }

    /// Start a check when one is due, pick up a finished one. With `enabled` false, forget
    /// everything and stay offline.
    pub fn poll(&mut self, enabled: bool, now: Instant) {
        if !enabled {
            self.next_check = None;
            self.pending = None; // a check in flight finishes unheard
            self.available = None;
            return;
        }
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Err(TryRecvError::Empty) => return,
                Ok(Ok(release)) => {
                    self.available = release.filter(|r| r.version > self.current);
                    match &self.available {
                        Some(r) => log::info!("update available: Ember {}", r.tag),
                        None => log::info!("Ember is up to date"),
                    }
                    self.next_check = Some(now + CHECK_EVERY);
                }
                Ok(Err(e)) => {
                    log::warn!("update check failed: {e:#}");
                    self.next_check = Some(now + RETRY_AFTER);
                }
                Err(TryRecvError::Disconnected) => {
                    log::warn!("update check stopped unexpectedly");
                    self.next_check = Some(now + RETRY_AFTER);
                }
            }
            self.pending = None;
        }
        let soonest = (now + ENABLED_DELAY).max(self.created + STARTUP_DELAY);
        let due = *self.next_check.get_or_insert(soonest);
        if now >= due {
            let (tx, rx) = channel();
            let fetch = self.fetch;
            let spawned = std::thread::Builder::new()
                .name("ember-update-check".into())
                .spawn(move || {
                    let _ = tx.send(fetch());
                });
            match spawned {
                Ok(_) => self.pending = Some(rx),
                Err(e) => {
                    log::warn!("couldn't start the update check: {e}");
                    self.next_check = Some(now + RETRY_AFTER);
                }
            }
        }
    }
}

impl Default for Checker {
    fn default() -> Self {
        Checker::new()
    }
}

#[cfg(windows)]
pub use imp::{cleanup_after_update, install, wait_for_exit};

#[cfg(windows)]
fn fetch_latest() -> CheckResult {
    let json = imp::download(LATEST_URL, true)?;
    parse_release(std::str::from_utf8(&json).context("GitHub's release info isn't text")?)
}

/// Update checks only exist on Windows; elsewhere the package manager or the user updates.
#[cfg(not(windows))]
fn fetch_latest() -> CheckResult {
    Ok(None)
}

#[cfg(windows)]
mod imp {
    use super::Release;
    use anyhow::{Context, Result, bail};
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        CREATE_NO_WINDOW, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };

    /// Fetch `url` with the `curl.exe` that ships with Windows 10 and 11: HTTPS only (redirects
    /// too), no console window.
    pub(super) fn download(url: &str, api: bool) -> Result<Vec<u8>> {
        let curl = crate::win::system_dir()?.join("curl.exe");
        let mut cmd = std::process::Command::new(&curl);
        cmd.args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--max-time",
            "300",
            "--max-filesize",
            "200000000",
            "--user-agent",
            concat!("Ember/", env!("CARGO_PKG_VERSION"), " (update check)"),
        ]);
        if api {
            cmd.args(["--header", "Accept: application/vnd.github+json"]);
        }
        let out = cmd
            .arg(url)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .with_context(|| format!("couldn't run {}", curl.display()))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            bail!("couldn't download {url}: {}", err.trim());
        }
        Ok(out.stdout)
    }

    fn sibling(exe: &Path, suffix: &str) -> PathBuf {
        let mut name = exe.file_name().unwrap_or_default().to_os_string();
        name.push(suffix);
        exe.with_file_name(name)
    }

    /// Download `release`, check it, and put it where this exe is: same file name, so Start
    /// with Windows and shortcuts keep working. Then start it, telling it to wait until this
    /// process has quit. The caller must quit right after.
    pub fn install(release: &Release, config_path: &Path) -> Result<()> {
        let exe = std::env::current_exe().context("finding ember.exe")?;
        let (new, old) = (sibling(&exe, ".new"), sibling(&exe, ".old"));

        let sums = download(&release.sha256_url, false)?;
        let expected =
            super::parse_sha256_file(&String::from_utf8_lossy(&sums), &release.exe_name)?;
        let bytes = download(&release.exe_url, false)?;
        super::verify_download(&bytes, &expected)?;

        std::fs::write(&new, &bytes).with_context(|| {
            format!(
                "couldn't save the update next to {} (is the folder read-only?)",
                exe.display()
            )
        })?;
        // Windows lets a running exe be renamed, not overwritten.
        let _ = std::fs::remove_file(&old);
        if let Err(e) = std::fs::rename(&exe, &old) {
            let _ = std::fs::remove_file(&new);
            return Err(e).with_context(|| format!("couldn't move {} aside", exe.display()));
        }
        if let Err(e) = std::fs::rename(&new, &exe) {
            let _ = std::fs::rename(&old, &exe);
            let _ = std::fs::remove_file(&new);
            return Err(e).with_context(|| format!("couldn't put the update at {}", exe.display()));
        }
        let started = std::process::Command::new(&exe)
            .arg("--config")
            .arg(config_path)
            .arg("--replaces")
            .arg(std::process::id().to_string())
            .spawn();
        if let Err(e) = started {
            // Put the old one back so the next start isn't an exe that won't run.
            let _ = std::fs::remove_file(&exe);
            let _ = std::fs::rename(&old, &exe);
            return Err(e).context("couldn't start the new Ember");
        }
        log::info!("updated to Ember {}; restarting", release.tag);
        Ok(())
    }

    /// Wait (up to `timeout`) for the Ember that started us to quit and let go of the displays.
    pub fn wait_for_exit(pid: u32, timeout: std::time::Duration) {
        unsafe {
            let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
            if h.is_null() {
                return; // already gone
            }
            if WaitForSingleObject(h, timeout.as_millis().min(u32::MAX as u128) as u32)
                != WAIT_OBJECT_0
            {
                log::warn!("the previous Ember (pid {pid}) is still running");
            }
            CloseHandle(h);
        }
    }

    /// Delete what `install` left behind: the replaced exe and an unfinished download.
    pub fn cleanup_after_update() {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        for suffix in [".old", ".new"] {
            let p = sibling(&exe, suffix);
            if p.exists()
                && let Err(e) = std::fs::remove_file(&p)
            {
                log::warn!("couldn't delete {}: {e}", p.display());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELEASE_JSON: &str = r#"{
        "tag_name": "v0.2.0",
        "html_url": "https://github.com/fredagsguf/Ember/releases/tag/v0.2.0",
        "draft": false,
        "prerelease": false,
        "assets": [
            {"name": "ember-v0.2.0-linux-x64.tar.gz",
             "browser_download_url": "https://github.com/fredagsguf/Ember/releases/download/v0.2.0/ember-v0.2.0-linux-x64.tar.gz"},
            {"name": "ember-v0.2.0-windows-x64.exe",
             "browser_download_url": "https://github.com/fredagsguf/Ember/releases/download/v0.2.0/ember-v0.2.0-windows-x64.exe"},
            {"name": "ember-v0.2.0-windows-x64.exe.sha256",
             "browser_download_url": "https://github.com/fredagsguf/Ember/releases/download/v0.2.0/ember-v0.2.0-windows-x64.exe.sha256"}
        ],
        "body": "ignored"
    }"#;

    #[test]
    fn parses_versions() {
        assert_eq!(parse_version("v0.1.10"), Some([0, 1, 10]));
        assert_eq!(parse_version("1.2.3"), Some([1, 2, 3]));
        assert_eq!(parse_version("v0.2.0-rc.1"), None);
        assert_eq!(parse_version("v1.2"), None);
        assert_eq!(parse_version("v1.2.3.4"), None);
        assert_eq!(parse_version("v1.+2.3"), None);
        assert!(parse_version("v0.1.10") > parse_version("v0.1.9"));
        assert!(parse_version("v1.0.0") > parse_version("v0.99.99"));
        assert_eq!(
            version_text(current_version()),
            format!("v{}", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn reads_the_windows_download_from_a_release() {
        let r = parse_release(RELEASE_JSON).unwrap().unwrap();
        assert_eq!(r.version, [0, 2, 0]);
        assert_eq!(r.exe_name, "ember-v0.2.0-windows-x64.exe");
        assert!(r.exe_url.ends_with("/ember-v0.2.0-windows-x64.exe"));
        assert!(r.sha256_url.ends_with(".exe.sha256"));
    }

    #[test]
    fn skips_pre_releases_and_needs_a_windows_download() {
        let pre = RELEASE_JSON.replace(r#""prerelease": false"#, r#""prerelease": true"#);
        assert_eq!(parse_release(&pre).unwrap(), None);
        let no_exe = RELEASE_JSON.replace("windows-x64.exe\",", "windows-x64.zip\",");
        assert!(parse_release(&no_exe).is_err());
        let elsewhere = RELEASE_JSON.replace(
            "https://github.com/fredagsguf/Ember/releases/download/v0.2.0/ember-v0.2.0-windows-x64.exe\"",
            "https://example.com/ember.exe\"",
        );
        assert!(parse_release(&elsewhere).is_err());
    }

    #[test]
    fn checks_downloads_against_the_sha256_file() {
        let exe = b"MZ pretend program";
        let hash = sha256(exe);
        let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
        let file = format!("{hex}  ember-v0.2.0-windows-x64.exe\n");
        let expected = parse_sha256_file(&file, "ember-v0.2.0-windows-x64.exe").unwrap();
        assert_eq!(expected, hash);
        verify_download(exe, &expected).unwrap();
        assert!(verify_download(b"MZ something else", &expected).is_err());
        assert!(parse_sha256_file(&file, "other.exe").is_err());
        assert!(parse_sha256_file("abc  ember.exe", "ember.exe").is_err());
        // Uppercase, binary-mode marker: as Get-FileHash or sha256sum -b might write it.
        let upper = format!("{}  *ember.exe", hex.to_uppercase());
        assert_eq!(parse_sha256_file(&upper, "ember.exe").unwrap(), hash);
        let not_exe = b"plain text";
        assert!(verify_download(not_exe, &sha256(not_exe)).is_err());
    }

    fn found_newer() -> CheckResult {
        parse_release(RELEASE_JSON)
    }

    fn wait_for_check(c: &mut Checker, now: Instant) {
        for _ in 0..500 {
            c.poll(true, now);
            if c.pending.is_none() {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("check never finished");
    }

    #[test]
    fn checker_stays_offline_until_enabled_and_forgets_when_disabled() {
        let mut c = Checker::with_fetch(|| panic!("must not go online"), [0, 1, 0]);
        let t0 = Instant::now();
        c.poll(false, t0);
        c.poll(false, t0 + CHECK_EVERY * 3);
        assert!(c.available().is_none());

        let mut c = Checker::with_fetch(found_newer, [0, 1, 0]);
        let t0 = Instant::now();
        c.poll(true, t0);
        assert!(c.pending.is_none(), "waits for the network after startup");
        wait_for_check(&mut c, t0 + STARTUP_DELAY);
        assert_eq!(c.available().map(|r| r.version), Some([0, 2, 0]));
        assert_eq!(c.next_check, Some(t0 + STARTUP_DELAY + CHECK_EVERY));

        c.poll(false, t0 + STARTUP_DELAY);
        assert!(c.available().is_none());
        // Turned back on: checks soon, not after the startup delay.
        let t1 = t0 + STARTUP_DELAY * 2;
        c.poll(true, t1);
        assert_eq!(c.next_check, Some(t1 + ENABLED_DELAY));
    }

    #[test]
    fn checker_ignores_releases_that_are_not_newer() {
        let mut c = Checker::with_fetch(found_newer, [0, 2, 0]);
        let t0 = Instant::now();
        c.poll(true, t0);
        wait_for_check(&mut c, t0 + STARTUP_DELAY);
        assert!(c.available().is_none());
    }

    #[test]
    fn checker_retries_sooner_after_a_failure() {
        let mut c = Checker::with_fetch(|| bail!("offline"), [0, 1, 0]);
        let t0 = Instant::now();
        c.poll(true, t0);
        wait_for_check(&mut c, t0 + STARTUP_DELAY);
        assert_eq!(c.next_check, Some(t0 + STARTUP_DELAY + RETRY_AFTER));
    }
}
