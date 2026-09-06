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

/// `-f`/`-m` (CLI) — wraps the launch in a nested `gamescope` session
/// instead of spawning `umu-run` directly. Useful when already running
/// inside an *embedded* gamescope session (Steam Game Mode/a Deck) — that's
/// the standard, documented trick for forcing one specific non-Steam-game
/// to behave, since a plain windowed Wine game won't otherwise switch
/// display modes or fill the screen on its own. Real gamescope flags
/// confirmed against the actually-installed `gamescope --help` (3.16.25),
/// not guessed:
/// - `-f` → gamescope's own `-f`/`--fullscreen` (nested mode option — an
///   actual display-mode-switching fullscreen for the nested window).
/// - `-m` → gamescope's `--force-windows-fullscreen` (stretches whatever
///   window the game itself opens to fill the nested surface, regardless
///   of the size it requests) — gamescope has no literal "maximized"
///   concept (it's a Wayland compositor, not an X11 window manager); this
///   is the closest real equivalent, confirmed with the user directly
///   rather than guessed.
///
/// Combining both is allowed (`-f -m` stacks their args).
#[derive(Default, Clone, Copy)]
pub struct GamescopeMode {
    pub fullscreen: bool,
    pub maximize: bool,
}

impl GamescopeMode {
    fn is_active(self) -> bool {
        self.fullscreen || self.maximize
    }

    fn args(self) -> Vec<&'static str> {
        let mut args = Vec::new();
        if self.fullscreen {
            args.push("-f");
        }
        if self.maximize {
            args.push("--force-windows-fullscreen");
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
            },
            crate::config::GamescopeSetting::Maximize => GamescopeMode {
                fullscreen: false,
                maximize: true,
            },
        }
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

    // CLI-passed `-f`/`-m` win when actually typed (consistent with every
    // other `opts` override beating the profile/global config); otherwise
    // fall back to whatever the profile/global `gamescope` setting already
    // remembers, so `iprolaunch <slug>` doesn't need `-f`/`-m` every time.
    let gamescope = if opts.gamescope.is_active() {
        opts.gamescope
    } else {
        effective.gamescope.into()
    };

    // Per `man umu`: WINEPREFIX/PROTONPATH/GAMEID are all optional env vars;
    // GAMEID defaults to "umu-default" when unset.
    let mut command = if gamescope.is_active() {
        let mut c = Command::new("gamescope");
        c.args(gamescope.args());
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
    for (k, v) in merge_env(&effective.env, &opts.env) {
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
        };
        assert!(mode.is_active());
        assert_eq!(mode.args(), vec!["-f"]);
    }

    #[test]
    fn maximize_maps_to_force_windows_fullscreen() {
        let mode = GamescopeMode {
            fullscreen: false,
            maximize: true,
        };
        assert!(mode.is_active());
        assert_eq!(mode.args(), vec!["--force-windows-fullscreen"]);
    }

    #[test]
    fn both_stack_fullscreen_first() {
        let mode = GamescopeMode {
            fullscreen: true,
            maximize: true,
        };
        assert_eq!(mode.args(), vec!["-f", "--force-windows-fullscreen"]);
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
