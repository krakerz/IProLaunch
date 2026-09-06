mod config;
mod context_menu;
mod gamedb;
mod integrate;
mod launch;
mod logging;
mod prefix;
mod proton;
mod quick_launch_cmd;
mod running;
mod tui;

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use config::{Config, Profile};
use launch::{GamescopeMode, RunOptions};

#[derive(Parser)]
#[command(
    name = "iprolaunch",
    version,
    about = "Proton/umu launcher for Windows apps"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Wrap the launch in a nested `gamescope -f` session (real fullscreen
    /// mode-switch) — useful inside an existing gamescope session (e.g.
    /// Steam Game Mode/a Deck), the standard trick for forcing one
    /// non-Steam-game to behave since a plain windowed Wine game won't
    /// otherwise switch display modes. Applies to `run` and quick-launch
    /// (`iprolaunch <slug>`) only; needs `gamescope` on `$PATH`.
    #[arg(short = 'f', long, global = true)]
    fullscreen: bool,

    /// Wrap the launch in a nested gamescope session with
    /// `--force-windows-fullscreen` (stretches the game's own window to
    /// fill it, regardless of the size it requests) — gamescope has no
    /// literal "maximized" mode; this is the closest real equivalent.
    /// Combine with -f for both at once.
    #[arg(short = 'm', long, global = true)]
    maximize: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Launch an exe directly, bypassing the TUI.
    Run {
        target: PathBuf,
        #[arg(long)]
        proton: Option<String>,
        #[arg(long)]
        prefix: Option<PathBuf>,
        #[arg(long = "env", value_parser = parse_env_kv)]
        env: Vec<(String, String)>,
        /// Extra args forwarded to the launched exe.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Register an exe as a library profile without launching it — unlike
    /// `run` (which launches, auto-creating the profile as a side effect) or
    /// the TUI's own Library `a` (add-by-path, which also launches once,
    /// the existing convention there). Useful for batch-registering
    /// profiles, or setting one up before ever actually playing it.
    Add { target: PathBuf },
    /// Inspect or manage the global config.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Inspect the game library (profiles added or run at least once).
    Library {
        #[command(subcommand)]
        action: LibraryAction,
    },
    /// Inspect or kill launches started by this wrapper.
    Running {
        #[command(subcommand)]
        action: RunningAction,
    },
    /// Inspect installed Proton builds.
    Proton {
        #[command(subcommand)]
        action: ProtonAction,
    },
    /// Register (or unregister) iprolaunch as the default handler for
    /// Windows `.exe` files, so double-clicking one in a file manager runs
    /// it through iprolaunch automatically.
    Integrate {
        #[command(subcommand)]
        action: IntegrateAction,
    },
    /// Install (or remove) a file-manager right-click "Add to IProLaunch
    /// Library" action — see `helper-script/README.md` for what this does
    /// per desktop environment (KDE/Dolphin, GNOME/Cinnamon/MATE, XFCE).
    ContextMenu {
        #[command(subcommand)]
        action: ContextMenuAction,
    },
    /// `iprolaunch <name-or-slug> [args...]` — quick-launch a library entry by
    /// its display name or slug, no `run` prefix needed. Exists so a Steam
    /// (Deck or desktop) non-Steam-game shortcut can point straight at
    /// `iprolaunch game#1` as its launch command.
    #[command(external_subcommand)]
    Quick(Vec<String>),
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print the resolved global config (creating it with defaults if absent).
    Show,
    /// Interactively pick a default Proton build from what's installed.
    Init,
}

#[derive(Subcommand)]
enum ProtonAction {
    /// List detected Proton builds.
    List,
}

#[derive(Subcommand)]
enum IntegrateAction {
    /// Install the `.desktop` file and register it as the default handler.
    Install,
    /// Remove the `.desktop` file and default-handler registration.
    Uninstall,
}

#[derive(Subcommand)]
enum ContextMenuAction {
    /// Install the right-click action — every supported DE by default
    /// (best-effort, one missing doesn't block the others), or just one.
    Install {
        de: Option<context_menu::DesktopEnv>,
    },
    /// Remove it — every supported DE by default, or just one.
    Uninstall {
        de: Option<context_menu::DesktopEnv>,
    },
}

#[derive(Subcommand)]
enum LibraryAction {
    /// List every known profile.
    List,
}

#[derive(Subcommand)]
enum RunningAction {
    /// List currently-running launches (reaps stale entries as it goes).
    List,
    /// Send SIGTERM to a running launch, by pid or by its profile name.
    Kill { target: String },
}

fn parse_env_kv(s: &str) -> Result<(String, String), String> {
    s.split_once('=')
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .ok_or_else(|| format!("expected KEY=VALUE, got `{s}`"))
}

/// Recognizes `KEY=VALUE` the way `env`/Steam launch options do: `KEY` must
/// look like an env var name, not an arbitrary flag that happens to contain
/// `=` (e.g. `-connect=1.2.3.4` is left alone).
fn parse_leading_env(s: &str) -> Option<(String, String)> {
    let (key, value) = s.split_once('=')?;
    let mut chars = key.chars();
    let starts_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    let rest_ok = chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    (starts_ok && rest_ok).then(|| (key.to_string(), value.to_string()))
}

/// Splits leading `KEY=VALUE` tokens off the front of `args` (stopping at the
/// first token that isn't one), mirroring `env KEY=VAL cmd args...` / Steam's
/// `VAR=value %command%` launch-options convention. These become temporary
/// env overrides for this one launch — highest priority, above both the
/// profile and global `[env]` (see `Config::effective` + `RunOptions.env`).
fn split_leading_env(args: Vec<String>) -> (Vec<(String, String)>, Vec<String>) {
    let mut env = Vec::new();
    let mut rest = args.into_iter();
    for arg in rest.by_ref() {
        match parse_leading_env(&arg) {
            Some(kv) => env.push(kv),
            None => return (env, std::iter::once(arg).chain(rest).collect()),
        }
    }
    (env, Vec::new())
}

/// Registers `target` as a library profile (creating it if it's not already
/// one, reusing the existing entry if it is — same lookup `run` uses) without
/// launching anything. Also copies the quick-launch command to the
/// clipboard (best-effort — a headless/no-clipboard environment, e.g. a
/// file-manager script running detached, shouldn't fail the add over it).
fn add_profile(target: &Path) -> Result<()> {
    let target = target
        .canonicalize()
        .with_context(|| format!("target executable not found: {}", target.display()))?;
    let (slug, profile) = launch::ensure_profile(&target)?;
    println!("Added \"{}\" as [{slug}].", profile.name);
    match quick_launch_cmd::copy_for_slug(&slug) {
        Ok(command) => println!("Copied to clipboard: {command}"),
        Err(err) => println!("Quick-launch with: iprolaunch {slug} ({err:#})"),
    }
    Ok(())
}

/// Resolves `query` against a profile's slug or display name (case-insensitive,
/// so a fumbled Steam shortcut still lands) and launches it. Leading
/// `KEY=VALUE` tokens in `extra_args` become one-off env overrides; whatever
/// remains is forwarded to the exe.
fn quick_launch(
    cfg: &Config,
    query: &str,
    extra_args: Vec<String>,
    gamescope: GamescopeMode,
) -> Result<()> {
    let profiles = Profile::load_all()?;
    let matches: Vec<&(String, Profile)> = profiles
        .iter()
        .filter(|(slug, p)| slug.eq_ignore_ascii_case(query) || p.name.eq_ignore_ascii_case(query))
        .collect();

    let (_, profile) = match matches.as_slice() {
        [one] => *one,
        [] => anyhow::bail!(
            "no game in the library matches `{query}` — check `iprolaunch library list`"
        ),
        _ => anyhow::bail!("`{query}` matches more than one game — use its exact slug"),
    };

    let (env, args) = split_leading_env(extra_args);

    launch::run(
        cfg,
        Path::new(&profile.target_path),
        RunOptions {
            env,
            args,
            gamescope,
            ..Default::default()
        },
    )
}

/// Resolves `target` as the displayed pid first, falling back to a
/// case-insensitive match against a running entry's profile name, then
/// terminates every process belonging to that launch (see
/// `running::terminate`'s doc comment for why that's more than one pid).
fn kill_running(target: &str) -> Result<()> {
    let entries = running::list_live()?;
    let matches: Vec<_> = if let Ok(pid) = target.parse::<u32>() {
        entries.iter().filter(|e| e.pid == pid).collect()
    } else {
        entries
            .iter()
            .filter(|e| e.name.eq_ignore_ascii_case(target))
            .collect()
    };

    match matches.as_slice() {
        [one] => running::terminate(&one.prefix_path),
        [] => anyhow::bail!("nothing running matches `{target}` — check `iprolaunch running list`"),
        _ => anyhow::bail!("`{target}` matches more than one running entry — use its pid"),
    }
}

/// Prints detected Proton builds numbered for picking, prompts on stdin, and
/// saves the choice as `defaults.proton`. Empty input leaves it unchanged.
fn config_init(mut cfg: Config) -> Result<()> {
    let builds = proton::scan()?;

    println!("Detected Proton builds:");
    println!("  0) system  (let umu-run auto-manage UMU-Proton)");
    for (i, b) in builds.iter().enumerate() {
        println!("  {})  {}  [{}]", i + 1, b.display_name, b.id);
    }
    print!(
        "Pick a default Proton build [currently: {}]: ",
        cfg.defaults.proton
    );
    std::io::stdout().flush().ok();

    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .context("reading choice")?;
    let choice = input.trim();
    if choice.is_empty() {
        println!("Unchanged.");
        return Ok(());
    }

    let index: usize = choice
        .parse()
        .context("expected a number from the list above")?;
    cfg.defaults.proton = if index == 0 {
        "system".to_string()
    } else {
        builds
            .get(index - 1)
            .with_context(|| format!("no such option: {index}"))?
            .id
            .clone()
    };
    cfg.save()?;
    println!("Saved default proton = \"{}\"", cfg.defaults.proton);
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = Config::load_or_init()?;
    let gamescope = GamescopeMode {
        fullscreen: cli.fullscreen,
        maximize: cli.maximize,
    };

    match cli.command {
        Some(Command::Run {
            target,
            proton,
            prefix,
            env,
            args,
        }) => launch::run(
            &cfg,
            &target,
            RunOptions {
                proton,
                prefix,
                env,
                args,
                gamescope,
            },
        ),
        Some(Command::Add { target }) => add_profile(&target),
        Some(Command::Config {
            action: ConfigAction::Show,
        }) => {
            println!("{}", toml::to_string_pretty(&cfg)?);
            Ok(())
        }
        Some(Command::Config {
            action: ConfigAction::Init,
        }) => config_init(cfg),
        Some(Command::Library {
            action: LibraryAction::List,
        }) => {
            let profiles = Profile::load_all()?;
            if profiles.is_empty() {
                println!("No games in the library yet — run one with `iprolaunch run <target>`.");
            }
            for (slug, profile) in profiles {
                println!("{}  [{slug}]  {}", profile.name, profile.target_path);
            }
            Ok(())
        }
        Some(Command::Running {
            action: RunningAction::List,
        }) => {
            let entries = running::list_live()?;
            if entries.is_empty() {
                println!("Nothing running.");
            }
            for e in entries {
                println!("{}  [pid {}]  {}", e.name, e.pid, e.target_path);
            }
            Ok(())
        }
        Some(Command::Running {
            action: RunningAction::Kill { target },
        }) => kill_running(&target),
        Some(Command::Proton {
            action: ProtonAction::List,
        }) => {
            let builds = proton::scan()?;
            if builds.is_empty() {
                println!("No installed Proton builds detected.");
            }
            for b in builds {
                println!("{}  [{}]", b.display_name, b.id);
            }
            Ok(())
        }
        Some(Command::Integrate {
            action: IntegrateAction::Install,
        }) => integrate::install(),
        Some(Command::Integrate {
            action: IntegrateAction::Uninstall,
        }) => integrate::uninstall(),
        Some(Command::ContextMenu {
            action: ContextMenuAction::Install { de },
        }) => context_menu::install(de),
        Some(Command::ContextMenu {
            action: ContextMenuAction::Uninstall { de },
        }) => context_menu::uninstall(de),
        Some(Command::Quick(mut args)) => {
            if args.is_empty() {
                anyhow::bail!("usage: iprolaunch <name-or-slug> [args...]");
            }
            let query = args.remove(0);
            quick_launch(&cfg, &query, args, gamescope)
        }
        None => tui::run(cfg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn splits_leading_env_and_stops_at_first_non_matching_token() {
        let (env, args) = split_leading_env(strs(&[
            "PROTON_LOG=1",
            "DXVK_HUD=0",
            "MANGOHUD=1",
            "-fullscreen",
        ]));
        assert_eq!(
            env,
            vec![
                ("PROTON_LOG".into(), "1".into()),
                ("DXVK_HUD".into(), "0".into()),
                ("MANGOHUD".into(), "1".into()),
            ]
        );
        assert_eq!(args, strs(&["-fullscreen"]));
    }

    #[test]
    fn does_not_treat_a_flag_containing_equals_as_env() {
        let (env, args) = split_leading_env(strs(&["-connect=1.2.3.4", "FOO=bar"]));
        assert!(env.is_empty());
        assert_eq!(args, strs(&["-connect=1.2.3.4", "FOO=bar"]));
    }
}
