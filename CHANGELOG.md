# Changelog

## [Unreleased]

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
