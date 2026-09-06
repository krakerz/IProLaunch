use std::ffi::OsStr;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result};

use crate::config::{Effective, LogMode, Profile, RecordMode};

/// One launch's log file, from creation through the keep/discard decision.
pub struct LogSession {
    path: PathBuf,
    dir: PathBuf,
    slug: String,
    record: RecordMode,
    keep: u32,
}

/// Where one launch's log file lives, before it's actually created — kept
/// separate from `LogSession::start` (which follows this up with
/// `fs::create_dir_all`) so the directory *choice* is unit-testable without
/// touching disk: `Profile::profiles_dir()` itself is a pure computation
/// (just `project_dirs()` + a join), no I/O, so calling it in a test is
/// safe — only actually creating the directory would touch the real
/// `~/.config/iprolaunch/` tree.
fn session_dir(effective: &Effective, log_dir: &Path, slug: &str) -> Result<PathBuf> {
    Ok(match effective.log_mode {
        LogMode::Each => Profile::profiles_dir()?.join(slug).join("logs"),
        LogMode::Single => log_dir.to_path_buf(),
    })
}

impl LogSession {
    /// Opens a fresh log file for one launch. Caller must check
    /// `effective.record != RecordMode::Off` before calling this — `Off` means
    /// don't create a session (and stdout/stderr should pass through instead).
    /// `log_dir` (from `Config::log_dir`) is only used for `LogMode::Single`
    /// — `Each` ignores it entirely and writes inside the profile's own
    /// folder instead (see `LogMode::Each`'s doc comment).
    pub fn start(effective: &Effective, log_dir: &Path, slug: &str) -> Result<Self> {
        let dir = session_dir(effective, log_dir, slug)?;
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

        let stamp = timestamp();
        let path = dir.join(format!("{slug}-{stamp}.log"));

        Ok(Self {
            path,
            dir,
            slug: slug.to_string(),
            record: effective.record,
            keep: effective.keep,
        })
    }

    /// Stdio targets for `Command::stdout`/`stderr`. Both point at the same
    /// underlying file via a cloned handle (sharing the OS file offset) so
    /// interleaved writes from the two streams don't overwrite each other —
    /// two independent `File::create` calls on the same path would each start
    /// writing at offset 0 and clobber whichever stream wrote second.
    pub fn stdio_pair(&self) -> Result<(Stdio, Stdio)> {
        let file = File::create(&self.path)
            .with_context(|| format!("creating {}", self.path.display()))?;
        let dup = file.try_clone().context("cloning log file handle")?;
        Ok((Stdio::from(file), Stdio::from(dup)))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Call once the child has exited. Discards the just-written log on a
    /// clean exit when `record == Errors`, then prunes this exe's remaining
    /// history down to `keep`. Returns the surviving path, if any.
    pub fn finish(self, exit_success: bool) -> Result<Option<PathBuf>> {
        if self.record == RecordMode::Errors && exit_success {
            let _ = fs::remove_file(&self.path);
            return Ok(None);
        }
        prune(&self.dir, &self.slug, self.keep)?;
        Ok(Some(self.path))
    }
}

fn timestamp() -> String {
    let now = crate::config::now_local();
    now.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| now.unix_timestamp().to_string())
        .replace(':', "-")
}

/// Deletes the oldest `*.log` files for `slug_prefix` in `dir` beyond `keep`.
fn prune(dir: &Path, slug_prefix: &str, keep: u32) -> Result<()> {
    let prefix = format!("{slug_prefix}-");
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|n| n.starts_with(&prefix) && n.ends_with(".log"))
        })
        .collect();

    entries.sort_by_key(|p| fs::metadata(p).and_then(|m| m.modified()).ok());
    entries.reverse(); // newest first

    for stale in entries.into_iter().skip(keep as usize) {
        let _ = fs::remove_file(stale);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, PrefixMode};

    #[test]
    fn prunes_down_to_keep_count_scoped_by_slug() {
        let tmp = std::env::temp_dir().join(format!("iprolaunch-test-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();

        for i in 0..5 {
            fs::write(tmp.join(format!("game-2026010{i}T000000-00-00.log")), "x").unwrap();
        }
        fs::write(tmp.join("other-20260101T000000-00-00.log"), "x").unwrap();

        prune(&tmp, "game", 2).unwrap();

        let remaining: Vec<_> = fs::read_dir(&tmp)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();

        assert_eq!(
            remaining.iter().filter(|n| n.starts_with("game-")).count(),
            2
        );
        assert!(remaining.iter().any(|n| n.starts_with("other-")));

        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn effective_defaults_are_sane() {
        let cfg = Config::default();
        assert_eq!(cfg.defaults.prefix_mode, PrefixMode::Single);
    }

    fn test_effective(log_mode: LogMode) -> Effective {
        Effective {
            proton: "system".to_string(),
            prefix_mode: PrefixMode::Single,
            prefix_path: "~/unused".to_string(),
            prefixes_root: "~/unused".to_string(),
            windows_version: None,
            gamescope: Default::default(),
            gamescope_settings: Default::default(),
            log_mode,
            keep: 3,
            record: RecordMode::Errors,
            auto_open: false,
            env: Default::default(),
            winedlloverride: Default::default(),
        }
    }

    #[test]
    fn single_mode_uses_the_given_log_dir_as_is() {
        let effective = test_effective(LogMode::Single);
        let dir = session_dir(&effective, Path::new("/tmp/custom-logs"), "game-1").unwrap();
        assert_eq!(dir, Path::new("/tmp/custom-logs"));
    }

    #[test]
    fn each_mode_ignores_log_dir_and_uses_the_profile_folder_instead() {
        // Safe to call `session_dir` (and so `Profile::profiles_dir`) here —
        // it's a pure path computation, no `fs::create_dir_all` — unlike
        // `LogSession::start`, which isn't exercised in this test.
        let effective = test_effective(LogMode::Each);
        let dir = session_dir(&effective, Path::new("/tmp/custom-logs"), "game-1").unwrap();
        assert!(
            dir.ends_with("profiles/game-1/logs"),
            "expected a .../profiles/game-1/logs path, got {}",
            dir.display()
        );
        assert!(!dir.starts_with("/tmp/custom-logs"));
    }
}
