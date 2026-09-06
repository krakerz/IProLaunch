use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::config::{Config, Profile, RecordMode};
use crate::gamedb;
use crate::logging::LogSession;
use crate::prefix;
use crate::running;

#[derive(Default)]
pub struct RunOptions {
    pub proton: Option<String>,
    pub prefix: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub args: Vec<String>,
    pub gamescope: GamescopeMode,
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
/// differ), `--force-grab-cursor` (relative mouse mode, config-only — see
/// `GamescopeSettings::grab_cursor`'s doc comment for why it's not a CLI
/// flag). `borderless` is handled separately, merged into `GamescopeMode`
/// alongside the CLI `-b` flag instead (see `run`) — everything else here
/// is only meaningful, and only ever appended, when `gamescope` is
/// actually wrapping the launch at all (`GamescopeMode::is_active()`).
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
    if settings.grab_cursor == Some(true) {
        args.push("--force-grab-cursor".to_string());
    }
    args
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

    if let Some(version) = &effective.windows_version
        && let Err(err) = apply_windows_version(&prefix_path, version)
    {
        // Best-effort: a failed registry tweak shouldn't block the game itself.
        eprintln!("iprolaunch: warning: failed to apply windows-version={version}: {err:#}");
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

    // Per `man umu`: WINEPREFIX/PROTONPATH/GAMEID are all optional env vars;
    // GAMEID defaults to "umu-default" when unset.
    let mut command = if gamescope.is_active() {
        let mut c = Command::new("gamescope");
        c.args(gamescope.args());
        c.args(gamescope_settings_args(&effective.gamescope_settings));
        c.arg("--").arg("umu-run");
        c
    } else {
        Command::new("umu-run")
    };
    command.arg(&target);
    command.args(&profile.args); // profile's own defaults first, e.g. `--dx11`
    command.args(&opts.args); // then CLI/quick-launch args, supplementing rather than replacing
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
        if gamescope.is_active() {
            "failed to spawn gamescope (is it installed and on $PATH? required for -f/-m)"
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
        if effective.auto_open
            && let Some(path) = &log_path
        {
            open_in_pager(path);
        }
        bail!("umu-run exited with {status}");
    }
    Ok(())
}

/// Finds the profile whose `target_path` matches, or creates one — disambiguating
/// both the storage slug and the display `name` when another profile already
/// claims the same exe stem (e.g. two different games each shipping a `game.exe`).
/// `pub` (rather than crate-private) so `main.rs`'s `add` subcommand can
/// register a profile without launching anything — `run` calls this too,
/// which is what makes a game show up in the library after a single `run`
/// with no separate add step required there.
pub fn ensure_profile(target: &Path) -> Result<(String, Profile)> {
    let target_str = target.to_string_lossy().into_owned();
    let existing = Profile::load_all()?;

    if let Some((slug, profile)) = existing.iter().find(|(_, p)| p.target_path == target_str) {
        return Ok((slug.clone(), profile.clone()));
    }

    clear_execute_bit(target);

    let base = prefix::slug_from_exe(target);
    let mut slug = base.clone();
    let mut n = 1;
    while existing.iter().any(|(s, _)| *s == slug) {
        n += 1;
        slug = format!("{base}-{n}");
    }

    let profile = Profile {
        name: format!("{base}#{n}"),
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

/// Sets the prefix's reported Windows version via `winetricks -q <version>`.
/// Uses the system winetricks/wine rather than the Proton build's own bundled
/// wine (there's no clean way to know which build umu-run resolved to ahead
/// of its own run) — fine for these verbs specifically, since `win7`/`win10`/
/// etc. only rewrite a few registry keys rather than run real Windows code,
/// and winetricks already tracks applied verbs in the prefix and skips
/// reapplying, so calling this on every launch is cheap once it's set.
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
fn apply_windows_version(prefix_path: &Path, version: &str) -> Result<()> {
    if !running::is_prefix_active(&prefix_path.to_string_lossy()) {
        Command::new("wineserver")
            .arg("-k")
            .env("WINEPREFIX", prefix_path)
            .status()
            .ok();
    }

    let status = Command::new("winetricks")
        .arg("-q")
        .arg(version)
        .env("WINEPREFIX", prefix_path)
        .status()
        .context("failed to run winetricks (is it installed and on $PATH?)")?;
    if !status.success() {
        bail!("winetricks {version} exited with {status}");
    }
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

/// No-op when there's no controlling terminal (e.g. launched detached from a
/// desktop file's double-click, which is the default — see `integrate`):
/// spawning a pager with nothing to attach to would just fail or hang. The
/// log file is still on disk either way, inspectable later via the TUI/CLI.
fn open_in_pager(path: &Path) {
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() {
        return;
    }
    let pager = std::env::var("PAGER").unwrap_or_else(|_| "less".into());
    let _ = Command::new(pager).arg(path).status();
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
        use crate::config::{GamescopeFilter, GamescopeSettings};
        let settings = GamescopeSettings {
            output_width: Some(1920),
            output_height: Some(1080),
            refresh: Some(60),
            nested_width: Some(1280),
            nested_height: Some(800),
            filter: Some(GamescopeFilter::Fsr),
            borderless: None,
            grab_cursor: None,
        };
        assert_eq!(
            gamescope_settings_args(&settings),
            vec![
                "-W", "1920", "-H", "1080", "-r", "60", "-w", "1280", "-h", "800", "-F", "fsr",
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
    fn gamescope_filter_value_matches_real_gamescope_option_names() {
        use crate::config::GamescopeFilter::*;
        assert_eq!(gamescope_filter_value(Linear), "linear");
        assert_eq!(gamescope_filter_value(Nearest), "nearest");
        assert_eq!(gamescope_filter_value(Fsr), "fsr");
        assert_eq!(gamescope_filter_value(Nis), "nis");
        assert_eq!(gamescope_filter_value(Pixel), "pixel");
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
