# Ember

An open-source alternative to f.lux: warms your screen in the evening — and gets out of the way when you're gaming.

**Per-app rules** are the main feature: tell Ember that while `cs2` (Counter-Strike 2) is in the foreground it should switch off, and your colours go back to normal the moment you tab into the game and warm up again when you tab out.

```toml
[[rules]]
name = "Counter-Strike 2"
processes = ["cs2"]      # matches cs2.exe on Windows and cs2 on Linux
when = "focused"         # or "running"
# no temperature/brightness = disable Ember while this matches
```

## Status

Early. Ember is a single system tray app, `ember`.

| Platform | Status |
|---|---|
| Windows 10/11 | Supported (GDI gamma ramps, foreground-window detection, native tray icon). Builds in CI; **not yet tested on real hardware** |
| Linux, X11 | Supported (RandR gamma, `_NET_ACTIVE_WINDOW` focus detection, StatusNotifierItem tray). Tested against Xvfb + a D-Bus tray host |
| Linux, Wayland | Not supported yet |
| macOS | Not supported yet |

## Install

### Windows

Download `ember-vX.Y.Z-windows-x64.zip` from [Releases](https://github.com/fredagsguf/Ember/releases), extract it anywhere, and run **`ember.exe`**. No installer or runtime needed.

`ember.exe` isn't code-signed yet, so the first launch shows *"Windows protected your PC"*: click **More info → Run anyway**. Each release has a `.sha256` file you can check with `Get-FileHash`.

### From source (Windows or Linux)

Requires Rust (https://rustup.rs).

```bash
cargo install --git https://github.com/fredagsguf/Ember
```

```bash
ember           # starts in the tray; no console window on Windows
```

Only one Ember can run at a time; a second one exits with "already running".

## Settings window

Open it from the tray (**Settings…**) or run `ember --settings`.

![Settings window](docs/settings.png)

- **Sliders preview live**: while you drag a temperature or brightness slider, your screen shows that value; let go and it returns to what the schedule says. A running Ember holds the preview instead of fighting it.
- **Changes save automatically** (no Save button) and Ember picks them up within one poll. Only the values you changed are rewritten in `config.toml`, so your comments stay.
- **Schedule**: fixed times, or sunset/sunrise with a city picker (or exact coordinates) and today's fade times shown. No network access.
- **Apps & games**: add a rule from a list of running apps, choose "active window" vs "running", and "turn Ember off" vs a custom colour (with its own previewing slider).
- Edits made elsewhere (e.g. the tray's "Disable while … is focused") show up in an open settings window.

On Linux the window needs OpenGL and `libxkbcommon-x11` (present on any normal desktop install).

## Tray menu

| Item | What it does |
|---|---|
| *status line* | e.g. `3400K`, `Daytime — no tint`, `Off while Counter-Strike 2`, `Paused for 12 more min` |
| Enabled | Untick to pause until you tick it again |
| Disable for ▸ 15 min / 1 hour / 3 hours | Temporary pause |
| Disable while "*app*" is focused | Adds/removes a rule for the last app you used (the taskbar/tray itself is ignored). Open the game, then click the tray icon and tick this — no config editing needed |
| Settings… | Opens the settings window |
| Start with Windows / Start at login | Registry `HKCU\…\Run` on Windows, `~/.config/autostart/ember.desktop` on Linux |
| Open config file | Opens `config.toml` in your editor (created if missing); changes apply on save |
| Quit Ember | Restores neutral colours and exits |

The icon is a glowing ember while your screen is tinted and a grey ring when it isn't. The tray app logs to `ember.log` in the data dir (Windows: `%LOCALAPPDATA%\ember\data\ember.log`, Linux: `~/.local/share/ember/ember.log`).

**Linux tray support** uses the StatusNotifierItem protocol: works on KDE, XFCE, Cinnamon, LXQt, waybar, etc. On GNOME you need the *AppIndicator and KStatusNotifierItem Support* extension; without a tray host `ember` exits with an explanation.

Rules added from the tray are written into your config file without touching your comments or formatting.

## Command-line options

| Option | What it does |
|---|---|
| `--settings` | Open the settings window instead of the tray |
| `-c, --config PATH` | Use a different config file |
| `--dry-run` | Don't touch the display, just log what would happen (see `ember.log`) |

## Configuration

See [`config.example.toml`](config.example.toml) for every option. The config is reloaded automatically when you save it.

- **Schedule**: `fixed` (clock times with a fade), `solar` (follows the sun at your latitude/longitude, no network needed), or `always_night`.
- **Rules**: first match wins. `when = "focused"` only applies while the app is in the foreground; `"running"` applies as long as it's open. Leave out `temperature`/`brightness` to disable Ember; set either to override instead (e.g. a movie player at 5000K).
- Process names are case-insensitive and `.exe` is optional, so one rule works on both OSes.

## Platform notes

**Windows – warm temperatures not applying.** Windows rejects gamma ramps below roughly 4000K unless a registry value is set (f.lux sets the same one). Import [`contrib/enable-full-gamma-range.reg`](contrib/enable-full-gamma-range.reg) as admin and sign out/in. Ember logs a warning when Windows rejects a ramp.

**Windows – autostart.** Use *Start with Windows* in the tray menu. Don't use a Task Scheduler task set to "run whether user is logged on or not" — it runs in a different session and can't change your display.

**Games and anti-cheat.** Ember only changes display gamma ramps (the same API f.lux uses) and reads process names/foreground window. It never touches game processes. Fullscreen-exclusive games may reset the gamma ramp; Ember re-applies it every `poll_interval_secs`.

**Linux autostart.** Use *Start at login* in the tray menu.

**Colour calibration.** On exit / when a rule disables Ember, displays are set to a neutral identity ramp. If you load an ICC calibration curve (VCGT), it will be overwritten until your calibration loader re-applies it.

## Roadmap

- [x] System tray app with on/off toggle, timed disable, and per-app rules from the menu
- [x] Settings window with live-preview sliders, location picker and rule editor
- [ ] Wayland (wlr-gamma-control for sway/Hyprland; GNOME/KDE night-light integration)
- [ ] macOS (CoreGraphics `CGSetDisplayTransferByTable`)
- [ ] Global hotkey to toggle
- [ ] Preserve calibration ramps instead of resetting to identity
- [ ] Prebuilt releases

## Development

```bash
cargo test
cargo clippy --all-targets
cargo check --target x86_64-pc-windows-gnu   # type-check the Windows backend from Linux
```

Layout: `src/lib.rs` is the library; `src/bin/ember/` is the tray app (`windows.rs` uses `tray-icon`, `linux.rs` uses `ksni`, `settings.rs` is the egui settings window). Library pieces: `daemon.rs` (run loop, single-instance lock), `tray_model.rs` (menu state, actions, icon drawing), `autostart.rs`, `color.rs` (Kelvin → RGB → gamma ramp), `solar.rs` (sun elevation), `schedule.rs`, `rules.rs` (per-app matching), `engine.rs` (decision + process watching), `gamma/` (platform backends).

## Releasing

1. Bump `version` in `Cargo.toml` and commit.
2. Tag and push: `git tag v0.2.0 && git push origin v0.2.0`

The [Release workflow](.github/workflows/release.yml) checks the tag matches `Cargo.toml`, runs the tests, builds `ember.exe` with a static C runtime, zips it with the README, example config and registry fix, and publishes a GitHub Release with a SHA-256 checksum. Tags with a suffix (`v0.2.0-beta.1`) become pre-releases. Running the workflow manually from the Actions tab builds the zip as a downloadable artifact without releasing.

The exe icon lives in `assets/ember.ico`; regenerate it with `cargo run --example make_ico` after changing the icon drawing in `tray_model.rs`.

## License

MIT
