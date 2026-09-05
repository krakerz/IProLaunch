# iprolaunch

A small CLI/TUI launcher for running Windows apps and games through Proton on
Linux, without going through Steam.

## Description

Steam's own Proton integration only covers games added to your Steam
library. `iprolaunch` wraps [`umu-launcher`](https://github.com/Open-Wine-Components/umu-launcher)
(`umu-run`) so you can launch any Windows `.exe` through Proton from outside
Steam — with per-game config, log retention, and a quick-launch shortcut
that's a natural fit for a Steam (Deck or desktop) non-Steam-game entry.

## Features

- Global config for default Proton version, prefix mode, and environment
  variables, with per-game overrides.
- Two prefix strategies: one shared prefix for everything, or one prefix
  auto-created per exe.
- A game library: every exe you launch gets a profile automatically, listed
  by a friendly, editable name.
- Quick launch by name — `iprolaunch <name>` — for pointing a Steam
  shortcut straight at one game.
- Per-launch logging with configurable retention, and an option to keep only
  the logs from failed runs.

## Installation

Download the latest release archive, extract it, and put the `iprolaunch`
binary on your `$PATH`. Requires [`umu-launcher`](https://github.com/Open-Wine-Components/umu-launcher)
(`umu-run`) installed separately.

## Building from source

Requires a recent stable Rust toolchain (`rustup` recommended).

```sh
git clone <this-repo>
cd iprolaunch
cargo build --release
# binary at target/release/iprolaunch
```

## Usage

```sh
# Launch an exe directly — creates its library profile on first run
iprolaunch run ~/Games/EldenRing/Game/eldenring.exe

# List everything in the library
iprolaunch library list

# Quick-launch by name or slug (what a Steam shortcut should point at)
iprolaunch "eldenring#1"

# Inspect the resolved config
iprolaunch config show
```

Global config lives at `~/.config/iprolaunch/config.toml` (created with
defaults on first run — see `config/config.example.toml` for the shipped
defaults). Per-game overrides live at
`~/.config/iprolaunch/profiles/<slug>/profile.toml`, created automatically
the first time you run that exe; edit it by hand to override proton
version, prefix path, Windows version (per-exe prefix mode only), logging,
or environment variables for that one game.

## FAQ

**Do I need Steam installed?** No — `iprolaunch` only needs `umu-run` on
`$PATH`.

**Can I still add a game to Steam as a non-Steam game?** Yes — that's the
point of the quick-launch form. Point the shortcut at
`iprolaunch <name-or-slug>` instead of the exe directly.

## Status

The TUI (running processes / game library / config / help) isn't built yet
— `run`, `library list`, `config show`, and quick launch are the current CLI
surface.

---

### Notes

Built and maintained with the help of AI.
