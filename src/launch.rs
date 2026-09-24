use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::config::{AutoOpenScope, Config, Effective, Profile, RecordMode};
use crate::gamedb;
use crate::logging::LogSession;
use crate::prefix;
use crate::running;
use crate::terminal;

#[derive(Default)]
pub struct RunOptions {
    pub proton: Option<String>,
    pub prefix: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub args: Vec<String>,
    pub gamescope: GamescopeMode,
    /// Set by the TUI's own launch call site — see
    /// `AutoOpenScope::TuiOnly`'s doc comment for why this gates whether a
    /// failed launch's log gets auto-opened in a new terminal at all.
    pub from_tui: bool,
}

/// `-f`/`-w`/`-b` (CLI) — wraps the launch in a nested `gamescope` session
/// instead of spawning `umu-run` directly, forcing a plain windowed Wine
/// game that won't otherwise switch display modes to actually fill the
/// screen. Only works from a session that isn't *already* gamescope
/// (Desktop Mode, a bare console/SSH) — confirmed via a real failure
/// ("Gamescope WSI Layer Error / Hooking has failed somewhere") and
/// gamescope's own source that it does NOT work from inside Steam Game
/// Mode: gamescope's WSI layer explicitly detects it's nested inside
/// another gamescope session (`WAYLAND_DISPLAY` != `GAMESCOPE_WAYLAND_
/// DISPLAY`) and deliberately disables its own swapchain hook in that
/// case — not fixable by disabling an overlay, a structural limitation.
/// Real gamescope flags confirmed against the actually-installed
/// `gamescope --help` (3.16.25), not guessed:
/// - `-f` → gamescope's own `-f`/`--fullscreen` (nested mode option — an
///   actual display-mode-switching fullscreen for the nested window).
/// - `-w` → gamescope's `--force-windows-fullscreen` (stretches whatever
///   window the game itself opens to fill the nested surface, regardless
///   of the size it requests) — gamescope has no literal "maximized"
///   concept (it's a Wayland compositor, not an X11 window manager); this
///   is the closest real equivalent, confirmed with the user directly
///   rather than guessed. (iprolaunch's own `--maximize` long flag name —
///   only the short letter changed, to leave `-m` free.)
/// - `-b` → gamescope's own `-b`/`--borderless` (nested mode option — no
///   window decorations, no exclusive mode-switch; distinct from `-f`'s
///   real fullscreen).
///
/// Combining any of the three is allowed (stacks their args, `-f` first).
#[derive(Default, Clone, Copy)]
pub struct GamescopeMode {
    pub fullscreen: bool,
    pub maximize: bool,
    pub borderless: bool,
}

impl GamescopeMode {
    fn is_active(self) -> bool {
        self.fullscreen || self.maximize || self.borderless
    }

    fn args(self) -> Vec<&'static str> {
        let mut args = Vec::new();
        if self.fullscreen {
            args.push("-f");
        }
        if self.maximize {
            args.push("--force-windows-fullscreen");
        }
        if self.borderless {
            args.push("-b");
        }
        args
    }
}

/// This profile's own `-f`/`-w`/`-b` — iprolaunch's own CLI letters, not
/// gamescope's (distinct from `GamescopeMode::args()`, which are gamescope's
/// own flags for the `gamescope` subprocess) — as resolved from its
/// `Effective` config (the same values a plain `iprolaunch <slug>` with no
/// CLI flags at all already applies automatically). Used by
/// `steam_shortcut.rs` to optionally bake the equivalent of typing these by
/// hand into a Steam shortcut's own Launch Options, each as its own token
/// (confirmed real Steam behavior: one value per quoted Launch-Options
/// segment, e.g. `"-w" "<slug>"` — a single segment with a space inside,
/// like `"-w <slug>"`, does not work).
pub fn iprolaunch_cli_flags_for(effective: &Effective) -> Vec<&'static str> {
    let mut flags = Vec::new();
    if let Some(flag) = gamescope_setting_cli_flag(effective.gamescope) {
        flags.push(flag);
    }
    if effective.gamescope_settings.borderless == Some(true) {
        flags.push("-b");
    }
    flags
}

/// A bare `GamescopeSetting` mapped to iprolaunch's own CLI flag (`None` for
/// `GamescopeSetting::None` — no flag at all), with no borderless axis of
/// its own. Shared by `iprolaunch_cli_flags_for` above (which folds in its
/// own separate `gamescope_settings.borderless` override) and Sunshine's own
/// dedicated `sunshine.gamescope` setting (`tui/library.rs`'s
/// `add_to_sunshine`, which folds in its own separate
/// `sunshine.borderless` the same way).
pub fn gamescope_setting_cli_flag(
    setting: crate::config::GamescopeSetting,
) -> Option<&'static str> {
    match setting {
        crate::config::GamescopeSetting::None => None,
        crate::config::GamescopeSetting::Fullscreen => Some("-f"),
        crate::config::GamescopeSetting::Maximize => Some("-w"),
    }
}

impl From<crate::config::GamescopeSetting> for GamescopeMode {
    fn from(setting: crate::config::GamescopeSetting) -> Self {
        match setting {
            crate::config::GamescopeSetting::None => GamescopeMode::default(),
            crate::config::GamescopeSetting::Fullscreen => GamescopeMode {
                fullscreen: true,
                maximize: false,
                borderless: false,
            },
            crate::config::GamescopeSetting::Maximize => GamescopeMode {
                fullscreen: false,
                maximize: true,
                borderless: false,
            },
        }
    }
}

/// Extra `gamescope` flags from `defaults.gamescope_settings`/a profile
/// override — `-W`/`-H` (real output size, gamescope only auto-detects this
/// when it owns the display directly, not nested in an existing desktop
/// session), `-r` (refresh rate cap), `-w`/`-h` (the game's own internal
/// render resolution), `-F` (upscale filter when nested/output resolutions
/// differ), `-S` (upscale strategy, pairs with `-F`), `--force-grab-cursor`
/// (relative mouse mode, config-only — see `GamescopeSettings::grab_cursor`'s
/// doc comment for why it's not a CLI flag), `--adaptive-sync` (VRR,
/// config-only, no CLI equivalent). `borderless` is handled separately,
/// merged into `GamescopeMode` alongside the CLI `-b` flag instead (see
/// `run`) — everything else here is only meaningful, and only ever
/// appended, when `gamescope` is actually wrapping the launch at all
/// (`GamescopeMode::is_active()`).
fn gamescope_settings_args(settings: &crate::config::GamescopeSettings) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(w) = settings.output_width {
        args.push("-W".to_string());
        args.push(w.to_string());
    }
    if let Some(h) = settings.output_height {
        args.push("-H".to_string());
        args.push(h.to_string());
    }
    if let Some(r) = settings.refresh {
        args.push("-r".to_string());
        args.push(r.to_string());
    }
    if let Some(w) = settings.nested_width {
        args.push("-w".to_string());
        args.push(w.to_string());
    }
    if let Some(h) = settings.nested_height {
        args.push("-h".to_string());
        args.push(h.to_string());
    }
    if let Some(filter) = settings.filter {
        args.push("-F".to_string());
        args.push(gamescope_filter_value(filter).to_string());
    }
    if let Some(scaler) = settings.scaler {
        args.push("-S".to_string());
        args.push(gamescope_scaler_value(scaler).to_string());
    }
    if settings.grab_cursor == Some(true) {
        args.push("--force-grab-cursor".to_string());
    }
    if settings.adaptive_sync == Some(true) {
        args.push("--adaptive-sync".to_string());
    }
    args
}

/// True when this process is already running inside an existing `gamescope`
/// session — Steam Game Mode always is one. Checked via
/// `GAMESCOPE_WAYLAND_DISPLAY`, the same environment variable gamescope
/// itself sets for every child process, and the same one its own WSI
/// layer's `isRunningUnderGamescope()` reads (confirmed straight from
/// gamescope's real source, `layer/VkLayer_FROG_gamescope_wsi.cpp`, not
/// guessed) — its presence means some gamescope instance already owns this
/// session's display.
fn already_under_gamescope() -> bool {
    std::env::var_os("GAMESCOPE_WAYLAND_DISPLAY").is_some()
}

/// Whether to actually spawn our own nested `gamescope` for this launch:
/// `requested` (a CLI flag, a profile's remembered default, or a flag baked
/// into a Steam shortcut's Launch Options — see `steam_shortcut.rs`) AND we
/// aren't already running inside one. Nesting gamescope inside gamescope is
/// exactly the scenario that produces "Gamescope WSI Layer Error / Hooking
/// has failed somewhere" (see `GamescopeMode`'s doc comment above) — Steam
/// Game Mode always puts every launch inside its own outer gamescope, so
/// this makes every gamescope-wrapping launch path automatically safe
/// there instead of crashing, with no new flag for a user to remember.
fn should_wrap_with_gamescope(requested: bool, already_nested: bool) -> bool {
    requested && !already_nested
}

/// Splits a `launch_wrapper` config string into a program + its own args —
/// whitespace-only, no shell-quoting support (same convention `profile.args`
/// already uses). Expands a leading `~/` in just the program itself (e.g.
/// `~/lsfg`), the same as `config::expand_home` does for other path-like
/// config values — the wrapper's own args are passed through as-is.
fn wrapper_argv(wrapper: &str) -> Vec<String> {
    let mut parts: Vec<String> = wrapper.split_whitespace().map(String::from).collect();
    if let Some(program) = parts.first_mut() {
        *program = crate::config::expand_home(program)
            .to_string_lossy()
            .into_owned();
    }
    parts
}

fn gamescope_filter_value(filter: crate::config::GamescopeFilter) -> &'static str {
    use crate::config::GamescopeFilter::{Fsr, Linear, Nearest, Nis, Pixel};
    match filter {
        Linear => "linear",
        Nearest => "nearest",
        Fsr => "fsr",
        Nis => "nis",
        Pixel => "pixel",
    }
}

fn gamescope_scaler_value(scaler: crate::config::GamescopeScaler) -> &'static str {
    use crate::config::GamescopeScaler::{Auto, Fill, Fit, Integer, Stretch};
    match scaler {
        Auto => "auto",
        Integer => "integer",
        Fit => "fit",
        Fill => "fill",
        Stretch => "stretch",
    }
}

/// Launches `target` through `umu-run`. Looks up (or creates, on first run)
/// the matching profile so its overrides apply automatically — this is what
/// makes a game show up in the library after a single `run` invocation, with
/// no separate "add" step required. CLI-level overrides in `opts` win over
/// both the profile and global config (see `Config::effective`).
pub fn run(cfg: &Config, target: &Path, opts: RunOptions) -> Result<()> {
    let target = target
        .canonicalize()
        .with_context(|| format!("target executable not found: {}", target.display()))?;

    let (slug, mut profile) = ensure_profile(&target)?;

    let mut effective = cfg.effective(Some(&profile));
    if let Some(proton) = opts.proton {
        effective.proton = proton;
    }

    gamedb::refresh_if_stale(cfg.gamedb.update_interval_days);
    let gameid_query = profile.title.clone().unwrap_or_else(|| {
        target
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string()
    });
    let gameid = gamedb::lookup_gameid(&gameid_query);
    match &gameid {
        Some(id) => eprintln!("iprolaunch: matched GAMEID {id} for \"{gameid_query}\""),
        None => eprintln!("iprolaunch: no umu-database match for \"{gameid_query}\""),
    }

    let prefix_path = opts
        .prefix
        .unwrap_or_else(|| prefix::resolve(&effective, &slug));
    fs::create_dir_all(&prefix_path)
        .with_context(|| format!("creating prefix dir {}", prefix_path.display()))?;

    if let Err(err) = apply_windows_version(
        &prefix_path,
        effective.windows_version.as_str(),
        effective.winearch.as_str(),
    ) {
        // Best-effort: a failed registry tweak shouldn't block the game itself.
        eprintln!(
            "iprolaunch: warning: failed to apply windows-version={}: {err:#}",
            effective.windows_version.as_str()
        );
    }

    // Uniquely tags this one launch's whole process tree — see
    // `running::matching_pids_for_launch`'s doc comment for why `WINEPREFIX`
    // alone can't do this (every profile shares one in `single` prefix mode).
    let launch_id = running::new_launch_id();
    install_signal_forwarding(&launch_id);

    // CLI-passed `-f`/`-w` win when actually typed (consistent with every
    // other `opts` override beating the profile/global config); otherwise
    // fall back to whatever the profile/global `gamescope` setting already
    // remembers, so `iprolaunch <slug>` doesn't need `-f`/`-w` every time.
    // `-b`/borderless is independent of that pair (it isn't part of the
    // mutually-exclusive `GamescopeSetting` enum, so there's no "whole unit"
    // to override) — it's simply on if *either* the CLI flag or the
    // remembered `gamescope_settings.borderless` override says so.
    let mut gamescope = if opts.gamescope.fullscreen || opts.gamescope.maximize {
        opts.gamescope
    } else {
        effective.gamescope.into()
    };
    gamescope.borderless =
        opts.gamescope.borderless || effective.gamescope_settings.borderless.unwrap_or(false);

    let already_nested = already_under_gamescope();
    if gamescope.is_active() && already_nested {
        eprintln!(
            "iprolaunch: already running under gamescope (e.g. Steam Game Mode) — skipping the \
             nested wrap (it would fail with \"Gamescope WSI Layer Error\"); launching directly \
             instead."
        );
    }
    let wrap_with_gamescope = should_wrap_with_gamescope(gamescope.is_active(), already_nested);

    // What iprolaunch would spawn directly, absent a `launch_wrapper` —
    // built as program + args first (rather than straight into a `Command`)
    // so a wrapper (below) can prepend itself ahead of all of it.
    // Per `man umu`: WINEPREFIX/PROTONPATH/GAMEID are all optional env vars;
    // GAMEID defaults to "umu-default" when unset.
    let (inner_program, mut inner_args): (&str, Vec<OsString>) = if wrap_with_gamescope {
        let mut args: Vec<OsString> = gamescope.args().into_iter().map(OsString::from).collect();
        args.extend(
            gamescope_settings_args(&effective.gamescope_settings)
                .into_iter()
                .map(OsString::from),
        );
        args.push(OsString::from("--"));
        args.push(OsString::from("umu-run"));
        ("gamescope", args)
    } else {
        ("umu-run", Vec::new())
    };
    inner_args.push(target.as_os_str().to_os_string());
    inner_args.extend(profile.args.iter().map(OsString::from)); // profile's own defaults first, e.g. `--dx11`
    inner_args.extend(opts.args.iter().map(OsString::from)); // then CLI/quick-launch args, supplementing rather than replacing

    // `defaults.launch_wrapper`/a profile override (e.g. `gamemoderun`, a
    // frame-generation layer's own wrapper script) — runs the *entire*
    // above through it instead, exactly like a Steam Launch Options
    // wrapper + `%command%` already does for a game added to Steam (see
    // README's "Injecting env vars or a wrapper tool via a Steam
    // shortcut"), just native to iprolaunch so it applies the same way
    // regardless of how the game's actually launched.
    let mut command = match effective.launch_wrapper.as_deref().map(wrapper_argv) {
        Some(wrapper) if !wrapper.is_empty() => {
            let mut c = Command::new(&wrapper[0]);
            c.args(&wrapper[1..]);
            c.arg(inner_program);
            c
        }
        _ => Command::new(inner_program),
    };
    command.args(&inner_args);
    // Without this, the spawned process inherits *iprolaunch's own* cwd
    // (wherever it happened to be run from) instead of the game's install
    // folder — exactly what double-clicking the exe in Windows Explorer
    // (or Lutris, which always sets this) gives it instead. A game whose
    // asset loading assumes cwd == its own folder (a real, confirmed case:
    // a VN that otherwise rendered no background art / threw a load error
    // under iprolaunch, working fine under Lutris) breaks without this.
    if let Some(dir) = target.parent() {
        command.current_dir(dir);
    }
    command.env("WINEPREFIX", &prefix_path);
    command.env("IPROLAUNCH_LAUNCH_ID", &launch_id);
    // `umu-run` doesn't touch `WINEARCH` itself (confirmed against its own
    // source — not part of its whitelisted env dict, and it never clears
    // the inherited environment before spawning Proton), so this reaches
    // the actual wine process untouched, same as every other env var here.
    // Only meaningful at prefix *creation*; changing it against an
    // already-existing prefix does nothing (or errors, depending on the
    // Proton build) — the TUI warns about this when the value changes.
    command.env("WINEARCH", effective.winearch.as_str());
    if !effective.proton.is_empty() && effective.proton != "system" {
        command.env("PROTONPATH", &effective.proton);
    }
    if let Some(id) = &gameid {
        command.env("GAMEID", id);
    }
    if let Some(overrides) = winedlloverrides_value(&effective.winedlloverride) {
        command.env("WINEDLLOVERRIDES", overrides);
    }
    let mut env = merge_env(&effective.env, &opts.env);
    // `umu-run` defaults to Proton's `waitforexitandrun` verb, which waits
    // for the prefix's *existing* wineserver to fully exit before starting —
    // confirmed straight from Proton's own script: `wineserver -w` then run.
    // That's fine when nothing else is using this prefix, but in `single`
    // mode running a second, different game concurrently means that
    // wineserver never exits, so the new launch would just hang forever
    // waiting on it (a real reported bug). If something's already running
    // against this exact prefix, skip the wait (`PROTON_VERB=run`) instead —
    // wineserver already supports multiple simultaneous client processes
    // (that's its whole job), confirmed for real: 3 concurrent launches
    // sharing one wineserver, killing one leaving the other two untouched.
    // Never overrides an explicit `PROTON_VERB` the user already set.
    if !env.contains_key("PROTON_VERB") && running::is_prefix_active(&prefix_path.to_string_lossy())
    {
        env.insert("PROTON_VERB".to_string(), "run".to_string());
    }
    for (k, v) in env {
        command.env(k, v);
    }

    let log_session = if effective.record != RecordMode::Off {
        let log_dir = cfg.log_dir()?;
        let session = LogSession::start(&effective, &log_dir, &slug)?;
        eprintln!("iprolaunch: logging to {}", session.path().display());
        let (out, err) = session.stdio_pair()?;
        command.stdout(out);
        command.stderr(err);
        Some(session)
    } else {
        None
    };

    let mut child = command.spawn().with_context(|| {
        if let Some(wrapper) = &effective.launch_wrapper {
            format!("failed to spawn launch_wrapper {wrapper:?} (is it installed and on $PATH/executable?)")
        } else if wrap_with_gamescope {
            "failed to spawn gamescope (is it installed and on $PATH? required for -f/-w/-b)"
                .to_string()
        } else {
            "failed to spawn umu-run (is it installed and on $PATH?)".to_string()
        }
    })?;
    let state_path = running::record(child.id(), &profile.name, &target, &prefix_path, &launch_id)?;

    let status = child.wait().context("waiting for umu-run")?;
    running::clear(&state_path);

    profile.mark_launched_now();
    profile.save(&slug)?;

    if let Some(session) = &log_session
        && let Ok(text) = fs::read_to_string(session.path())
        && let Some(summary) = summarize_protonfixes(&text)
    {
        eprintln!("iprolaunch: {summary}");
    }

    let log_path = match log_session {
        Some(session) => session.finish(status.success())?,
        None => None,
    };

    if !status.success() {
        let should_auto_open = effective.auto_open
            && (effective.auto_open_scope == AutoOpenScope::Always || opts.from_tui);
        if should_auto_open && let Some(path) = &log_path {
            let pager = std::env::var("PAGER").unwrap_or_else(|_| "less".into());
            terminal::spawn_in_new_terminal(&pager, path);
        }
        bail!("umu-run exited with {status}");
    }
    Ok(())
}

/// The first (slug, name) pair built from `base` (`base`/`base#1`, then
/// `base-2`/`base#2`, ...) that collides with neither an existing slug nor
/// an existing name in `existing` — checked *independently* of each other,
/// not just the slug with the name assumed to follow along for free (see
/// `ensure_profile`'s own doc comment for the real bug that was). Pure, so
/// it's testable against a hand-built `existing` list without touching
/// real disk, unlike `ensure_profile` itself (`Profile::load_all`/`save`).
fn next_available_slug_and_name(base: &str, existing: &[(String, Profile)]) -> (String, String) {
    let mut n = 1;
    let mut slug = base.to_string();
    let mut name = format!("{base}#{n}");
    while existing.iter().any(|(s, _)| *s == slug) || existing.iter().any(|(_, p)| p.name == name) {
        n += 1;
        slug = format!("{base}-{n}");
        name = format!("{base}#{n}");
    }
    (slug, name)
}

/// Finds the profile whose `target_path` matches, or creates one — disambiguating
/// both the storage slug and the display `name` when another profile already
/// claims the same exe stem (e.g. two different games each shipping a `game.exe`).
/// `pub` (rather than crate-private) so `main.rs`'s `add` subcommand can
/// register a profile without launching anything — `run` calls this too,
/// which is what makes a game show up in the library after a single `run`
/// with no separate add step required there.
///
/// The candidate slug and name are checked for collisions *independently*
/// (not just the slug, with the name assumed to follow along for free) —
/// a real reported bug: the profile editor's slug rename only ever touches
/// the slug, deliberately leaving `name` as-is (see `profile_editor::
/// rename_slug`'s own doc comment), so a later profile auto-created for a
/// same-stemmed exe could land on a slug that's genuinely free while still
/// landing on a `name` some *other*, differently-slugged profile already
/// has — two entries both displaying as e.g. "game#1" in the Library, with
/// no way to tell them apart at a glance.
pub fn ensure_profile(target: &Path) -> Result<(String, Profile)> {
    let target_str = target.to_string_lossy().into_owned();
    let existing = Profile::load_all()?;

    if let Some((slug, profile)) = existing.iter().find(|(_, p)| p.target_path == target_str) {
        return Ok((slug.clone(), profile.clone()));
    }

    clear_execute_bit(target);

    let base = prefix::slug_from_exe(target);
    let (slug, name) = next_available_slug_and_name(&base, &existing);

    let profile = Profile {
        name,
        target_path: target_str,
        title: None,
        last_launched: None,
        defaults: Default::default(),
        logging: Default::default(),
        env: Default::default(),
        winedlloverride: Default::default(),
        args: Default::default(),
    };
    profile.save(&slug)?;
    Ok((slug, profile))
}

/// Windows exes never need the Linux execute bit — `umu-run`/wine read the
/// path as an argument, never `execve` it directly — but on KDE, the
/// `kiorc` setting `[Executable scripts] behaviourOnLaunch=execute` makes
/// *any* file with `+x` set get executed directly on open/double-click
/// (routed by the kernel's own `binfmt_misc`, e.g. a `DOSWin` MZ-header
/// registration straight to `/usr/bin/wine`), entirely bypassing xdg-mime —
/// confirmed for real: `xdg-mime query default` still correctly named
/// `iprolaunch.desktop`, yet double-clicking a `+x` `.exe` still launched
/// Wine directly. KIO has no per-mimetype override for that setting (it's a
/// single global toggle — checked its own source), so the file's own
/// permission bit is the only lever that's actually ours to pull. Clearing
/// it here, once, when a profile is first created, fixes that without
/// touching the user's global KDE setting. Best-effort: a failure (e.g.
/// read-only media) shouldn't block adding the profile.
fn clear_execute_bit(target: &Path) {
    let Ok(meta) = fs::metadata(target) else {
        return;
    };
    let mut perms = meta.permissions();
    let mode = perms.mode();
    let cleared = mode & !0o111;
    if cleared != mode {
        perms.set_mode(cleared);
        let _ = fs::set_permissions(target, perms);
    }
}

/// Reports whether `umu-run`'s own automatic ProtonFixes run found and
/// applied anything game-specific, by scanning its captured output. `umu-run`
/// always runs ProtonFixes itself (confirmed in NOTES.md) — this doesn't
/// reimplement or trigger it, just surfaces the result, which otherwise only
/// shows up buried in the log file.
///
/// Per protonfixes' own source (`fix.py`'s `_run_fix`), the two messages that
/// matter are `Using {stage} stage {scope} protonfix for {name} ({id})`
/// (applied) and `No {stage} stage {scope} protonfix found for {name} ({id})`
/// (not applied) — deliberately distinct from its `... {scope} defaults for
/// ...` messages, which are generic per-store setup that runs on every
/// launch regardless of game and would be noise to report as "a fix was
/// applied". Returns `None` if ProtonFixes didn't run at all (e.g.
/// `PROTONFIXES_DISABLE=1`), so this stays silent rather than printing
/// anything about protonfixes when there's genuinely nothing to say.
fn summarize_protonfixes(log_text: &str) -> Option<String> {
    let mut ran_at_all = false;
    let mut applied = Vec::new();

    for line in log_text.lines() {
        let Some(msg) = line.split_once("ProtonFixes[").map(|(_, r)| r) else {
            continue;
        };
        ran_at_all = true;
        if let Some(idx) = msg.find("Using ")
            && msg[idx..].contains(" protonfix for ")
        {
            applied.push(msg[idx..].trim().to_string());
        }
    }

    if !ran_at_all {
        return None;
    }
    Some(if applied.is_empty() {
        "no protonfix found for this game".to_string()
    } else {
        format!("protonfix applied — {}", applied.join("; "))
    })
}

/// Where `apply_windows_version` remembers which version it last actually
/// applied to this exact prefix — a plain dotfile inside the prefix
/// directory itself (not iprolaunch's own `~/.config/iprolaunch/` tree),
/// since this is a property of *that prefix's* registry state, not of
/// iprolaunch's own settings, and needs to travel with the prefix if it's
/// ever moved/renamed rather than living somewhere keyed by slug.
fn windows_version_marker_path(prefix_path: &Path) -> PathBuf {
    prefix_path.join(".iprolaunch-windows-version")
}

/// The version last recorded as applied to this prefix, or `None` — covers
/// "never applied by iprolaunch yet" (a fresh prefix, or one from before
/// this marker existed) the same as any read failure, so a missing/corrupt
/// marker just means "apply once and start tracking," never an error.
fn cached_windows_version(prefix_path: &Path) -> Option<String> {
    fs::read_to_string(windows_version_marker_path(prefix_path))
        .ok()
        .map(|s| s.trim().to_string())
}

/// Sets the prefix's reported Windows version via `winetricks -q <version>`,
/// but only when `version` actually differs from what's cached as already
/// applied to this exact prefix (see `cached_windows_version`) — a real
/// reported problem: winetricks' own "already applied" bookkeeping wasn't
/// enough to stop this from re-running (and killing/relaunching wineserver
/// below) on *every single launch*, version unchanged or not, which is real
/// overhead/disruption on every launch rather than just the first one for a
/// given version. Deliberately independent of *why* the effective value is
/// what it is (the global default changed, a per-slug profile's own
/// override changed, or this is just a brand new prefix) — any of those
/// just means "does the marker already say this string?", same check
/// either way, so a change in *either* direction (global 11 → profile
/// override 10, or a later global 11 → 10 with no override) is picked up
/// correctly, while a launch where nothing changed short-circuits before
/// spawning anything at all.
///
/// Uses the system winetricks/wine rather than the Proton build's own bundled
/// wine (there's no clean way to know which build umu-run resolved to ahead
/// of its own run) — fine for these verbs specifically, since `win7`/`win10`/
/// etc. only rewrite a few registry keys rather than run real Windows code.
/// A real consequence of that: since this is also the *first* thing to ever
/// touch a brand new prefix (see `winearch` below), it's the *system*
/// wine's own support for `WINEARCH=win32` that actually gates whether a
/// `win32` prefix can be created at all — not whichever Proton build the
/// profile/global default has selected for the real launch.
///
/// `winearch`: threaded through as `WINEARCH` alongside `WINEPREFIX` on the
/// same `winetricks` call — a real reported bug otherwise: without it, a
/// freshly deleted/nonexistent prefix always came up `win64` regardless of
/// the configured `winearch`, since this function runs (and creates the
/// prefix) before the actual game-launch command below ever gets a chance
/// to apply its own `WINEARCH`.
///
/// Kills any wineserver already bound to this prefix first, but only when
/// nothing of ours is actually running against it — a previous launch's
/// Proton-bundled wineserver can be left stale/attached, and if its version
/// doesn't match the system wine's, winetricks fails outright (observed:
/// `wine client error:0: version mismatch 856/961` — confirmed by testing,
/// see project NOTES.md). Checking `running::is_prefix_active` first matters:
/// unconditionally killing it would just as easily tear down a wineserver
/// that's genuinely still in use (e.g. the same exe launched twice, or two
/// exes sharing a prefix in single mode).
fn apply_windows_version(prefix_path: &Path, version: &str, winearch: &str) -> Result<()> {
    if cached_windows_version(prefix_path).as_deref() == Some(version) {
        return Ok(());
    }

    if !running::is_prefix_active(&prefix_path.to_string_lossy()) {
        Command::new("wineserver")
            .arg("-k")
            .env("WINEPREFIX", prefix_path)
            .status()
            .ok();
    }

    // A real reported bug: this is the *first* thing to ever touch a freshly
    // deleted/nonexistent prefix (before the actual game launch's own
    // umu-run invocation), so it — not that later invocation — is what
    // actually decides a new prefix's architecture. Without `WINEARCH` set
    // here too, a fresh prefix always came up `win64` regardless of the
    // configured `winearch`, since this ran and created it first.
    let status = Command::new("winetricks")
        .arg("-q")
        .arg(version)
        .env("WINEPREFIX", prefix_path)
        .env("WINEARCH", winearch)
        .status()
        .context("failed to run winetricks (is it installed and on $PATH?)")?;
    if !status.success() {
        bail!("winetricks {version} exited with {status}");
    }

    // Best-effort: a failed write here just means the next launch reapplies
    // a no-op (winetricks itself still skips real work once it's already
    // set) — never worth failing the whole launch over.
    let _ = fs::write(windows_version_marker_path(prefix_path), version);
    Ok(())
}

/// Forwards Ctrl+C (and a plain `kill`/SIGTERM on `iprolaunch` itself) into
/// the sandboxed game tree via `running::terminate`'s launch-id-matching
/// sweep. Needed because the tree bwrap creates detaches into its own
/// session (see NOTES.md, 2026-09-06) — the terminal's SIGINT reaches
/// `iprolaunch` and the directly-spawned `umu-run` (both still share the
/// foreground process group), but nothing forwards it deeper on its own, so
/// without this, Ctrl+C would kill only `iprolaunch` and leave the game
/// running orphaned. Best-effort: if installing the handler fails, launch
/// proceeds anyway with just a warning — Ctrl+C during the run degrades back
/// to today's behavior rather than blocking the whole launch over it.
fn install_signal_forwarding(launch_id: &str) {
    use signal_hook::consts::{SIGINT, SIGTERM};
    use signal_hook::iterator::Signals;

    match Signals::new([SIGINT, SIGTERM]) {
        Ok(mut signals) => {
            let launch_id = launch_id.to_string();
            std::thread::spawn(move || {
                for _ in signals.forever() {
                    let _ = running::terminate(&launch_id);
                }
            });
        }
        Err(err) => {
            eprintln!("iprolaunch: warning: failed to install Ctrl+C handler: {err}");
        }
    }
}

/// Per-key merge: `cli` overrides `base` key-for-key, leaving every other key
/// from `base` (global + profile, already merged into `effective.env`)
/// untouched — never a wholesale replace of the whole map.
fn merge_env(
    base: &BTreeMap<String, String>,
    cli: &[(String, String)],
) -> BTreeMap<String, String> {
    let mut merged = base.clone();
    for (k, v) in cli {
        merged.insert(k.clone(), v.clone());
    }
    merged
}

/// Joins a per-DLL override table into one `WINEDLLOVERRIDES` value:
/// `dll1=modes1;dll2=modes2`. `BTreeMap` iterates in key order, so the
/// result is deterministic. `None` when empty — nothing to set.
fn winedlloverrides_value(overrides: &BTreeMap<String, String>) -> Option<String> {
    if overrides.is_empty() {
        return None;
    }
    Some(
        overrides
            .iter()
            .map(|(dll, modes)| format!("{dll}={modes}"))
            .collect::<Vec<_>>()
            .join(";"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile_named(slug: &str, name: &str) -> (String, Profile) {
        (
            slug.to_string(),
            Profile {
                name: name.to_string(),
                target_path: format!("/tmp/{slug}.exe"),
                title: None,
                last_launched: None,
                defaults: Default::default(),
                logging: Default::default(),
                env: Default::default(),
                winedlloverride: Default::default(),
                args: Default::default(),
            },
        )
    }

    #[test]
    fn next_available_slug_and_name_checks_the_name_even_when_the_slug_is_free() {
        // Real reported bug: after a slug-only rename (which deliberately
        // leaves `name` untouched — see `profile_editor::rename_slug`),
        // "game" the *slug* is free again, but "game#1" the *name* is
        // still claimed by the renamed profile under its new slug. A new
        // same-stemmed exe used to land on ("game", "game#1") anyway —
        // two entries both showing as "game#1" in the Library with no way
        // to tell them apart — since the old disambiguation loop only
        // checked the slug, assuming the name always followed along.
        // Slug and name are still bumped *together* (consistent with the
        // app's existing convention that a profile's folder and display
        // name always share the same "#N" — see e.g. two different
        // `game.exe`-stemmed games landing on `game`/`game#1` and
        // `game-2`/`game#2`, never a mismatched `game`/`game#2`): the fix
        // is that the *name* collision is what triggers the bump at all
        // here, even though the slug alone was already free.
        let existing = vec![profile_named("monster-g", "game#1")];
        assert_eq!(
            next_available_slug_and_name("game", &existing),
            ("game-2".to_string(), "game#2".to_string())
        );
    }

    #[test]
    fn next_available_slug_and_name_is_just_base_hash_1_with_nothing_existing() {
        assert_eq!(
            next_available_slug_and_name("game", &[]),
            ("game".to_string(), "game#1".to_string())
        );
    }

    #[test]
    fn gamescope_mode_default_is_inactive_with_no_args() {
        let mode = GamescopeMode::default();
        assert!(!mode.is_active());
        assert!(mode.args().is_empty());
    }

    #[test]
    fn fullscreen_maps_to_gamescopes_own_dash_f() {
        let mode = GamescopeMode {
            fullscreen: true,
            maximize: false,
            borderless: false,
        };
        assert!(mode.is_active());
        assert_eq!(mode.args(), vec!["-f"]);
    }

    #[test]
    fn maximize_maps_to_force_windows_fullscreen() {
        let mode = GamescopeMode {
            fullscreen: false,
            maximize: true,
            borderless: false,
        };
        assert!(mode.is_active());
        assert_eq!(mode.args(), vec!["--force-windows-fullscreen"]);
    }

    #[test]
    fn borderless_maps_to_gamescopes_own_dash_b() {
        let mode = GamescopeMode {
            fullscreen: false,
            maximize: false,
            borderless: true,
        };
        assert!(mode.is_active());
        assert_eq!(mode.args(), vec!["-b"]);
    }

    #[test]
    fn all_three_stack_fullscreen_then_maximize_then_borderless() {
        let mode = GamescopeMode {
            fullscreen: true,
            maximize: true,
            borderless: true,
        };
        assert_eq!(mode.args(), vec!["-f", "--force-windows-fullscreen", "-b"]);
    }

    #[test]
    fn should_wrap_with_gamescope_only_when_requested_and_not_already_nested() {
        assert!(should_wrap_with_gamescope(true, false));
        assert!(!should_wrap_with_gamescope(true, true));
        assert!(!should_wrap_with_gamescope(false, false));
        assert!(!should_wrap_with_gamescope(false, true));
    }

    #[test]
    fn iprolaunch_cli_flags_for_maps_gamescope_setting_and_borderless() {
        use crate::config::{Config, GamescopeSetting};
        let mut cfg = Config::default();
        assert!(iprolaunch_cli_flags_for(&cfg.effective(None)).is_empty());

        cfg.defaults.gamescope = GamescopeSetting::Fullscreen;
        assert_eq!(iprolaunch_cli_flags_for(&cfg.effective(None)), vec!["-f"]);

        cfg.defaults.gamescope = GamescopeSetting::Maximize;
        cfg.defaults.gamescope_settings.borderless = Some(true);
        assert_eq!(
            iprolaunch_cli_flags_for(&cfg.effective(None)),
            vec!["-w", "-b"]
        );
    }

    #[test]
    fn gamescope_setting_converts_to_the_matching_mode() {
        use crate::config::GamescopeSetting;
        assert!(!GamescopeMode::from(GamescopeSetting::None).is_active());
        assert_eq!(
            GamescopeMode::from(GamescopeSetting::Fullscreen).args(),
            vec!["-f"]
        );
        assert_eq!(
            GamescopeMode::from(GamescopeSetting::Maximize).args(),
            vec!["--force-windows-fullscreen"]
        );
    }

    #[test]
    fn gamescope_settings_args_is_empty_when_nothing_set() {
        assert!(gamescope_settings_args(&crate::config::GamescopeSettings::default()).is_empty());
    }

    #[test]
    fn gamescope_settings_args_maps_every_field_to_its_real_flag() {
        use crate::config::{GamescopeFilter, GamescopeScaler, GamescopeSettings};
        let settings = GamescopeSettings {
            output_width: Some(1920),
            output_height: Some(1080),
            refresh: Some(60),
            nested_width: Some(1280),
            nested_height: Some(800),
            filter: Some(GamescopeFilter::Fsr),
            scaler: Some(GamescopeScaler::Fit),
            borderless: None,
            grab_cursor: None,
            adaptive_sync: None,
        };
        assert_eq!(
            gamescope_settings_args(&settings),
            vec![
                "-W", "1920", "-H", "1080", "-r", "60", "-w", "1280", "-h", "800", "-F", "fsr",
                "-S", "fit",
            ]
        );
    }

    #[test]
    fn gamescope_settings_args_maps_grab_cursor_but_not_borderless() {
        use crate::config::GamescopeSettings;
        // `borderless` is deliberately NOT in `gamescope_settings_args`'
        // output — it's merged into `GamescopeMode` instead (alongside the
        // CLI `-b` flag), never appended here. `grab_cursor` has no CLI
        // counterpart, so it's config-only and belongs here.
        let settings = GamescopeSettings {
            borderless: Some(true),
            grab_cursor: Some(true),
            ..Default::default()
        };
        assert_eq!(
            gamescope_settings_args(&settings),
            vec!["--force-grab-cursor"]
        );

        let settings = GamescopeSettings {
            grab_cursor: Some(false),
            ..Default::default()
        };
        assert!(gamescope_settings_args(&settings).is_empty());
    }

    #[test]
    fn gamescope_settings_args_maps_scaler_and_adaptive_sync() {
        use crate::config::{GamescopeScaler, GamescopeSettings};
        let settings = GamescopeSettings {
            scaler: Some(GamescopeScaler::Auto),
            adaptive_sync: Some(true),
            ..Default::default()
        };
        assert_eq!(
            gamescope_settings_args(&settings),
            vec!["-S", "auto", "--adaptive-sync"]
        );

        let settings = GamescopeSettings {
            adaptive_sync: Some(false),
            ..Default::default()
        };
        assert!(gamescope_settings_args(&settings).is_empty());
    }

    #[test]
    fn gamescope_filter_value_matches_real_gamescope_option_names() {
        use crate::config::GamescopeFilter::*;
        assert_eq!(gamescope_filter_value(Linear), "linear");
        assert_eq!(gamescope_filter_value(Nearest), "nearest");
        assert_eq!(gamescope_filter_value(Fsr), "fsr");
        assert_eq!(gamescope_filter_value(Nis), "nis");
        assert_eq!(gamescope_filter_value(Pixel), "pixel");
    }

    #[test]
    fn wrapper_argv_splits_on_whitespace() {
        assert_eq!(
            wrapper_argv("gamemoderun --something"),
            vec!["gamemoderun", "--something"]
        );
        assert_eq!(wrapper_argv("mangohud"), vec!["mangohud"]);
    }

    #[test]
    fn wrapper_argv_expands_home_in_just_the_program_not_its_args() {
        let home = directories::UserDirs::new()
            .unwrap()
            .home_dir()
            .display()
            .to_string();
        assert_eq!(
            wrapper_argv("~/lsfg --keep-tilde ~/not-expanded"),
            vec![
                format!("{home}/lsfg"),
                "--keep-tilde".to_string(),
                "~/not-expanded".to_string()
            ]
        );
    }

    #[test]
    fn gamescope_scaler_value_matches_real_gamescope_option_names() {
        use crate::config::GamescopeScaler::*;
        assert_eq!(gamescope_scaler_value(Auto), "auto");
        assert_eq!(gamescope_scaler_value(Integer), "integer");
        assert_eq!(gamescope_scaler_value(Fit), "fit");
        assert_eq!(gamescope_scaler_value(Fill), "fill");
        assert_eq!(gamescope_scaler_value(Stretch), "stretch");
    }

    #[test]
    fn clear_execute_bit_strips_only_the_execute_bits() {
        let path =
            std::env::temp_dir().join(format!("iprolaunch-launch-test-{}.exe", std::process::id()));
        fs::write(&path, b"MZ").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o777)).unwrap();

        clear_execute_bit(&path);

        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o666,
            "execute bits should be cleared, read/write kept"
        );

        fs::remove_file(&path).ok();
    }

    #[test]
    fn clear_execute_bit_is_a_no_op_for_a_missing_file() {
        // Best-effort: must not panic when the target doesn't exist.
        clear_execute_bit(Path::new("/nonexistent/iprolaunch-test.exe"));
    }

    fn windows_version_test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "iprolaunch-launch-test-{name}-{}",
            std::process::id()
        ))
    }

    #[test]
    fn cached_windows_version_is_none_for_a_fresh_prefix() {
        let dir = windows_version_test_dir("wv-missing");
        assert_eq!(cached_windows_version(&dir), None);
    }

    #[test]
    fn cached_windows_version_reads_and_trims_what_was_written() {
        let dir = windows_version_test_dir("wv-roundtrip");
        fs::create_dir_all(&dir).unwrap();
        fs::write(windows_version_marker_path(&dir), "win10\n").unwrap();

        assert_eq!(cached_windows_version(&dir), Some("win10".to_string()));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn apply_windows_version_short_circuits_when_the_marker_already_matches() {
        // Real reported bug: unconditionally re-running `winetricks` (and
        // killing/relaunching wineserver) on every launch even when the
        // version hadn't changed. This is safe to test for real (no
        // wineserver/winetricks needed) specifically *because* a matching
        // marker means the function must return before spawning anything.
        let dir = windows_version_test_dir("wv-shortcircuit");
        fs::create_dir_all(&dir).unwrap();
        fs::write(windows_version_marker_path(&dir), "win11").unwrap();

        let result = apply_windows_version(&dir, "win11", "win64");
        assert!(result.is_ok(), "should short-circuit, not fail: {result:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn winedlloverrides_joins_deterministically_by_key_order() {
        let mut overrides = BTreeMap::new();
        overrides.insert("LunaHook64".to_string(), "n,b".to_string());
        overrides.insert("winhttp".to_string(), "n,b".to_string());

        assert_eq!(
            winedlloverrides_value(&overrides),
            Some("LunaHook64=n,b;winhttp=n,b".to_string())
        );
    }

    #[test]
    fn winedlloverrides_none_when_empty() {
        assert_eq!(winedlloverrides_value(&BTreeMap::new()), None);
    }

    #[test]
    fn cli_env_overrides_only_the_keys_it_sets() {
        let mut profile_and_global_env = BTreeMap::new();
        profile_and_global_env.insert("PROTON_LOG".to_string(), "1".to_string());
        profile_and_global_env.insert("MANGOHUD".to_string(), "1".to_string());

        let cli_env = vec![("PROTON_LOG".to_string(), "0".to_string())];

        let merged = merge_env(&profile_and_global_env, &cli_env);

        assert_eq!(merged.get("PROTON_LOG").map(String::as_str), Some("0"));
        assert_eq!(merged.get("MANGOHUD").map(String::as_str), Some("1"));
    }

    #[test]
    fn protonfix_not_found_reported_from_real_observed_log_excerpt() {
        // Verbatim excerpt from the 2026-09-05 self-test (GAMEID unset).
        let log = "\
ProtonFixes[1298151] INFO: Running protonfixes on \"UMU-Proton-10.0-4\", build at 2026-03-30 07:33:47+00:00.
ProtonFixes[1298151] INFO: Running checks
ProtonFixes[1298151] INFO: All checks successful
ProtonFixes[1298151] WARN: Game title not found in CSV
ProtonFixes[1298151] INFO: Non-steam game UNKNOWN (umu-default)
ProtonFixes[1298151] INFO: No store specified, using UMU database
ProtonFixes[1298151] INFO: Using early stage global defaults for UNKNOWN (umu-default)
ProtonFixes[1298151] INFO: No early stage global protonfix found for UNKNOWN (umu-default)
ProtonFixes[1298151] INFO: Using main stage global defaults for UNKNOWN (umu-default)
ProtonFixes[1298151] INFO: No main stage global protonfix found for UNKNOWN (umu-default)
";
        assert_eq!(
            summarize_protonfixes(log),
            Some("no protonfix found for this game".to_string())
        );
    }

    #[test]
    fn protonfix_applied_is_distinguished_from_defaults() {
        // Matches protonfixes' own fix.py message format for a real fix,
        // not the generic "... defaults for ..." that runs on every launch.
        let log = "\
ProtonFixes[1298151] INFO: Using early stage global defaults for Some Game (12345)
ProtonFixes[1298151] INFO: Using early stage global protonfix for Some Game (12345)
";
        let summary = summarize_protonfixes(log).unwrap();
        assert!(summary.starts_with("protonfix applied"));
        assert!(summary.contains("Some Game (12345)"));
        assert!(!summary.contains("defaults"));
    }

    #[test]
    fn no_summary_when_protonfixes_never_ran() {
        assert_eq!(
            summarize_protonfixes("some unrelated umu-run output\n"),
            None
        );
    }
}
