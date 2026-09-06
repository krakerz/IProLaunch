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
    /// This launch's own `IPROLAUNCH_LAUNCH_ID` value — what liveness/kill
    /// actually key on (see `matching_pids_for_launch`). `prefix_path` alone
    /// can't identify "this one launch": in `single` prefix mode every
    /// profile shares the same `WINEPREFIX`, so two games running at once
    /// are otherwise indistinguishable — a real reported bug (killing one
    /// killed both), confirmed and fixed 2026-09-06.
    pub launch_id: String,
    pub started_at: String,
}

fn dir() -> Result<PathBuf> {
    Ok(project_dirs()?.config_dir().join("state").join("running"))
}

/// A value unique enough to tag one specific launch's whole process tree,
/// set as `IPROLAUNCH_LAUNCH_ID` on the spawned command (propagates through
/// `bwrap`'s sandbox exactly like `WINEPREFIX` already does — same
/// mechanism, just a value that's actually unique per launch instead of
/// shared across every launch against the same prefix). Not a real UUID —
/// this process's own pid plus a nanosecond timestamp is already far more
/// precise than two launches could plausibly collide on.
pub fn new_launch_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{}-{nanos}", std::process::id())
}

/// Writes the state file for a freshly-spawned launch. Returns its path so
/// the caller can remove it on its own clean exit.
pub fn record(
    pid: u32,
    name: &str,
    target_path: &Path,
    prefix_path: &Path,
    launch_id: &str,
) -> Result<PathBuf> {
    let dir = dir()?;
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(format!("{pid}.json"));
    let entry = RunningEntry {
        pid,
        name: name.to_string(),
        target_path: target_path.to_string_lossy().into_owned(),
        prefix_path: prefix_path.to_string_lossy().into_owned(),
        launch_id: launch_id.to_string(),
        started_at: crate::config::now_local()
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

/// Reads every state file and returns the ones whose own launch is still
/// live, deleting stale entries (nothing live, or unparsable) as it goes.
/// This is the only reader of `state/running/` — call it instead of
/// scanning the directory directly, so a launch that was hard-killed (never
/// reaching its own clean-exit removal) doesn't linger forever.
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
        if !matching_pids_for_launch(&parsed.launch_id).is_empty() {
            live.push(parsed);
        } else {
            fs::remove_file(&path).ok();
        }
    }
    Ok(live)
}

/// Whether anything at all is currently running against `prefix_path` —
/// deliberately prefix-wide (unlike `terminate`, which acts on one launch),
/// used before touching a prefix's wineserver (e.g. resetting a stale one)
/// to avoid tearing down a session that's actually still in use, by a
/// *different* launch sharing the same prefix in `single` mode.
pub fn is_prefix_active(prefix_path: &str) -> bool {
    !matching_pids_for_prefix(prefix_path).is_empty()
}

/// Scans `/proc/*/environ` for every pid carrying `var=<something matches>`.
/// Shared by the prefix-wide and per-launch lookups below — both need the
/// same walk, just a different env var and match rule.
fn matching_pids_by_env(var: &str, matches: impl Fn(&str) -> bool) -> Vec<u32> {
    let needle = format!("{var}=");
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let pid: u32 = e.file_name().to_str()?.parse().ok()?;
            let environ = fs::read(e.path().join("environ")).ok()?;
            let value = environ
                .split(|&b| b == 0)
                .find_map(|kv| std::str::from_utf8(kv).ok()?.strip_prefix(needle.as_str()))?;
            matches(value).then_some(pid)
        })
        .collect()
}

/// Every currently-running pid whose `WINEPREFIX` is `prefix_path` or nested
/// under it (Proton's own inner layer reports `<prefix_path>/pfx` rather than
/// `prefix_path` itself). Identifies "everything sharing this one prefix" —
/// *not* "everything belonging to one launch": in `single` prefix mode every
/// profile shares the same `WINEPREFIX`, so two concurrent launches are
/// otherwise indistinguishable by this alone (a real reported bug — killing
/// one killed both — is why `matching_pids_for_launch` exists instead, for
/// anything that needs to act on just one launch).
fn matching_pids_for_prefix(prefix_path: &str) -> Vec<u32> {
    let want_exact = prefix_path.trim_end_matches('/').to_string();
    let want_nested = format!("{want_exact}/");
    matching_pids_by_env("WINEPREFIX", |v| {
        v == want_exact || v.starts_with(&want_nested)
    })
}

/// Every currently-running pid tagged with this launch's own
/// `IPROLAUNCH_LAUNCH_ID` (set by `launch::run` on the spawned command,
/// alongside `WINEPREFIX` — confirmed to propagate through `bwrap`'s sandbox
/// the same way). Unlike `WINEPREFIX`, this value is unique per launch even
/// when several profiles share one prefix, so it's what actually identifies
/// "everything belonging to this one launch" for killing/liveness checks.
fn matching_pids_for_launch(launch_id: &str) -> Vec<u32> {
    matching_pids_by_env("IPROLAUNCH_LAUNCH_ID", |v| v == launch_id)
}

/// Sends SIGTERM to every process belonging to this one launch (see
/// `matching_pids_for_launch`'s doc comment for why that, rather than a
/// single pid, process group, or a shared `WINEPREFIX`, is what actually
/// reaches — and *only* reaches — the sandboxed tree `umu-run` creates for
/// this specific launch). Shells out to the system `kill` rather than
/// pulling in a signal-handling crate — this project is Linux-only already
/// (see the project CLAUDE.md's Target-gated code section), and `kill` is
/// universal there.
pub fn terminate(launch_id: &str) -> Result<()> {
    let pids = matching_pids_for_launch(launch_id);
    if pids.is_empty() {
        bail!("nothing running against launch {launch_id}");
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

    /// Spawns a real, short-lived process carrying a fake env var, so
    /// `matching_pids_for_*`/`terminate` are exercised against an actual pid
    /// rather than a mock — this project's own convention for tests that
    /// touch a real external resource (here, `/proc`).
    fn spawn_with_env(var: &str, value: &str) -> std::process::Child {
        Command::new("sleep")
            .arg("30")
            .env(var, value)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn `sleep` for the test")
    }

    #[test]
    fn matching_pids_for_prefix_finds_exact_and_nested_wineprefix() {
        let prefix = format!("/tmp/iprolaunch-test-prefix-{}", std::process::id());
        let mut exact = spawn_with_env("WINEPREFIX", &prefix);
        let mut nested = spawn_with_env("WINEPREFIX", &format!("{prefix}/pfx"));
        // Give /proc a moment to reflect the freshly-spawned processes (same
        // race the terminate tests below guard against).
        std::thread::sleep(std::time::Duration::from_millis(200));

        let found = matching_pids_for_prefix(&prefix);
        assert!(found.contains(&exact.id()));
        assert!(found.contains(&nested.id()));

        exact.kill().ok();
        nested.kill().ok();
        exact.wait().ok();
        nested.wait().ok();
    }

    #[test]
    fn matching_pids_for_prefix_is_empty_for_an_unused_prefix() {
        assert!(matching_pids_for_prefix("/nonexistent/prefix/for/testing").is_empty());
    }

    #[test]
    fn terminate_kills_every_pid_for_that_launch() {
        let launch_id = format!("iprolaunch-test-launch-term-{}", std::process::id());
        let mut child = spawn_with_env("IPROLAUNCH_LAUNCH_ID", &launch_id);
        // Give /proc a moment to reflect the freshly-spawned process.
        std::thread::sleep(std::time::Duration::from_millis(200));

        terminate(&launch_id).expect("terminate should find and kill the spawned process");
        let status = child.wait().expect("wait on killed child");
        assert!(!status.success());
    }

    /// The actual reported bug, reproduced directly: in `single` prefix
    /// mode, two launches share the same `WINEPREFIX` but each carries its
    /// own `IPROLAUNCH_LAUNCH_ID` — killing one by its launch id must never
    /// touch the other, even though they share a prefix.
    #[test]
    fn terminate_by_launch_id_does_not_touch_a_different_launch_sharing_the_same_prefix() {
        let prefix = format!("/tmp/iprolaunch-test-shared-prefix-{}", std::process::id());
        let launch_a = format!("iprolaunch-test-launch-a-{}", std::process::id());
        let launch_b = format!("iprolaunch-test-launch-b-{}", std::process::id());

        let mut a = Command::new("sleep")
            .arg("30")
            .env("WINEPREFIX", &prefix)
            .env("IPROLAUNCH_LAUNCH_ID", &launch_a)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn launch A");
        let mut b = Command::new("sleep")
            .arg("30")
            .env("WINEPREFIX", &prefix)
            .env("IPROLAUNCH_LAUNCH_ID", &launch_b)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn launch B");
        std::thread::sleep(std::time::Duration::from_millis(200));

        terminate(&launch_a).expect("terminate should find and kill launch A");
        let status_a = a.wait().expect("wait on killed launch A");
        assert!(!status_a.success());

        // Launch B must still be alive — confirmed via a real, direct
        // /proc probe rather than assumed.
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(matching_pids_for_launch(&launch_b).contains(&b.id()));

        b.kill().ok();
        b.wait().ok();
    }
}
