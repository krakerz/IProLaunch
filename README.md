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

- Global config (Proton version, prefix mode, environment variables) with
  per-game overrides.
- Two prefix strategies: one shared prefix, or one per profile keyed by
  slug (renaming moves its prefix dir, with confirmation). A per-game
  Proton override only applies in per-slug mode.
- A game library — every launched exe gets an editable profile
  automatically, or add one without launching via `iprolaunch add <exe>`.
  `integrate install` (below) adds a file-manager right-click "Add to
  IProLaunch Library" action (KDE, GNOME/Cinnamon/MATE, XFCE); both it and
  `add` copy a ready-to-paste quick-launch command to the clipboard.
- Quick launch by name or slug (`iprolaunch <name>`) — points a Steam
  shortcut straight at a game. `-f`/`-w`/`-b` wrap the launch in a nested
  `gamescope` session (real fullscreen/stretch-to-fill/borderless,
  combinable, remembered per-game) — not usable from inside Steam Game
  Mode itself (see FAQ). Output size, render resolution, refresh cap,
  upscale filter/strategy, relative-mouse-mode, and adaptive-sync are all
  configurable too (Config tab / profile editor).
- `defaults.launch_wrapper` (global default + profile override) runs the
  whole launch through an external command — `gamemoderun`, `mangohud`, a
  frame-generation layer's own wrapper script (see "Injecting env vars or
  a wrapper tool" below).
- Per-launch logging with configurable retention, one shared log folder or
  one per profile.
- `proton list` / `config init` detect installed Proton builds everywhere
  (native/Flatpak Steam, official Steam installs, distro packages), not
  just `compatibilitytools.d`.
- Automatic GAMEID matching against the community
  [umu-database](https://umu.openwinecomponents.org) so protonfixes has a
  real shot at finding a fix instead of a generic default.
- `Ctrl+C`, `running kill` all reliably stop the *whole* sandboxed game
  tree, not just `iprolaunch` itself.
- A full TUI (`iprolaunch`, no arguments) — running games with quick-kill,
  a library (launch/add/edit/delete, overrides resettable to "inherit"), a
  live config editor, and a scrollable help screen (`?` from any tab). `f`
  quick-searches Running/Library by name.
- Library `p` runs `winetricks` against the exact prefix a real launch of
  that game would use (confirms first).
- Library `s` adds a game to Steam as a non-Steam-game shortcut, live, via
  Steam's own importer — no `shortcuts.vdf` editing, no restart required
  (press `?` in the TUI for the full behavior, including the gamescope-flag
  and duplicate-detection options).
- Gamepad navigation — a real controller works alongside the keyboard with
  zero setup (see the Help screen's "Gamepad" section for the full button
  map); every legend switches to the matching captions automatically.
- `iprolaunch integrate install` — registers as the default handler for
  `.exe`/`.bat`/`.cmd`/`.msi`, adds an app-menu entry with an icon, and the
  right-click "Add to Library" action for whichever DE is present —
  backing up the prior default so `uninstall` restores everything. Also
  available from the TUI's Config tab. `context-menu install
  [kde|gnome|xfce]` manages just the right-click action alone.

## Installation

Download the latest release archive, extract it, and put the `iprolaunch`
binary on your `$PATH`.

### Requirements

Assuming nothing's installed yet:

- **[`umu-launcher`](https://github.com/Open-Wine-Components/umu-launcher)**
  (`umu-run` on `$PATH`) — required; this is what actually launches a game
  through Proton.
- **A Proton build** — auto-managed by default (`proton = "system"`), or
  point `defaults.proton`/a profile override at one you already have
  (`proton list` detects what's installed).

Everything else is optional, each tied to one specific feature, and each
degrades gracefully (usually a clear error) if missing:

- **`xdg-utils`** (`xdg-mime`, usually already present) — `integrate
  install`/`uninstall`.
- **`gtk-update-icon-cache`** (GLib/GTK) — icon-cache refresh after
  `integrate install`; the icon still shows up without it, just maybe not
  until next login.
- **KDE**: `kbuildsycoca6`/`5` — refreshes Dolphin's right-click menu after
  `context-menu install`.
- **`wl-copy`** (Wayland) or **`xclip`** (X11) — clipboard copy (`c`, `add`,
  the right-click action).
- **[`gamescope`](https://github.com/ValveSoftware/gamescope)** — `-f`/`-w`/`-b`.
  Already on SteamOS/a Deck; otherwise a distro package. Only reliable from
  a session that isn't already gamescope itself — see the FAQ for why it
  fails from inside Steam Game Mode.
- **`winetricks`** — Library's `p` key.

## Building from source

Requires a recent stable Rust toolchain (`rustup` recommended), plus
`libudev`'s development headers (gamepad support's `gilrs` dependency needs
them to build on Linux) — `libudev-dev` on Debian/Ubuntu,
`systemd-devel`/`libudev-devel` on Fedora/openSUSE, `eudev-libs`/`libudev`
(with headers) on other distros. Already present on most desktop Linux
installs; if `cargo build` fails looking for `libudev.pc`, that's this.

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

# Same, wrapped in a nested gamescope session — real fullscreen, or
# stretch-to-fill (gamescope's closest thing to "maximized"), or borderless
iprolaunch -f "eldenring#1"
iprolaunch -w "eldenring#1"
iprolaunch -b "eldenring#1"

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

## Running the full TUI from a Steam shortcut (Game Mode)

`iprolaunch <name-or-slug>` (see the FAQ below) is the simplest way to put
one specific game on a Steam Deck/Game Mode shortcut, but it only launches
that one game — no browsing the library, no config editing. To get the
*whole* TUI usable from Game Mode instead, point the shortcut at a terminal
emulator that wraps `iprolaunch`, e.g. [Alacritty](https://alacritty.org/):

```
Target:         /usr/bin/alacritty
Launch Options: -e /home/deck/.local/bin/iprolaunch
```

(swap the path for wherever the binary actually lives — check with `which
iprolaunch` first). Two things to also set up, both one-time:

- **Disable the Steam Overlay for this shortcut** (right-click it →
  Properties → General → uncheck "Enable the Steam Overlay while
  in-game"). Steam injects its overlay (`LD_PRELOAD`) into every shortcut
  it launches, Steam or non-Steam — a plain terminal emulator isn't what
  that's meant for, and it can crash the shortcut near-instantly with an
  error like `LD_PRELOAD: wrong ELF class`, with no visible message (the
  window just flashes and closes). If you ever hit that on a different
  terminal/setup, add Alacritty's `--hold` flag temporarily (`Launch
  Options: --hold -e ...`) — it keeps the window open after the child exits
  instead of always closing it, so you can actually read the error.
- **Set the shortcut's Controller Layout to "Gamepad"** (its own Properties
  → Controller Layout) if you want to navigate the TUI with a controller —
  see the TUI's Help screen's "Gamepad" section for the full button map.
  The default "Desktop" layout emulates a keyboard/mouse instead of a real
  joystick, which the TUI can't read directly.

Global config lives at `~/.config/iprolaunch/config.toml` (created with
defaults on first run — see `config/config.example.toml`). Per-game
overrides live at `~/.config/iprolaunch/profiles/<slug>/profile.toml` —
edit via the TUI's Library tab (`e`) or by hand: proton/prefix
path/Windows version (per-slug mode only)/gamescope, logging, env vars,
DLL overrides (`[winedlloverride]`, e.g. `winhttp = "n,b"`), extra args
(`args = ["--dx11"]`, always forwarded), or `title` (matched against the
umu-database for a GAMEID).

## Injecting env vars or a wrapper tool via a Steam shortcut

Once a game's been added to Steam (Library `s`, see Features above — or
Steam's own "Add a Non-Steam Game" browse dialog), its Launch Options field
is a normal Steam non-Steam-game shortcut, so anything the wider Steam/Proton
community already does with `%command%` works here too — `MangoHud`,
`gamemoderun`, a frame-generation layer like
[`lsfg-vk`](https://lsfg-vk.dev/), env vars, all the same way as any other
shortcut. This is separate from (and doesn't need touching) this profile's
own `env`/`gamescope` config in `iprolaunch` itself — anything set this way
in Steam's own Launch Options is inherited straight down through
`iprolaunch` → `umu-run` → Proton → the game, the same path `env`/
`winedlloverride` already use, confirmed for real (checked `/proc/<pid>/
environ` at every level of the process tree, including inside Proton's own
sandbox).

**The one thing to get right: ordering.** Once `%command%` appears anywhere
in Launch Options, Steam runs the *whole* line as one shell command,
substituting `%command%` with the resolved `iprolaunch` path alone — and a
shell always treats the first word as the program to run. So:

- A wrapper tool has to come *first*, with `%command%` right after it as its
  argument — and anything meant for `iprolaunch` itself (e.g. `-f`, or the
  slug, if you're editing Launch Options by hand rather than through
  Library `s`) has to come *after* `%command%`, not before it:

  ```
  ~/lsfg %command% "-f" "app"
  ```

  Putting `"-f" "app"` *before* the wrapper instead (`"-f" "app" ~/lsfg
  %command%`) makes the shell try to run a program literally named `-f`,
  which fails silently — no visible error, the shortcut just doesn't open.
- A plain env var doesn't need a wrapper script at all — `lsfg-vk` itself is
  fully controlled by environment variables (`LSFG_MULTIPLIER`,
  `LSFG_LEGACY`, etc., see its own docs), so this works exactly the same
  way, no `~/lsfg` script required:

  ```
  LSFG_MULTIPLIER=2 %command% "-f" "app"
  ```

Confirmed working for real: toggling a frame-generation layer on/off this
way visibly changes the game's reported FPS, same as it would launched any
other way.

### The same thing, native to iprolaunch instead

If you'd rather have a wrapper tool (not just env vars — those already just
go in `env`, global or per-profile) apply regardless of *how* the game gets
launched — `run`, quick-launch, or a Steam shortcut pointed back at
`iprolaunch <slug>` — set `defaults.launch_wrapper` (global) or a profile's
own override instead (Config tab / profile editor, blank = none). It wraps
the *entire* launch the same way a Steam Launch Options wrapper +
`%command%` does, no ordering gotcha to get right since there's no
`%command%` token to place — just the bare command:

```toml
[defaults]
launch_wrapper = "~/lsfg"
```

or per-profile, in `profiles/<slug>/profile.toml`:

```toml
[defaults]
launch_wrapper = "gamemoderun"
```

Whitespace-split only (no shell-quoting support, same as `args`); a leading
`~/` in the command itself is expanded.

## FAQ

**Do I need Steam installed?** No — `iprolaunch` only needs `umu-run` on
`$PATH`.

**Can I still add a game to Steam as a non-Steam game?** Yes — point the
shortcut at `iprolaunch <name-or-slug>` instead of the exe directly. Bare
`iprolaunch` opens the TUI, which needs a real terminal, so it won't work
as a Game Mode/gamescope shortcut's Target on its own — either use the
slug form for a single game, or see "Running the full TUI from a Steam
shortcut" above to wrap it in a terminal emulator instead.

**Why does `-f`/`-w`/`-b` fail with "Gamescope WSI Layer Error / Hooking has
failed somewhere" in Steam Game Mode?** gamescope's own WSI layer
deliberately disables its swapchain hook when it detects it's nested inside
another gamescope session, which Game Mode always is (confirmed against
gamescope's own source, not assumed) — not fixable by disabling an overlay.
Works fine from a session that isn't already gamescope (Desktop Mode, a bare
console/SSH); this is also why any gamescope wrap now automatically skips
itself instead of crashing when it detects it's already running under one.

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
