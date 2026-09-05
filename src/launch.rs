use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::config::{Config, Profile, RecordMode, project_dirs};
use crate::logging::LogSession;
use crate::prefix;

#[derive(Serialize)]
struct RunningEntry<'a> {
    pid: u32,
    name: &'a str,
    target_path: &'a str,
    prefix_path: &'a str,
    started_at: String,
}

#[derive(Default)]
pub struct RunOptions {
    pub proton: Option<String>,
    pub prefix: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub args: Vec<String>,
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

    let prefix_path = opts
        .prefix
        .unwrap_or_else(|| prefix::resolve(&effective, &target));
    fs::create_dir_all(&prefix_path)
        .with_context(|| format!("creating prefix dir {}", prefix_path.display()))?;

    if let Some(version) = &effective.windows_version {
        // Applying this means writing the prefix's Wine registry (normally via
        // `winetricks win10`/`win7`/etc, run once against WINEPREFIX) — not
        // implemented yet, see project NOTES.md.
        eprintln!(
            "iprolaunch: windows-version={version} is configured but not yet applied to the prefix (not implemented)"
        );
    }

    // Per `man umu`: WINEPREFIX/PROTONPATH/GAMEID are all optional env vars;
    // GAMEID defaults to "umu-default" when unset.
    let mut command = Command::new("umu-run");
    command.arg(&target);
    command.args(&opts.args);
    command.env("WINEPREFIX", &prefix_path);
    if !effective.proton.is_empty() && effective.proton != "system" {
        command.env("PROTONPATH", &effective.proton);
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

    let mut child = command
        .spawn()
        .context("failed to spawn umu-run (is it installed and on $PATH?)")?;
    let state_path = record_running(child.id(), &profile.name, &target, &prefix_path)?;

    let status = child.wait().context("waiting for umu-run")?;
    let _ = fs::remove_file(&state_path);

    profile.last_launched = Some(time::OffsetDateTime::now_utc());
    profile.save(&slug)?;

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
fn ensure_profile(target: &Path) -> Result<(String, Profile)> {
    let target_str = target.to_string_lossy().into_owned();
    let existing = Profile::load_all()?;

    if let Some((slug, profile)) = existing.iter().find(|(_, p)| p.target_path == target_str) {
        return Ok((slug.clone(), profile.clone()));
    }

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
        last_launched: None,
        defaults: Default::default(),
        logging: Default::default(),
        env: Default::default(),
    };
    profile.save(&slug)?;
    Ok((slug, profile))
}

fn record_running(pid: u32, name: &str, target: &Path, prefix_path: &Path) -> Result<PathBuf> {
    let dir = project_dirs()?.config_dir().join("state").join("running");
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(format!("{pid}.json"));
    let entry = RunningEntry {
        pid,
        name,
        target_path: &target.to_string_lossy(),
        prefix_path: &prefix_path.to_string_lossy(),
        started_at: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
    };
    fs::write(&path, serde_json::to_string_pretty(&entry)?)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

fn open_in_pager(path: &Path) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
