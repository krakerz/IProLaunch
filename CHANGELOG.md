# Changelog

## [Unreleased]

## [1.1.0] — 2026-09-06

### Added
- Ctrl+C (and SIGTERM) during `run` now forwards into and stops the
  sandboxed game tree, instead of only killing `iprolaunch` itself
- `proton list` subcommand: lists installed Proton builds detected under
  Steam's `compatibilitytools.d`
- `config init`: interactively pick a default Proton build from what's
  installed
- Automatic GAMEID lookup against the community umu-database
  (`[gamedb]`'s `update_interval_days`, default 7, controls how often the
  local cache is refreshed; never blocks a launch on network failure) — a
  profile's optional `title` field is the lookup query, falling back to the
  exe's file stem when unset
- Full TUI (`iprolaunch`, no arguments): running-games list with quick-kill,
  a game library (launch existing entries or add a new one by exe path), a
  live config editor (proton picker, prefix mode, logging, gamedb interval),
  a help screen, and an ASCII wordmark header showing the current version
- `winedlloverride` section, in both global config and a profile override:
  one line per DLL (e.g. `winhttp = "n,b"`), joined into a single
  `WINEDLLOVERRIDES` value at launch

## [1.0.0] — 2026-09-06

### Added
- `run` subcommand: launch an exe through `umu-run`, auto-creating its
  library profile on first run
- `library list` subcommand
- Quick launch by profile name or slug (`iprolaunch <name>`), for pointing a
  Steam non-Steam-game shortcut directly at a game
- `config show` subcommand
- Global config (`~/.config/iprolaunch/config.toml`) with per-profile
  overrides, single/per-exe prefix modes, and configurable log retention
- On-the-fly env overrides on quick launch (`iprolaunch <name> KEY=VALUE...`),
  taking priority over both the profile's and global `[env]`
- `running list` / `running kill <pid-or-name>` subcommands
- `windows-version` (per-exe prefix mode only) is now actually applied to the
  prefix via `winetricks`, best-effort — a failure warns but doesn't block
  the launch; resets a stale wineserver bound to the prefix first (only when
  nothing is actively running against it) to avoid a version-mismatch failure
- One-line summary after each launch reporting whether `umu-run`'s automatic
  ProtonFixes found and applied a game-specific fix
- `LICENSE` (MIT)

### Changed
- `last_launched` in a profile's `profile.toml` is now stored as
  `DD-MM-YYYY, HH-MM-SS` instead of TOML's native datetime type
- All timestamps (log filenames, `last_launched`, running-state metadata) use
  local time instead of UTC
