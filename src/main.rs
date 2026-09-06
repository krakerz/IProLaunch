mod config;
mod launch;
mod logging;
mod prefix;
mod running;
mod tui;

use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::{Parser, Subcommand};

use config::{Config, Profile};
use launch::RunOptions;

#[derive(Parser)]
#[command(
    name = "iprolaunch",
    version,
    about = "Proton/umu launcher for Windows apps"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
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

/// Resolves `query` against a profile's slug or display name (case-insensitive,
/// so a fumbled Steam shortcut still lands) and launches it. Leading
/// `KEY=VALUE` tokens in `extra_args` become one-off env overrides; whatever
/// remains is forwarded to the exe.
fn quick_launch(cfg: &Config, query: &str, extra_args: Vec<String>) -> Result<()> {
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = Config::load_or_init()?;

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
            },
        ),
        Some(Command::Config {
            action: ConfigAction::Show,
        }) => {
            println!("{}", toml::to_string_pretty(&cfg)?);
            Ok(())
        }
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
        Some(Command::Quick(mut args)) => {
            if args.is_empty() {
                anyhow::bail!("usage: iprolaunch <name-or-slug> [args...]");
            }
            let query = args.remove(0);
            quick_launch(&cfg, &query, args)
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
