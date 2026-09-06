# IProLaunch

A small CLI/TUI launcher for running Windows apps and games through Proton on
Linux, without going through Steam.

## Description

IProLaunch — short for **Instant Proton Launch** — wraps
[`umu-launcher`](https://github.com/Open-Wine-Components/umu-launcher)
(`umu-run`) so you can launch any Windows `.exe` through Proton from outside
Steam, since Steam's own Proton integration only covers games added to your
Steam library — with per-game config, log retention, and a quick-launch
shortcut that's a natural fit for a Steam (Deck or desktop) non-Steam-game
entry.

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
- `proton list` / `config init` to detect installed Proton builds and pick a
  default interactively.
- Automatic GAMEID matching against the community
  [umu-database](https://umu.openwinecomponents.org), refreshed periodically
  in the background — so protonfixes has a real shot at finding a fix instead
  of always falling back to a generic default.
- `Ctrl+C` during a launch, and `running list` / `running kill`, both
  reliably stop the whole sandboxed game tree, not just `iprolaunch` itself.
- A full TUI (run `iprolaunch` with no arguments) — running-games/quick-kill,
  game library (launch or add by path), a live config editor (including
  managing global `env`/`winedlloverride` entries one at a time — add, edit,
  delete), and help — for everything above without needing to remember the
  CLI subcommands.

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
# Launch the TUI — running games, library, config editor, help
iprolaunch

# Launch an exe directly — creates its library profile on first run
iprolaunch run ~/Games/EldenRing/Game/eldenring.exe

# List everything in the library
iprolaunch library list

# Quick-launch by name or slug (what a Steam shortcut should point at)
iprolaunch "eldenring#1"

# See what's currently running, and stop one
iprolaunch running list
iprolaunch running kill "eldenring#1"

# Detect installed Proton builds and pick a default
iprolaunch proton list
iprolaunch config init

# Inspect the resolved config
iprolaunch config show
```

Global config lives at `~/.config/iprolaunch/config.toml` (created with
defaults on first run — see `config/config.example.toml` for the shipped
defaults). Per-game overrides live at
`~/.config/iprolaunch/profiles/<slug>/profile.toml`, created automatically
the first time you run that exe; edit it by hand to override proton
version, prefix path, Windows version (per-exe prefix mode only), logging,
environment variables, DLL overrides (`[winedlloverride]`, e.g.
`winhttp = "n,b"` — joined into a single `WINEDLLOVERRIDES` at launch), or
extra launch args (`args = ["--dx11"]`, always forwarded to that exe, in
addition to anything passed on the command line) for that one game — or set
`title` to the game's real name (e.g. `"Grand Theft Auto V"`) so it can be
matched against the umu-database for a GAMEID, which is what lets
`umu-run`'s automatic protonfixes actually find a fix instead of a generic
default.

## FAQ

**Do I need Steam installed?** No — `iprolaunch` only needs `umu-run` on
`$PATH`.

**Can I still add a game to Steam as a non-Steam game?** Yes — that's the
point of the quick-launch form. Point the shortcut at
`iprolaunch <name-or-slug>` instead of the exe directly.

**Does it need internet access?** Only to auto-fetch the umu-database
(re-checked every 7 days by default, configurable via `gamedb`'s
`update_interval_days`) and, separately, whatever `umu-run` itself needs to
download a Proton build. Neither blocks a launch if the network's
unavailable — the launch just proceeds without a GAMEID match.

---

### Notes

Built and maintained with the help of AI.
