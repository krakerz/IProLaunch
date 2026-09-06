# Changelog

## [Unreleased]

## [1.20.1] — 2026-09-07

### Fixed
- Gamepad: retries reconnecting every ~2s while none is detected — Steam Input's virtual controller on a Steam Deck could otherwise go undetected for the rest of the session after Steam recreated it for a different app/game.

## [1.20.0] — 2026-09-06

### Added
- Library `s`: adds a game to Steam as a non-Steam-game shortcut live, via Steam's own "Add a Non-Steam Game" importer (no `shortcuts.vdf` editing, no Steam restart) — optionally bakes in this game's own gamescope flags; shows under its title or name (no internal "#N" suffix); a per-row marker shows what's already added; re-adding an already-added game is refused rather than creating a duplicate.
- `iprolaunch library add-to-steam <name-or-slug> [--gamescope]` — the CLI equivalent.
- Gamepad: LT triggers Library's `s`.

### Fixed
- Any gamescope wrap (`-f`/`-w`/`-b`, a profile's remembered default, or one baked into a Steam shortcut) now automatically skips itself when already running under gamescope (Steam Game Mode always is) instead of crashing with "Gamescope WSI Layer Error".

### Changed
- Library title bar (and its shortcut legend) now marquee-scrolls when too long for the terminal instead of being cut off.

## [1.19.5] — 2026-09-06

### Added
- Configurable gamescope upscale strategy (`-S`/`--scaler`) and adaptive-sync/VRR toggle (`--adaptive-sync`), global default + per-profile override, alongside the existing upscale-filter setting.

## [1.19.4] — 2026-09-06

### Added
- Configurable gamescope output size, refresh rate, nested render resolution, upscale filter, borderless mode, and relative-mouse-mode (global default + per-profile override) — fixes `-m`/`-w` producing a small window instead of filling the screen on a normal desktop.
- `-b`: wraps a launch in a nested `gamescope -b`/`--borderless` session, combinable with `-f`/`-w`.

### Changed
- `-m` renamed to `-w` (long flag `--maximize` unchanged) — frees up `-m` for a future flag.
- README/CLI help: `-f`/`-w`/`-b` documented as not working from inside Steam Game Mode itself (a real gamescope limitation, not an overlay conflict) — only from a non-gamescope session (Desktop Mode, a bare console/SSH).

## [1.19.3] — 2026-09-06

### Fixed
- Launching a second, different game in `single` prefix mode while one was already running could hang forever — now skips the redundant wait automatically.

## [1.19.2] — 2026-09-06

### Fixed
- Killing one game while two ran concurrently in `single` prefix mode killed both — kill/liveness now key on a per-launch id instead of the shared prefix.

## [1.19.1] — 2026-09-06

### Fixed
- CI: install `libudev-dev` before building — needed by the gamepad backend.

## [1.19.0] — 2026-09-06

### Added
- TUI: gamepad navigation (D-pad, face buttons, shoulders, triggers) alongside the keyboard, via `gilrs`.
- Shortcut legends switch to gamepad captions automatically while a controller is in use, and back for the keyboard.

## [1.18.0] — 2026-09-06

### Added
- TUI Library: `p` runs `winetricks` against the selected game's own prefix.

## [1.17.2] — 2026-09-06

### Fixed
- A launch's working directory is now the target exe's own folder, not iprolaunch's.

## [1.17.1] — 2026-09-06

### Fixed
- TUI: a locked quick-search filter now still shows the tab's full shortcut legend.

## [1.17.0] — 2026-09-06

### Added
- `defaults.gamescope` (global/per-profile): remembers `-f`/`-m` per game.
- README: added a Requirements section for external tools.

### Changed
- A profile's `proton` override now only applies in per-slug prefix mode.

## [1.16.0] — 2026-09-06

### Added
- `-f`/`-m`: wrap a launch in a nested `gamescope` session (real fullscreen / stretch-to-fill).

### Changed
- TUI quick-search: Enter now locks the filter instead of immediately launching/killing.
- Bare `iprolaunch` now fails with a clear message when there's no controlling terminal.

## [1.15.1] — 2026-09-06

### Changed
- TUI Library: hides "last launched" entirely instead of showing "never".

## [1.15.0] — 2026-09-06

### Added
- TUI Library: `c` copies a ready-to-paste quick-launch command to the clipboard.
- `iprolaunch add <exe>`: registers a profile without launching it.
- `context-menu install`/`uninstall`: file-manager right-click "Add to IProLaunch Library" action (KDE/GNOME family/XFCE).
- `add`/the context-menu action also copy the quick-launch command to the clipboard.
- TUI: Help (and a new `?` popup) can scroll.
- TUI: status bar shows global key hints when idle.

### Changed
- A freshly-added profile's exe has its execute bit cleared automatically.
- Clipboard copying shells out to `wl-copy`/`xclip` instead of a Rust clipboard crate.

## [1.14.1] — 2026-09-06

### Fixed
- A Proton build under a system-wide `compatibilitytools.d` is now stored/used by absolute path instead of its bare name.

## [1.14.0] — 2026-09-06

### Added
- `integrate install`: adds an app/start-menu entry and a real app icon.
- Project icon/logo finalized.

## [1.13.0] — 2026-09-06

### Added
- TUI: `f` quick-searches Running/Library by name.

## [1.12.0] — 2026-09-06

### Changed
- `logging.mode = "each"` now saves inside each profile's own folder.

## [1.11.0] — 2026-09-06

### Changed
- `defaults.prefix_mode`'s per-exe strategy renamed to per-slug, fixing a prefix-collision gap between same-named exes.
- TUI profile editor: renaming a slug in per-slug mode now confirms first if it would also move a prefix.

## [1.10.0] — 2026-09-06

### Added
- TUI profile editor: `slug` and `name` are now editable.
- Release workflow: draft release body links to the full CHANGELOG.

## [1.9.1] — 2026-09-06

### Changed
- README opens with the same ASCII wordmark as the TUI header.

## [1.9.0] — 2026-09-06

### Added
- TUI text inputs: cursor movement, insert, and delete, not just append-only.
- `integrate`: also registers `.msi` as a default handler.

### Changed
- Marquee scroll waits 2s before moving and resets on selection change.

### Fixed
- `integrate install`'s backup now tops up missing mimetypes instead of skipping the capture entirely.

## [1.8.0] — 2026-09-06

### Added
- TUI: a too-long selected row or popup title now marquee-scrolls instead of clipping.

## [1.7.0] — 2026-09-06

### Added
- Proton scan checks every real install location (Flatpak, official Steam, system-wide).
- TUI profile editor: `target-path` is now editable.
- TUI Library: shows each game's last 2 parent directories; right-aligns "last launched".
- Config tab: desktop integration is its own table (status/location/setup/reapply/uninstall).

### Changed
- `last_launched`'s date format changed to `DD-Mon-YYYY`.

## [1.6.0] — 2026-09-06

### Added
- TUI: full per-game profile editor (Library `e`).
- TUI Library: `r` (refresh) and `d` (delete profile, with confirmation).

### Changed
- `last_launched` time format changed to `HH:MM:SS`.

## [1.5.0] — 2026-09-06

### Added
- TUI: add-by-path now prompts for a title afterward.

### Fixed
- Friendlier error when config/profile TOML fails to parse.

## [1.4.0] — 2026-09-06

### Added
- TUI Config tab: desktop integration row (install/uninstall in place).
- `integrate install` now backs up the prior default handler; `uninstall` restores it.

### Fixed
- `integrate uninstall` now actually restores the prior default instead of leaving no handler.

## [1.3.0] — 2026-09-06

### Added
- `integrate install`/`uninstall`: registers IProLaunch as the default `.exe` handler via `xdg-mime`.

## [1.2.0] — 2026-09-06

### Added
- TUI: manage global `env`/`winedlloverride` entries directly.
- Profile `args`: extra launch args always forwarded.

### Fixed
- A detached launch no longer risks a hanging pager when `auto_open` fires on failure.

## [1.1.0] — 2026-09-06

### Added
- Ctrl+C/SIGTERM during `run` now stops the sandboxed game tree, not just iprolaunch.
- `proton list` subcommand.
- `config init`: interactively pick a default Proton build.
- Automatic GAMEID lookup against the community umu-database.
- Full TUI: running list, library, config editor, help, ASCII header.
- `winedlloverride` section (global + per-profile).

## [1.0.0] — 2026-09-06

### Added
- `run` subcommand: launch an exe, auto-creating its profile.
- `library list` subcommand.
- Quick launch by profile name or slug.
- `config show` subcommand.
- Global config with per-profile overrides, prefix modes, log retention.
- On-the-fly env overrides on quick launch.
- `running list`/`running kill` subcommands.
- `windows-version` is now applied via `winetricks`.
- Post-launch summary reports whether ProtonFixes applied a fix.
- `LICENSE` (MIT).

### Changed
- `last_launched` stored as `DD-MM-YYYY, HH-MM-SS` instead of TOML datetime.
- All timestamps use local time instead of UTC.
