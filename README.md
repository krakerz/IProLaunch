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

- Global config (default Proton version, prefix mode, environment variables)
  with per-game overrides.
- Two prefix strategies: one shared prefix, or one auto-created per profile,
  keyed by its slug (renaming a slug renames its prefix dir to match, with a
  confirmation first).
- A game library — every exe you launch gets an editable profile
  automatically, or add one without launching it via `iprolaunch add <exe>`.
  `integrate install` (below) also adds a file-manager right-click "Add to
  IProLaunch Library" action (KDE/Dolphin, GNOME/Cinnamon/MATE, XFCE) for
  mass-adding games without opening a terminal — `add`/that action also
  copy a ready-to-paste quick-launch command to the clipboard.
- Quick launch by name or slug (`iprolaunch <name>`) — a natural fit for a
  Steam shortcut. The TUI's Library tab (`c`) copies a ready-to-paste
  `"<iprolaunch binary>" <slug>` command for a Steam non-Steam-game shortcut's
  Target field.
- Per-launch logging with configurable retention (optionally keeping only
  failed runs), either one shared log folder or one per profile.
- `proton list` / `config init` detect installed Proton builds across every
  common location (native/Flatpak Steam, official Steam installs,
  distro-packaged system builds), not just `compatibilitytools.d`.
- Automatic GAMEID matching against the community
  [umu-database](https://umu.openwinecomponents.org) so protonfixes has a
  real shot at finding a fix instead of falling back to a generic default.
- `Ctrl+C`, `running list` / `running kill` all reliably stop the *whole*
  sandboxed game tree, not just `iprolaunch` itself.
- A full TUI (`iprolaunch`, no arguments) — running games with quick-kill,
  a library (launch/add/edit/delete, with per-game overrides each
  independently resettable to "inherit the default"), a live config editor
  (incl. env/winedlloverride management and desktop integration), and a
  scrollable help screen covering everything above without needing to
  remember CLI subcommands — press `?` from any tab to pop it up without
  losing your place. Press `f` in Running/Library to quick-search by name;
  long text marquee-scrolls instead of clipping on a small terminal; the
  status bar shows the global keys (`q`, `?`, tab-switching) whenever
  there's nothing else to report.
- `iprolaunch integrate install` — registers IProLaunch as the default
  handler for `.exe`/`.bat`/`.cmd`/`.msi` files, adds it to the app/start
  menu with its own icon, adds the right-click "Add to IProLaunch Library"
  action to whichever of KDE/GNOME/Cinnamon/MATE/XFCE are actually present,
  and backs up whatever was the default before so `integrate uninstall` can
  restore all of it. Both are also available from the TUI's Config tab.
  `iprolaunch context-menu install [kde|gnome|xfce]` manages just the
  right-click action on its own, if you ever want only that.

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

# Register an exe as a library profile without launching it
iprolaunch add ~/Games/EldenRing/Game/eldenring.exe

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

# Register as the default .exe/.bat/.cmd/.msi handler, app-menu entry,
# and file-manager right-click "Add to IProLaunch Library" action
iprolaunch integrate install
iprolaunch integrate uninstall

# Manage just the right-click action on its own, if you ever want only that
iprolaunch context-menu install          # every supported DE
iprolaunch context-menu install kde      # just one
iprolaunch context-menu uninstall
```

Global config lives at `~/.config/iprolaunch/config.toml` (created with
defaults on first run — see `config/config.example.toml`). Per-game
overrides live at `~/.config/iprolaunch/profiles/<slug>/profile.toml`,
created automatically on first run — edit via the TUI's Library tab (`e`) or
by hand: proton version, prefix path, Windows version (per-slug prefix mode
only), logging, environment variables, DLL overrides (`[winedlloverride]`,
e.g. `winhttp = "n,b"`, joined into `WINEDLLOVERRIDES` at launch), extra args
(`args = ["--dx11"]`, always forwarded), or `title` (the game's real name,
matched against the umu-database for a GAMEID so protonfixes can find a fix).

## FAQ

**Do I need Steam installed?** No — `iprolaunch` only needs `umu-run` on
`$PATH`.

**Can I still add a game to Steam as a non-Steam game?** Yes — point the
shortcut at `iprolaunch <name-or-slug>` instead of the exe directly.

**Does it need internet access?** Only to auto-fetch the umu-database
(re-checked every 7 days by default, configurable via `gamedb`'s
`update_interval_days`) and whatever `umu-run` itself needs to download a
Proton build. Neither blocks a launch if the network's unavailable.

**After `integrate uninstall`, `.exe` files open with something else again —
is that a bug?** No — `install` backs up whichever app was the default
before it ran, and `uninstall` restores it, then deletes the backup. If
nothing was set before `install`, `uninstall` leaves it unset too.

---

### Notes

Built and maintained with the help of AI.
