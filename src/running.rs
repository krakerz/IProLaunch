use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::config::project_dirs;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunningEntry {
    /// The pid `Command::spawn()` returned for `umu-run` itself — kept for
    /// display/lookup only. Not what liveness/kill act on: `umu-run` forks
    /// off `bwrap`, which immediately calls `setsid()` and gets reparented
    /// away (confirmed by testing), so this exact pid can — and does — die
    /// within seconds while the actual sandboxed game keeps running for the
    /// whole session, in a completely different process group and session.
    pub pid: u32,
    pub name: String,
    pub target_path: String,
    pub prefix_path: String,
    pub started_at: String,
}

fn dir() -> Result<PathBuf> {
    Ok(project_dirs()?.config_dir().join("state").join("running"))
}

/// Writes the state file for a freshly-spawned launch. Returns its path so
/// the caller can remove it on its own clean exit.
pub fn record(pid: u32, name: &str, target_path: &Path, prefix_path: &Path) -> Result<PathBuf> {
    let dir = dir()?;
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(format!("{pid}.json"));
    let entry = RunningEntry {
        pid,
        name: name.to_string(),
        target_path: target_path.to_string_lossy().into_owned(),
        prefix_path: prefix_path.to_string_lossy().into_owned(),
        started_at: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
    };
    fs::write(&path, serde_json::to_string_pretty(&entry)?)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

pub fn clear(path: &Path) {
    let _ = fs::remove_file(path);
}

/// Reads every state file and returns the ones with a still-live process
/// against their `prefix_path`, deleting stale entries (nothing live, or
/// unparsable) as it goes. This is the only reader of `state/running/` —
/// call it instead of scanning the directory directly, so a launch that was
/// hard-killed (never reaching its own clean-exit removal) doesn't linger
/// forever.
pub fn list_live() -> Result<Vec<RunningEntry>> {
    let dir = dir()?;
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut live = Vec::new();
    for entry in fs::read_dir(&dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(parsed) = serde_json::from_str::<RunningEntry>(&raw) else {
            fs::remove_file(&path).ok();
            continue;
        };
        if !matching_pids(&parsed.prefix_path).is_empty() {
            live.push(parsed);
        } else {
            fs::remove_file(&path).ok();
        }
    }
    Ok(live)
}

/// Whether anything is currently running against `prefix_path` — use this
/// before touching a prefix's wineserver (e.g. resetting a stale one) to
/// avoid killing a session that's actually still in use.
pub fn is_prefix_active(prefix_path: &str) -> bool {
    !matching_pids(prefix_path).is_empty()
}

/// Every currently-running pid whose `WINEPREFIX` is `prefix_path` or nested
/// under it (Proton's own inner layer reports `<prefix_path>/pfx` rather than
/// `prefix_path` itself). This — not pid/pgid/session — is what actually
/// identifies "everything belonging to this one launch": confirmed by
/// testing that `bwrap` and everything it sandboxes carry this env var
/// through even after `bwrap` re-sessions itself away from the process we
/// originally spawned.
fn matching_pids(prefix_path: &str) -> Vec<u32> {
    let want_exact = prefix_path.trim_end_matches('/');
    let want_nested = format!("{want_exact}/");

    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let pid: u32 = e.file_name().to_str()?.parse().ok()?;
            let environ = fs::read(e.path().join("environ")).ok()?;
            let wineprefix = environ
                .split(|&b| b == 0)
                .find_map(|kv| std::str::from_utf8(kv).ok()?.strip_prefix("WINEPREFIX="))?;
            (wineprefix == want_exact || wineprefix.starts_with(&want_nested)).then_some(pid)
        })
        .collect()
}

/// Sends SIGTERM to every process whose `WINEPREFIX` matches `prefix_path` —
/// see `matching_pids`' doc comment for why that, rather than a single pid or
/// process group, is what actually reaches the sandboxed tree `umu-run`
/// creates. Shells out to the system `kill` rather than pulling in a
/// signal-handling crate — this project is Linux-only already (see the
/// project CLAUDE.md's Target-gated code section), and `kill` is universal
/// there.
pub fn terminate(prefix_path: &str) -> Result<()> {
    let pids = matching_pids(prefix_path);
    if pids.is_empty() {
        bail!("nothing running against prefix {prefix_path}");
    }
    let status = Command::new("kill")
        .args(pids.iter().map(u32::to_string))
        .status()
        .context("failed to run `kill`")?;
    if !status.success() {
        bail!("`kill` exited with {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;

    /// Spawns a real, short-lived process carrying a fake `WINEPREFIX`, so
    /// `matching_pids`/`terminate` are exercised against an actual pid rather
    /// than a mock — this project's own convention for tests that touch a
    /// real external resource (here, `/proc`).
    fn spawn_with_prefix(prefix: &str) -> std::process::Child {
        Command::new("sleep")
            .arg("30")
            .env("WINEPREFIX", prefix)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn `sleep` for the test")
    }

    #[test]
    fn matching_pids_finds_exact_and_nested_wineprefix() {
        let prefix = format!("/tmp/iprolaunch-test-prefix-{}", std::process::id());
        let mut exact = spawn_with_prefix(&prefix);
        let mut nested = spawn_with_prefix(&format!("{prefix}/pfx"));

        let found = matching_pids(&prefix);
        assert!(found.contains(&exact.id()));
        assert!(found.contains(&nested.id()));

        exact.kill().ok();
        nested.kill().ok();
        exact.wait().ok();
        nested.wait().ok();
    }

    #[test]
    fn matching_pids_is_empty_for_an_unused_prefix() {
        assert!(matching_pids("/nonexistent/prefix/for/testing").is_empty());
    }

    #[test]
    fn terminate_kills_every_matching_pid() {
        let prefix = format!("/tmp/iprolaunch-test-prefix-term-{}", std::process::id());
        let mut child = spawn_with_prefix(&prefix);
        // Give /proc a moment to reflect the freshly-spawned process.
        std::thread::sleep(std::time::Duration::from_millis(200));

        terminate(&prefix).expect("terminate should find and kill the spawned process");
        let status = child.wait().expect("wait on killed child");
        assert!(!status.success());
    }
}
