<p align="center">
  <img src="docs/logo.png" width="128" height="128" alt="Ember logo: a crescent moon cradling a glowing ember">
</p>

<h1 align="center">Ember</h1>

<p align="center">
  <b>A warm screen at night, true colours the moment you tab into a game.</b><br>
  A free, open-source alternative to f.lux for Windows and Linux.
</p>

<p align="center">
  <a href="#windows"><img src="https://img.shields.io/badge/Download-Windows-f08c28?style=for-the-badge&logo=windows&logoColor=white" alt="Download for Windows"></a>
  <a href="#linux"><img src="https://img.shields.io/badge/Download-Linux-f08c28?style=for-the-badge&logo=linux&logoColor=white" alt="Download for Linux"></a>
  <br>
  <a href="https://github.com/fredagsguf/Ember/releases/latest"><img src="https://img.shields.io/github/v/release/fredagsguf/Ember?label=latest%20release&color=555" alt="Latest release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/fredagsguf/Ember?color=555" alt="MIT license"></a>
</p>

<p align="center">
  <img src="docs/how-it-works.png" alt="Three screens: an evening desktop tinted warm by Ember, Counter-Strike 2 in focus shown in true colours, and the desktop warm again after alt-tabbing out">
</p>

## Why Ember?

A warm screen in the evening is easier on your eyes and your sleep. But in a game, that orange tint gets in the way, and you end up switching it off by hand and forgetting to turn it back on.

**Ember does it for you.** It turns off while your game is in front and warms back up when you tab out.

## Features

- **Off for games, automatically.** For any fullscreen app, every Steam game, or apps you pick.
- **Rules for anything.** Turn Ember off or use another colour for an app, or for a window title such as YouTube.
- **Follows the sun or your clock.** Later nights on weekends, and an optional extra-warm bedtime.
- **Gentle.** Every change fades in, also when you turn on your PC in the evening.
- **Quick to pause.** For 15 minutes, an hour or until you turn it back on, or with **Ctrl+Alt+End**.
- **Respects your monitors.** Keeps colour calibration, and can leave a screen untinted.
- **Light and private.** One small program, no installer, and it never goes online.

## Get Ember

### Windows

1. Download `ember-…-windows-x64.exe` from **[Releases](https://github.com/fredagsguf/Ember/releases/latest)** and move it to where it should stay (e.g. `%LOCALAPPDATA%\Programs\Ember\`), since *Start with Windows* remembers that location.
2. Run it. It sits in the system tray (Windows 11 may hide it behind the **^** next to the clock).
3. Optional: tick **Start with Windows** in the tray menu.

The first launch shows *"Windows protected your PC"* because Ember isn't code-signed yet. Click **More info → Run anyway**.

If the screen won't go warmer than about 4000K, pick **Allow warmer colours…** in the tray menu, then sign out and back in.

### Linux

Download `ember-…-linux-x64.tar.gz` from **[Releases](https://github.com/fredagsguf/Ember/releases/latest)** and install it for your user:

```bash
tar -xzf ember-v*-linux-x64.tar.gz
cd ember-v*-linux-x64
install -Dm755 ember ~/.local/bin/ember
install -Dm644 ember.desktop ~/.local/share/applications/ember.desktop
install -Dm644 ember.png ~/.local/share/icons/hicolor/512x512/apps/ember.png
ember
```

Works on X11, and on Wayland with KDE Plasma, sway, Hyprland and other wlroots compositors. GNOME on Wayland doesn't let apps change the screen colours, so use its built-in Night Light there. On GNOME you also need the *AppIndicator* extension for the tray icon. More in [Linux details](#linux-details).

## Using Ember

Click the tray icon to open the settings, right above the icon, and click it again to put them away. Everything fits on one screen; less common settings are folded away, and a rule opens for editing when you click *Edit*. Drag a slider and your screen shows that colour while you hold it. Changes save on their own.

<p align="center">
  <img src="docs/settings.png" width="860" alt="Ember's settings window. Left: night and bedtime colour, the schedule with a bedtime and later nights on Friday and Saturday, and Apps &amp; games with one-tick options for fullscreen apps and Steam games plus two rules shown as one-line rows. Right: the Counter-Strike 2 rule opened for editing.">
</p>

Right-click the tray icon for the menu: pause, *Disable while "app" is focused* for the app you just used, start at login, and quit. The moon glows while Ember is tinting your screen, and hovering over it shows when the next change happens.

## FAQ

**Is it safe with anti-cheat?**
Yes. Ember uses the same Windows colour API as f.lux and only checks which app is in front. It never touches game processes.

**Does it collect any data?**
No. Ember has no network code. Its log file stays on your PC.

**My monitor is calibrated. Will Ember ruin that?**
No. Ember tints on top of your calibration and puts it back when it steps aside.

**How do I start it automatically?**
Tick *Start with Windows* (*Start at login* on Linux) in the tray menu.

## Roadmap

- A signed Windows app, so there's no "Windows protected your PC" warning
- Installing through winget and Linux package managers

Ideas and bug reports are welcome in [Issues](https://github.com/fredagsguf/Ember/issues).

<details>
<summary><b id="linux-details">Linux details</b></summary>

<br>

Ember picks the right method for your session by itself:

| Session | How the colours change | Focused rules match | Toggle hotkey |
|---|---|---|---|
| X11, any desktop | RandR gamma ramps | process name | ✓ |
| Wayland: sway, Hyprland, river, Wayfire, labwc… (wlroots) | `wlr-gamma-control` | the window's **app_id** | Hyprland ✓ |
| Wayland: KDE Plasma 5.18+ | KDE's Night Light | process name | Plasma 6 ✓ |
| Wayland: GNOME | not possible | — | — |

- **Tray:** Ember needs a StatusNotifierItem tray. KDE, XFCE, Cinnamon, LXQt and waybar have one built in.
- **wlroots:** only one program can control the colours, so quit gammastep or wlsunset first. Focused rules match the window's app_id (`firefox`, `org.mozilla.firefox`, `steam_app_730` for CS2…); ticking *Disable while … is focused* in the tray uses the right name.
- **Hotkey on Wayland:** it goes through the desktop's shortcuts portal. Plasma asks you to confirm the key once. On Hyprland, add `bind = CTRL ALT, End, global, ember:toggle` to `hyprland.conf`. Elsewhere, pause from the tray menu.
- **KDE Plasma on Wayland:** KWin doesn't let apps set colours, so Ember switches KDE's Night Light on and off instead. Turn Night Light on in *System Settings → Display & Monitor* and pick its colour and times there. Ember's colour sliders have no effect, and the settings window can't preview.
- **From source:** `cargo install --git https://github.com/fredagsguf/Ember` (needs [Rust](https://rustup.rs)).

</details>

<details>
<summary><b>Configuration file</b></summary>

<br>

Everything in the settings window lives in `config.toml` (Windows: `%APPDATA%\ember\config\config.toml`, Linux: `~/.config/ember/config.toml`). Ember reloads it when you save. [`config.example.toml`](config.example.toml) explains every option.

```toml
[[rules]]
name = "Counter-Strike 2"
processes = ["cs2"]      # cs2.exe on Windows, cs2 on Linux
when = "focused"         # or "running"
# no temperature/brightness = turn Ember off while this matches
```

- **Rules** match on `processes`, `titles` (text in the window title), `fullscreen = true` or `steam_games = true`. Set several and all must hold. The first matching rule wins.
- **Schedule:** `mode` is `fixed`, `solar` or `always_night`. Extras: `[schedule.days]` for other times on some nights, `sunset_offset_minutes`/`sunrise_offset_minutes`, and `bedtime = "23:00"`.
- **Displays:** `[[displays]]` with `id = '\\.\DISPLAY2'` (Linux: `"HDMI-1"`) and `tint = false` leaves that screen alone.
- **Hotkey:** `[general] toggle_hotkey = "Ctrl+Alt+End"`, or `""` for none.

Command-line options: `--settings` opens the settings window, `-c PATH` uses another config file, `--dry-run` only logs what Ember would do.

</details>

<details>
<summary><b>For developers</b></summary>

<br>

```bash
cargo test
cargo clippy --all-targets
```

`src/bin/ember/` is the app: the tray (`windows.rs`, `linux.rs`) and the egui settings window (`settings.rs`). The library in `src/` holds the rest: `engine.rs` and `rules.rs` decide what to show, `schedule.rs` and `solar.rs` the time of day, `color.rs` turns Kelvin into gamma ramps, and `gamma/` has a backend per platform (GDI, RandR, wlroots, KDE Night Light).

**Releasing:** bump `version` in `Cargo.toml`, commit, then tag and push (`git tag v0.2.0 && git push origin v0.2.0`). The [Release workflow](.github/workflows/release.yml) tests on Windows and Linux, builds both packages and publishes them with SHA-256 checksums.

**CI** runs by hand from the Actions tab, or on pushes to `ci/…` branches. Besides the tests, it checks the app on a real Windows desktop and saves screenshots to the `ci-screenshots` branch.

</details>

## License

[MIT](LICENSE). Free to use, change and share.
