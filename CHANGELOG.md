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
