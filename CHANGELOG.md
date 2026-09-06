# Changelog

## [Unreleased]

## [1.11.0] — 2026-09-06

### Changed
- `defaults.prefix_mode`'s per-exe strategy is now **per-slug**: the prefix
  directory is keyed by a profile's own (already-disambiguated) slug
  instead of being re-derived from the exe path on every launch. This
  fixes a real gap — two different exes that happened to share a file
  stem (e.g. two unrelated `game.exe`s) previously got separate profiles
  but silently shared the same Wine prefix; now each gets its own
  (`game`, `game-2`, ...), matching their profiles. Existing
  `config.toml`s with `prefix_mode = "per-exe"` keep loading (an alias),
  and get rewritten to `"per-slug"` on next save.
- TUI profile editor: renaming a profile's `slug` while
  `prefix_mode = "per-slug"` now warns and asks for confirmation first if
  a prefix directory already exists under the old slug — confirming
  renames that directory to match, so the app always finds the right
  prefix under the profile's current slug; declining leaves both
  untouched. Skipped entirely (no prefix to move yet) if the game's never
  been launched, or if prefix mode is `single`.

## [1.10.0] — 2026-09-06

### Added
- TUI profile editor: `slug` (the profile's folder name) and `name` (the
  library display name) are now editable. Both are edited as just their
  base text — `slug` renames the folder on disk, auto-appending `-N` only
  if that exact text collides with another profile; `name`'s `#N` is
  never typed, it's auto-filled to the lowest number not already used by
  another profile with the same base (reusing a gap left by a
  deleted/renamed profile, same as a brand-new profile's numbering
  already did).
- Release workflow's draft release body now ends with a "Full changelog"
  link to `CHANGELOG.md` on the default branch.

## [1.9.1] — 2026-09-06

### Changed
- README now opens with the same ASCII-art wordmark as the TUI header,
  instead of a plain `# IProLaunch` heading.

## [1.9.0] — 2026-09-06

### Added
- TUI text inputs (config/profile-editor field edits, add-by-path, the
  title prompt) now support Left/Right cursor movement, insert, and delete
  at the cursor position — not just always-append-at-the-end — shown as a
  reverse-video block cursor. Makes fixing one segment of a path (e.g. a
  different parent folder) easy without retyping the whole thing.
- `iprolaunch integrate` now also registers `.msi` installers as a default
  handler, alongside the existing `.exe`/`.bat`/`.cmd` — `iprolaunch run`
  already launched all of these correctly with no extra wrapping (`wine
  <path>` dispatches to `cmd`/`msiexec` internally by extension); this
  extends file-manager double-click association to match.

### Changed
- The marquee scroll added in 1.8.0 now waits 2 seconds before it starts
  moving, and always resets to position 0 whenever the selected row or open
  popup/field changes — previously it scrolled immediately and kept a
  single shared position that could pick up mid-scroll after moving the
  cursor to something new.

### Fixed
- `integrate install`'s backup now tops up any mimetype missing from an
  already-existing backup (rather than skipping the capture entirely once
  the file exists at all) — matters whenever the set of mimetypes IProLaunch
  manages grows (as it just did, twice, in this release): without this, a
  mimetype added after a system was already integrated would have nothing
  recorded to restore on a later `uninstall`.

## [1.8.0] — 2026-09-06

### Added
- TUI: a selected row or a popup title too long to fit a small terminal now
  scrolls (marquee-style) instead of being silently clipped, so the full
  text is still readable — the Config/Integrate/Library/profile-editor
  lists' selected row, and every popup's title (edit prompts, the proton
  picker, the env/winedlloverride entry editor). Animated by the TUI's
  existing idle redraw (already ticks ~4/sec even with no key pressed), so
  no extra timer/thread was needed.

## [1.7.0] — 2026-09-06

### Added
- Proton scan (`proton list` and the TUI's proton picker alike) now checks
  every place this machine might have a Proton build, not just the native
  `~/.local/share/Steam/compatibilitytools.d`: the `~/.steam/steam`/
  `~/.steam/root` symlinks some distros set up (deduped), Flatpak Steam's
  data dir, official Steam-installed Proton under `steamapps/common`
  (recognized by its `proton` script — an incomplete/pending download is
  correctly skipped), and the system-wide
  `/usr/share/steam/compatibilitytools.d` some distro packages install a
  default build into.
- TUI profile editor: `target-path` (the exe location) is now an editable
  field — checked against the real filesystem before being accepted, so a
  typo or a moved/deleted exe can't silently leave a profile pointing at
  nothing.
- TUI Library list now shows each game's last 2 parent directory names
  (e.g. `[..\Downloads\Programs]`) so two profiles that happen to share an
  exe filename are easy to tell apart at a glance, and "last launched" is
  right-aligned to the row's edge instead of immediately following the rest
  of the row.
- Config tab: desktop integration is now its own separate table below the
  main field list (sharing one continuous selection cursor with it), with
  five rows instead of one toggle: status, binary location (the actual
  registered path, read from the `.desktop` file itself — not just this
  process's own binary path, so it's accurate even if the registered
  binary was moved since), setup, reapply (re-points the registration at
  the current binary's path without touching the saved backup of the prior
  default), and uninstall.

### Changed
- `Profile.last_launched`'s date now reads `DD-Mon-YYYY` (e.g. `06-Sep-2026`)
  instead of `DD-MM-YYYY` — the time part is unchanged (`HH:MM:SS`).

## [1.6.0] — 2026-09-06

### Added
- TUI: full per-game profile editor, from the Library tab. `e` on a
  selected game opens it — title, args, and overrides for proton,
  prefix_path, windows-version, logging.keep/record/auto_open, and
  env/winedlloverride, each independently settable back to "inherit the
  global default". Reuses the same picker/map-editor machinery as the
  global Config tab.
- TUI: Library also gets `r` (refresh the list from disk) and `d` (delete
  the selected game's profile, with a y/N confirmation first — removes only
  `profile.toml`, never the exe).
- `Profile::last_launched` now formats its time as `HH:MM:SS` (colons)
  instead of `HH-MM-SS` (hyphens) — `DD-MM-YYYY, HH:MM:SS` overall. The
  library's display label was also clarified from "last:" to
  "last launched:".

## [1.5.0] — 2026-09-06

### Added
- TUI: after adding a game via Library's `a` (add-by-path), a follow-up
  prompt asks for the game's real title (blank to skip) — the same thing
  hand-editing `profile.toml` afterward was needed for, so a freshly-added
  game can get a proper GAMEID match on its very first launch.
- Friendlier error when `config.toml`/a profile's `profile.toml` fails to
  parse: still shows the exact line/column from the TOML parser, but now
  leads with a plain-English common-fix hint (quote your text values) and
  an escape hatch (delete the file — `config init`/next launch regenerates
  it) instead of only the raw parser diagnostic.

### Notes
- Confirmed `.bat` files already launch correctly through `iprolaunch run`
  with zero code changes needed — `wine <path>.bat` runs it directly (no
  `cmd.exe /c` wrapper required), and nothing in IProLaunch gates on file
  extension.
- Investigated switching `windows-version` from system winetricks to
  `protontricks`: not viable for a non-Steam wrapper like this one —
  `protontricks` operates on an `APPID` from your Steam library, which a
  plain `iprolaunch`-launched exe never has.

## [1.4.0] — 2026-09-06

### Added
- TUI: Config tab now has a "desktop integration" row, below the rest of
  the config fields, that installs/uninstalls IProLaunch as the default
  `.exe` handler in place — no need to drop to a shell for
  `iprolaunch integrate install`/`uninstall` anymore.
- `integrate install` now backs up whatever was the default handler for
  each mimetype *before* it overwrites it; `integrate uninstall` restores
  that prior default and then deletes the backup, so a later `install`
  always captures fresh state instead of restoring stale data a second
  time.

### Fixed
- `integrate uninstall` now actually restores whatever was the default
  before `install` ran, instead of just clearing IProLaunch's own
  registration and leaving the mimetype with no default at all.

## [1.3.0] — 2026-09-06

### Added
- `iprolaunch integrate install`/`uninstall`: registers IProLaunch as the
  default handler for Windows `.exe` files (`application/x-msdownload` /
  `application/x-ms-dos-executable`) via `xdg-mime`, so double-clicking one
  in a file manager runs it through IProLaunch — detached (no terminal
  window) by default. `uninstall` only removes IProLaunch's own
  registration; it does not restore whatever was the default before
  `install` ran.

## [1.2.0] — 2026-09-06

### Added
- TUI: manage global `env`/`winedlloverride` entries directly — `a` add,
  `e` edit, `d` delete, each a simple name-then-value prompt (no need to
  type `KEY=VALUE` syntax yourself)
- Profile `args`: extra launch args always forwarded to that exe (e.g.
  `args = ["--dx11"]`), supplementing rather than replacing anything passed
  via `run ... -- extra` or quick-launch trailing args

### Fixed
- A detached launch (no controlling terminal) no longer risks a broken/
  hanging pager when `auto_open` fires on failure — skipped when stdout
  isn't a terminal, since the log file is still there to inspect afterward

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
