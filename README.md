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

Early, CLI-only (no tray icon yet).

| Platform | Status |
|---|---|
| Windows 10/11 | Supported (GDI gamma ramps, foreground-window detection). Builds and type-checks in CI; **not yet tested on real hardware** |
| Linux, X11 | Supported (RandR gamma, `_NET_ACTIVE_WINDOW` focus detection). Tested against Xvfb |
| Linux, Wayland | Not supported yet |
| macOS | Not supported yet |

## Install

Requires Rust (https://rustup.rs).

```bash
cargo install --git https://github.com/fredagsguf/Ember
```

```bash
ember init      # writes an example config with a CS2 rule
ember           # run (same as `ember run`)
```

## Commands

| Command | What it does |
|---|---|
| `ember run [--dry-run]` | Run in the foreground. Ctrl+C restores neutral colours. |
| `ember status` | Show the scheduled colour, the focused process, and which rule (if any) is active |
| `ember processes` | List process names — use this to find the name to put in a rule |
| `ember pause [-m MIN]` / `ember resume` | Temporarily go neutral (a running `ember` picks it up within one poll) |
| `ember set 3000 [-b 0.8]` | Apply a temperature once and exit |
| `ember reset` | Reset all displays to neutral |
| `ember init` / `ember config-path` | Create / locate the config file |

## Configuration

See [`config.example.toml`](config.example.toml) for every option. The config is reloaded automatically when you save it.

- **Schedule**: `fixed` (clock times with a fade), `solar` (follows the sun at your latitude/longitude, no network needed), or `always_night`.
- **Rules**: first match wins. `when = "focused"` only applies while the app is in the foreground; `"running"` applies as long as it's open. Leave out `temperature`/`brightness` to disable Ember; set either to override instead (e.g. a movie player at 5000K).
- Process names are case-insensitive and `.exe` is optional, so one rule works on both OSes.

## Platform notes

**Windows – warm temperatures not applying.** Windows rejects gamma ramps below roughly 4000K unless a registry value is set (f.lux sets the same one). Import [`contrib/enable-full-gamma-range.reg`](contrib/enable-full-gamma-range.reg) as admin and sign out/in. Ember logs a warning when Windows rejects a ramp.

**Windows – console window / autostart.** `ember run` currently runs in a console window. To start at login, put a shortcut to `ember.exe run` in `shell:startup` (set it to run minimised). Don't use a Task Scheduler task set to "run whether user is logged on or not" — it runs in a different session and can't change your display. A tray app is on the roadmap.

**Games and anti-cheat.** Ember only changes display gamma ramps (the same API f.lux uses) and reads process names/foreground window. It never touches game processes. Fullscreen-exclusive games may reset the gamma ramp; Ember re-applies it every `poll_interval_secs`.

**Linux autostart.** See [`contrib/ember.service`](contrib/ember.service) (systemd user unit).

**Colour calibration.** On exit / when a rule disables Ember, displays are set to a neutral identity ramp. If you load an ICC calibration curve (VCGT), it will be overwritten until your calibration loader re-applies it.

## Roadmap

- [ ] System tray app with on/off toggle and "disable for 1 hour"
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

Layout: `color.rs` (Kelvin → RGB → gamma ramp), `solar.rs` (sun elevation), `schedule.rs`, `rules.rs` (per-app matching), `engine.rs` (decision + process watching), `gamma/` (platform backends), `main.rs` (CLI + run loop).

## License

MIT
