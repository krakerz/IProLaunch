```
    ________             __                           __
   /  _/ __ \_________  / /   ____ ___  ______  _____/ /_
   / // /_/ / ___/ __ \/ /   / __ `/ / / / __ \/ ___/ __ \
 _/ // ____/ /  / /_/ / /___/ /_/ / /_/ / / / / /__/ / / /
/___/_/   /_/   \____/_____/\__,_/\__,_/_/ /_/\___/_/ /_/
```

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
  auto-created per profile — keyed by the profile's own slug, so renaming a
  profile's slug renames its prefix directory to match (the TUI warns and
  confirms first, since it's a real directory move).
- A game library: every exe you launch gets a profile automatically, listed
  by a friendly, editable name.
- Quick launch by name — `iprolaunch <name>` — for pointing a Steam
  shortcut straight at one game.
- Per-launch logging with configurable retention, and an option to keep only
  the logs from failed runs.
- `proton list` / `config init` to detect installed Proton builds and pick a
  default interactively — checks every common install location (native and
  Flatpak Steam, official Steam-installed builds, and distro-packaged
  system-wide builds), not just `compatibilitytools.d`.
- Automatic GAMEID matching against the community
  [umu-database](https://umu.openwinecomponents.org), refreshed periodically
  in the background — so protonfixes has a real shot at finding a fix instead
  of always falling back to a generic default.
- `Ctrl+C` during a launch, and `running list` / `running kill`, both
  reliably stop the whole sandboxed game tree, not just `iprolaunch` itself.
- A full TUI (run `iprolaunch` with no arguments) — running-games/quick-kill,
  a game library (launch, add by path with a follow-up title prompt,
  refresh, and a per-game profile editor — target-path (validated against
  the real filesystem on save), slug (the folder name — renames it on disk)
  and name (the library display name — its `#N` is app-managed, auto-filling
  the lowest number not already taken by another profile) are edited as
  just their base text; title/args/proton/prefix_path/windows-version/
  logging/env/winedlloverride are overrides, each independently settable
  back to "inherit the global default" — plus delete, with a confirmation
  first), a live config editor (including managing global
  `env`/`winedlloverride` entries one at a time — add, edit, delete — and a
  separate desktop integration table: status, binary location, setup,
  reapply, uninstall), and help — for everything above without needing to
  remember the CLI
  subcommands. On a small terminal, a selected row or popup title too long
  to fit scrolls (marquee-style) instead of getting clipped.
- `iprolaunch integrate install` — registers IProLaunch as the default
  handler for Windows `.exe`, `.bat`/`.cmd`, and `.msi` files, so double-clicking
  one in a file manager (Dolphin, Nautilus, Thunar, ...) runs it through
  IProLaunch automatically.
  `integrate uninstall` removes that registration and restores whatever was
  the default before `install` ran. Both are also available from the TUI's
  Config tab, below the rest of the config fields.

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

# Make double-clicking a .exe/.bat/.cmd/.msi in your file manager launch it via IProLaunch
iprolaunch integrate install
iprolaunch integrate uninstall
```

Global config lives at `~/.config/iprolaunch/config.toml` (created with
defaults on first run — see `config/config.example.toml` for the shipped
defaults). Per-game overrides live at
`~/.config/iprolaunch/profiles/<slug>/profile.toml`, created automatically
the first time you run that exe — edit it via the TUI's Library tab (`e` on
a game) or by hand: override proton version, prefix path, Windows version
(per-slug prefix mode only), logging, environment variables, DLL overrides
(`[winedlloverride]`, e.g. `winhttp = "n,b"` — joined into a single
`WINEDLLOVERRIDES` at launch), or extra launch args (`args = ["--dx11"]`,
always forwarded to that exe, in addition to anything passed on the command
line) for that one game — or set `title` to the game's real name (e.g.
`"Grand Theft Auto V"`) so it can be matched against the umu-database for a
GAMEID, which is what lets `umu-run`'s automatic protonfixes actually find a
fix instead of a generic default.

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

**After `integrate uninstall`, `.exe` files open with something else again —
is that a bug?** No — that's by design. `install` backs up whichever app was
the default before it ran (per mimetype), and `uninstall` restores that
backup, then deletes it — so if you `install` again later, it captures
whatever's current at that point, not the stale pre-IProLaunch state. If
nothing was set before `install`, `uninstall` leaves it unset too, and your
file manager will prompt you to pick an app, same as if nothing had ever
been set.

---

### Notes

Built and maintained with the help of AI.
