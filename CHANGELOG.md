# Changelog

## [Unreleased]

## [1.19.0] — 2026-09-06

### Added
- TUI gamepad navigation: a real controller (a Steam Deck's, via Steam
  Input's Gamepad layout on a non-Steam-game shortcut, or any plain
  USB/Bluetooth pad) now works alongside the keyboard, read directly via
  the new `gilrs` dependency (evdev on Linux, no SDL2). D-pad moves,
  A/B/X/Y confirm/cancel/kill-or-delete/help, LB/RB switch tabs, L3/R3
  refresh/winetricks, RT confirms a delete/rename/winetricks prompt (kept
  deliberately separate from A, so mashing confirm can never delete
  anything by accident), Select searches, Start quits. Implemented as a
  translation layer only (`tui::gamepad::translate`, a `gilrs::Button` ->
  `crossterm::event::KeyCode`) — every existing key handler needed zero
  changes, since a translated gamepad press is indistinguishable from a
  real keystroke by the time it reaches them. Every shortcut legend
  (Running/Library titles, the idle status-bar hint) switches to the
  matching button captions the instant a gamepad is used, and back the
  moment a real key is pressed, via a new `App::input_kind` flag.

## [1.18.0] — 2026-09-06

### Added
- TUI Library: `p` runs `winetricks` against the exact prefix a real launch
  of the selected game would use (confirms first) — resolved through the
  same `WINEPREFIX`/Proton logic as a real launch, so it can never drift
  onto a different prefix. Works in both the plain and locked-filter list;
  the shortcut legend was checked to keep showing every key in both states.

## [1.17.2] — 2026-09-06

### Fixed
- A launch's working directory is now set to the target exe's own folder
  before spawning, matching what double-clicking it in Windows Explorer
  (or Lutris, which always sets this) gives it. Previously the spawned
  process just inherited iprolaunch's own cwd (wherever it happened to be
  run from), which broke a real game whose asset loading assumes cwd is
  its own install folder.

## [1.17.1] — 2026-09-06

### Fixed
- TUI Running/Library: once a quick-search filter is locked (Enter), the
  block title had only shown "(locked, Esc = clear)" — dropping the
  a/r/e/d/c/Enter shortcut legend, even though all of it works again once
  locked. Now shows both together.

## [1.17.0] — 2026-09-06

### Added
- `defaults.gamescope` (global) / a per-profile override, cycled in the TUI
  (Config tab and the profile editor): `none`/`fullscreen`/`maximize`,
  remembering the same choice `-f`/`-m` would set for one launch so
  `iprolaunch <slug>` doesn't need it retyped every time — an explicit
  `-f`/`-m` on the command line still wins for that one launch.
- README: a "Requirements" section listing every external tool iprolaunch
  can use, assuming a clean system — `umu-run` (required), and what each
  optional one (`xdg-utils`, `gtk-update-icon-cache`, KDE's
  `kbuildsycoca`, `wl-copy`/`xclip`, `gamescope`) is actually needed for.

### Changed
- A profile's `proton` override now only takes effect in
  `defaults.prefix_mode = per-slug`, same restriction `windows-version`
  already had — in `single` prefix mode every profile shares one prefix,
  so a mismatched Proton version from one profile's override risked
  corrupting it for the rest. The stored override itself is untouched
  (still editable, just inert until switched to per-slug).

## [1.16.0] — 2026-09-06

### Added
- `-f`/`-m` — wraps a launch (`run` or quick-launch, `iprolaunch <slug>`)
  in a nested `gamescope` session instead of spawning `umu-run` directly:
  `-f` uses gamescope's own `-f`/`--fullscreen` (a real display-mode-switch
  fullscreen), `-m` uses `--force-windows-fullscreen` (stretches the game's
  own window to fill the nested surface, regardless of the size it
  requests — the closest thing gamescope has to "maximized", since it's a
  Wayland compositor with no literal maximized-window concept). Combine
  both for `-f -m`. Useful inside an already-running gamescope session
  (Steam Game Mode/a Deck) — nesting one specifically around a single
  non-Steam-game's launch is the standard trick for forcing it to actually
  fullscreen/fill the screen, since a plain windowed Wine game won't
  otherwise switch display modes on its own. Requires `gamescope` on
  `$PATH`.

### Changed
- TUI Running/Library quick-search (`f`): Enter now *locks* the filter
  instead of immediately killing/launching — the narrowed list stays, but
  every other key (kill/refresh/launch/add/edit/delete/copy, Up/Down) goes
  back to working normally, now scoped to the filtered subset. Esc clears
  it entirely whether still typing or locked, and so does switching tabs
  (previously a filter just stuck around invisibly on a tab you'd left).
  Also fixed a related bug this surfaced: while actively typing a filter,
  digits/`q`/`?`/Tab were being intercepted by the global tab-switch/quit/
  help shortcuts before the filter ever saw them (so searching a game
  named e.g. "Dark Souls 3" didn't work) — those now correctly become
  filter text while typing, same as any other character.
- Bare `iprolaunch` (the TUI) now fails with a clear, actionable message
  when there's no controlling terminal to run in, instead of a bare
  `enable_raw_mode()` OS error — confirmed for real this is exactly what
  happens when Steam Game Mode launches a non-Steam-game shortcut directly
  (no console attached at all), which is why only `iprolaunch <slug>`
  worked there and bare `iprolaunch` silently did nothing.

## [1.15.1] — 2026-09-06

### Changed
- TUI Library list: "last launched: ..." is now hidden entirely for a
  profile that's never been launched, instead of showing "last launched:
  never".

## [1.15.0] — 2026-09-06

### Added
- TUI Library: `c` copies a ready-to-paste quick-launch command
  (`"<iprolaunch binary path>" <slug>`) to the clipboard — meant for
  pasting straight into a Steam non-Steam-game shortcut's Target field.
  Becomes a literal search character while a quick-search is active, same
  as the tab's other shortcuts.
- `iprolaunch add <exe>` — registers an exe as a library profile without
  launching it (unlike `run`, which launches and creates the profile as a
  side effect, or the TUI's own Library `a`, which also launches once).
  Reuses the existing profile if one already points at that exe.
- `iprolaunch context-menu install [kde|gnome|xfce]` (and `uninstall`) — a
  file-manager right-click "Add to IProLaunch Library" action, running
  `iprolaunch add <clicked file>`. With no DE given, installs all three
  (best-effort — one that isn't actually present just has nothing read its
  files): a KIO service menu for Dolphin (both the Plasma 6 and Plasma 5
  locations, marked executable — KDE refuses to run a service menu that
  isn't), a Nautilus-Scripts-style script for GNOME/Cinnamon/MATE
  (Nautilus/Nemo/Caja all share that convention), and a merged `<action>`
  entry in Thunar's `uca.xml` for XFCE (the only one of the three that has
  no drop-a-file-in mechanism — every other custom action already in that
  file is left untouched, matched by a stable id so re-running `install`
  updates in place instead of duplicating, and `uninstall` removes only
  that one entry). Also now folded into `integrate install`/`uninstall`
  (and the TUI's Config-tab "setup"/"reapply"/"uninstall" row, which just
  calls the same functions) — no DE detection needed, since each DE's files
  live in their own location and don't conflict with (or get read by) an
  unrelated DE, so installing every one unconditionally is harmless even on
  a single-DE system. `context-menu install`/`uninstall` remain available
  standalone too, for managing just the right-click action on its own.
- `iprolaunch add`/`context-menu`'s "Add to IProLaunch Library" also copies
  the quick-launch command to the clipboard (same one the TUI's `c` key
  produces), so a bare `add` (or the right-click action) leaves you with a
  ready-to-paste Steam Target-field command without a separate step.
- TUI: the Help tab (and a new `?` popup, see below) can now scroll —
  Up/Down by a line, PageUp/PageDown by 10, Home/End to jump to either end.
  It had grown too long to fit a normal terminal with no way to see the
  rest.
- TUI: `?` opens Help as a popup from *any* tab (Esc closes it), sharing
  the same content and scroll position as the Help tab — a quick reference
  without switching tabs and losing your place.
- TUI: the status bar shows the global keys (`q` = quit, `?` = help,
  `1`-`4`/`Tab`/`Shift-Tab` = switch tabs) whenever there's no real status
  message to show, instead of a bare "Ready.".

### Changed
- A freshly-added profile's target exe now has its Linux execute bit
  cleared automatically (best-effort). On KDE, a `+x` file gets executed
  directly on open/double-click (`kiorc`'s `[Executable scripts]
  behaviourOnLaunch=execute`, routed by the kernel's `binfmt_misc` — e.g. a
  `DOSWin` MZ-header registration straight to `/usr/bin/wine`), completely
  bypassing xdg-mime/`integrate install`'s file-type association. Windows
  exes never need `+x` on Linux (`umu-run`/wine take the path as an
  argument, never `execve` it), so clearing it is safe and is the only
  actually-scoped fix — KIO's `behaviourOnLaunch` has no per-mimetype
  override, confirmed from its own source, so anything short of this would
  mean changing that setting globally for every executable-permission file
  on the system, not just game exes.
- Clipboard copying (`c` / `add`) shells out to `wl-copy`/`xclip` instead of
  a Rust clipboard crate. Both Wayland and X11 clipboards require whoever
  "owns" the selection to stay alive to answer paste requests — fine for
  the long-running TUI, but a short-lived process like `iprolaunch add`
  exits immediately, so the clipboard read back empty afterwards; `wl-copy`/
  `xclip` already fork into the background to keep serving it, which is
  exactly what's needed here.

## [1.14.1] — 2026-09-06

### Fixed
- A Proton build found under a *system-wide* `compatibilitytools.d` (e.g.
  CachyOS's `proton-cachyos-slr` package under
  `/usr/share/steam/compatibilitytools.d`) is now stored/used by its
  absolute path instead of its bare folder name. `umu-run` only resolves a
  relative `PROTONPATH` against the user's own Steam root
  (`~/.local/share/Steam/compatibilitytools.d`, confirmed from `umu-run`'s
  own source) — never a system-wide directory — so a bare name for a
  system-wide build failed every launch with `PROTONPATH '<name>' is not
  valid, toolmanifest.vdf not found`. Official Steam-installed Proton
  (`steamapps/common/Proton*`) was already unaffected — it already used an
  absolute path. Existing configs/profiles with the old bare name need to
  be re-picked (TUI proton picker or `config init`) or hand-edited to the
  absolute path shown by `proton list`.

## [1.14.0] — 2026-09-06

### Added
- `integrate install` now also adds IProLaunch to the app/start menu (KDE,
  GNOME, etc.) as its own visible entry (separate from the existing
  `.exe`/`.bat`/`.cmd`/`.msi` file-handler entry, which stays hidden from
  menus — it only makes sense invoked with a real file), and installs a
  real app icon (an embedded SVG + 256x256 PNG, installed into the standard
  `~/.local/share/icons/hicolor` theme directories under the name
  `iprolaunch`) referenced by both `.desktop` entries. `integrate uninstall`
  removes both alongside the existing file-handler cleanup. No new runtime
  dependency — the icon bytes are embedded in the binary at compile time via
  `include_bytes!`, so a downloaded release binary installs the same icon a
  locally-built one does.
- Project icon/logo finalized: a flat, cel-shaded "IPL" mark with a tilted
  chibi-rocket mascot over a four-pane background (`assets/icon.svg` /
  `assets/icon.png`).

## [1.13.0] — 2026-09-06

### Added
- TUI: press `f` in the Running or Library tab to quick-search — typing
  narrows the visible list live by name (case-insensitive substring match).
  Up/Down and Enter (kill in Running, launch in Library) act on whatever's
  currently shown; `Esc` clears the search and restores the full list and
  that tab's normal shortcuts (`r`/`k` for Running, `a`/`r`/`e`/`d` for
  Library — those become literal search characters while a search is
  active).

## [1.12.0] — 2026-09-06

### Changed
- `logging.mode = "each"` now saves a profile's logs inside that profile's
  own folder (`~/.config/iprolaunch/profiles/<slug>/logs/`) instead of a
  shared `logs/<slug>/` split off from the global logs directory.
  `logging.mode = "single"` is unchanged (still the shared
  `~/.config/iprolaunch/logs/`, or `logging.path` if set). Existing logs
  under the old `logs/<slug>/` layout are left in place — not migrated —
  and new `each`-mode logs go straight to the new location.

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
