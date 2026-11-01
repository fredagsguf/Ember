<p align="center">
  <img src="docs/logo.png" width="128" height="128" alt="Ember logo: a crescent moon cradling a glowing ember">
</p>

<h1 align="center">Ember</h1>

<p align="center">
  <b>A warm screen at night, true colours the moment you tab into a game.</b><br>
  A free, open-source alternative to f.lux for Windows and Linux.
</p>

<p align="center">
  <a href="https://github.com/fredagsguf/Ember/releases/latest"><img src="https://img.shields.io/github/v/release/fredagsguf/Ember?style=for-the-badge&label=Download%20for%20Windows&color=f08c28&logo=windows&logoColor=white" alt="Download for Windows"></a>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/github/license/fredagsguf/Ember?color=555" alt="MIT license"></a>
  <a href="https://github.com/fredagsguf/Ember/actions/workflows/ci.yml"><img src="https://github.com/fredagsguf/Ember/actions/workflows/ci.yml/badge.svg" alt="CI status"></a>
</p>

<p align="center">
  <img src="docs/how-it-works.png" alt="Three screens: an evening desktop tinted warm by Ember, Counter-Strike 2 in focus shown in true colours, and the desktop warm again after alt-tabbing out">
</p>

## Why Ember?

Warm, dim light in the evening is easier on the eyes and on your sleep. That's why f.lux and Windows Night Light exist. But the moment you launch a game, that orange filter is in the way: enemies blend into the background, colours look wrong, and you end up switching the whole thing off by hand and forgetting to turn it back on.

**Ember knows what you're looking at.** Tell it once that Counter-Strike 2 (or any app) should get true colours, and it switches off the instant the game has focus and warms back up when you tab out. No hotkeys, no remembering.

## Features

- **Per-app rules.** Turn Ember off, or use a different colour, while a specific app is focused or simply running. Add one in two clicks from the tray: open the game, click the tray icon, tick *Disable while "cs2" is focused*.
- **Follows the sun or your clock.** Warm up at sunset and back at sunrise for your city, or pick fixed times. Every change fades in gently.
- **Live-preview settings.** Drag a slider and your screen shows that exact colour while you hold it.
- **Pause when you need to.** Disable for 15 minutes, an hour or three hours, or until you switch it back on.
- **Light and private.** One small exe with no installer and no runtime. Ember never connects to the internet: sunset times are calculated on your PC.
- **Free and open source** under the MIT license.

## Get Ember

### Windows 10 and 11

1. Download the latest `ember-vX.Y.Z-windows-x64.zip` from **[Releases](https://github.com/fredagsguf/Ember/releases/latest)**.
2. Extract it anywhere (for example `%LOCALAPPDATA%\Programs\Ember`). Run it from there, not from inside the zip.
3. Run **`ember.exe`**. Ember sits in the system tray; Windows 11 may tuck new icons behind the **^** arrow next to the clock, so drag it onto the taskbar to keep it visible.
4. Optional: tick **Start with Windows** in the tray menu.

`ember.exe` isn't code-signed yet, so the first launch shows *"Windows protected your PC"*. Click **More info → Run anyway**. Each release has a `.sha256` file you can check with `Get-FileHash`.

**Warm colours not going below ~4000K?** Windows limits how warm apps can make the screen until a registry value is set (f.lux sets the same one). Double-click `enable-full-gamma-range.reg` from the zip (needs admin), then sign out and back in.

### Linux (X11)

There's no prebuilt package yet. Install from source with [Rust](https://rustup.rs):

```bash
cargo install --git https://github.com/fredagsguf/Ember
ember
```

Ember needs a StatusNotifierItem tray, which KDE, XFCE, Cinnamon, LXQt and waybar have built in. On GNOME, install the *AppIndicator and KStatusNotifierItem Support* extension. The settings window needs OpenGL and `libxkbcommon-x11`, which any normal desktop install has.

## Settings

Open them from the tray icon (**Settings…**).

<p align="center">
  <img src="docs/settings.png" width="560" alt="Ember's settings window with colour sliders, schedule options and an app rule for Counter-Strike 2">
</p>

Changes save as you make them and apply within a couple of seconds; there's no Save button. Only the values you change are rewritten in `config.toml`, so any comments you've written there stay.

## The tray icon

The moon glows while Ember is warming your screen and turns grey when it isn't (daytime, paused, or a rule has switched it off). Right-click it for the menu:

| Item | What it does |
|---|---|
| *status line* | What Ember is doing now, e.g. `3400K`, `Daytime — no tint`, `Off while Counter-Strike 2`, `Paused for 12 more min` |
| Enabled | Untick to pause until you tick it again |
| Disable for ▸ 15 min / 1 hour / 3 hours | Temporary pause |
| Disable while "*app*" is focused | Adds or removes a rule for the last app you used |
| Settings… | Opens the settings window |
| Start with Windows / Start at login | Starts Ember when you sign in |
| Open config file | Opens `config.toml` in your editor; changes apply on save |
| Quit Ember | Restores normal colours and exits |

## FAQ

**Is it safe with anti-cheat?**
Ember changes the display's colour curves through the same Windows API f.lux uses, and only reads which app is in the foreground. It never touches game processes or their memory.

**Does it work in fullscreen games?**
Some fullscreen-exclusive games reset the colour curves; Ember re-applies its colours every couple of seconds. For games you've set a rule for, it doesn't matter: Ember is off while they're focused anyway.

**What about my monitor's colour calibration?**
When Ember turns off, it resets the display to neutral, which also clears a calibration curve (ICC/VCGT) loaded by other software until that software applies it again.

**Does it collect any data?**
No. Ember has no network code at all. It writes a log file to `%LOCALAPPDATA%\ember\data\ember.log` (Linux: `~/.local/share/ember/ember.log`), and that never leaves your PC.

**How do I start it automatically?**
Use *Start with Windows* (or *Start at login* on Linux) in the tray menu. Don't use a Task Scheduler task set to "run whether user is logged on or not": that runs outside your desktop session and can't change your display.

## Status and roadmap

Ember is young. Windows 10/11 and Linux on X11 are supported; Wayland and macOS aren't yet.

- [x] Tray app with per-app rules, timed pause and autostart
- [x] Settings window with live preview, sunset/sunrise schedule and rule editor
- [x] Prebuilt Windows releases
- [ ] Wayland (wlr-gamma-control for sway/Hyprland; GNOME/KDE night-light integration)
- [ ] macOS
- [ ] Global hotkey to toggle
- [ ] Keep your calibration curve instead of resetting to neutral

Ideas and bug reports are welcome in [Issues](https://github.com/fredagsguf/Ember/issues).

<details>
<summary><b>Configuration file reference</b></summary>

<br>

Everything in the settings window lives in `config.toml` (Windows: `%APPDATA%\ember\config\config.toml`, Linux: `~/.config/ember/config.toml`). See [`config.example.toml`](config.example.toml) for every option. Ember reloads the file automatically when you save it.

```toml
[[rules]]
name = "Counter-Strike 2"
processes = ["cs2"]      # matches cs2.exe on Windows and cs2 on Linux
when = "focused"         # or "running"
# no temperature/brightness = turn Ember off while this matches
```

- **Schedule**: `fixed` (clock times with a fade), `solar` (follows the sun at your latitude/longitude), or `always_night`.
- **Rules**: the first match wins. `when = "focused"` applies only while the app is in the foreground; `"running"` applies as long as it's open. Leave out `temperature` and `brightness` to turn Ember off; set either to use that colour instead (e.g. a movie player at 5000K).
- Process names are case-insensitive and `.exe` is optional, so one rule works on both Windows and Linux.

Command-line options:

| Option | What it does |
|---|---|
| `--settings` | Open the settings window instead of the tray |
| `-c, --config PATH` | Use a different config file |
| `--dry-run` | Don't touch the display; just log what would happen |

Only one Ember runs at a time; starting a second one exits with "already running".

</details>

<details>
<summary><b>For developers</b></summary>

<br>

```bash
cargo test
cargo clippy --all-targets
```

Layout: `src/lib.rs` is the library; `src/bin/ember/` is the tray app (`windows.rs` uses `tray-icon`, `linux.rs` uses `ksni`, `settings.rs` is the egui settings window). Library pieces: `daemon.rs` (run loop, single-instance lock), `tray_model.rs` (menu state, actions, icon drawing), `autostart.rs`, `color.rs` (Kelvin → RGB → gamma ramp), `solar.rs` (sun elevation), `schedule.rs`, `rules.rs` (per-app matching), `engine.rs` (decision + process watching), `gamma/` (platform backends).

The logo is drawn in code (`icon_rgba` in `tray_model.rs`). `assets/ember.ico`, embedded in the exe, is generated from it with `cargo run --example make_ico`.

**Releasing:**

1. Bump `version` in `Cargo.toml` and commit.
2. Tag and push: `git tag v0.2.0 && git push origin v0.2.0`

The [Release workflow](.github/workflows/release.yml) checks the tag matches `Cargo.toml`, runs the tests, builds `ember.exe` with a static C runtime, zips it with the README, example config and registry fix, and publishes a GitHub Release with a SHA-256 checksum. Tags with a suffix (`v0.2.0-beta.1`) become pre-releases. Running the workflow manually from the Actions tab builds the zip as a downloadable artifact without releasing.

</details>

## License

[MIT](LICENSE). Free to use, change and share.
