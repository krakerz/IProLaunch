use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};

use crate::config::project_dirs;

/// Raw CSV, not a cut-down index — fetched wholesale and re-parsed on each
/// lookup. At ~1200 rows this is cheap; no need for a real index (see
/// project TODO's note on why a cache/index was skipped for now).
const SOURCE_URL: &str =
    "https://raw.githubusercontent.com/Open-Wine-Components/umu-database/main/umu-database.csv";

fn cache_path() -> Result<PathBuf> {
    Ok(project_dirs()?.cache_dir().join("umu-database.csv"))
}

fn is_stale(path: &Path, interval_days: u32) -> bool {
    let Ok(modified) = fs::metadata(path).and_then(|m| m.modified()) else {
        return true; // missing, or mtime unavailable — treat as stale
    };
    let age = SystemTime::now()
        .duration_since(modified)
        .unwrap_or(Duration::ZERO);
    age > Duration::from_secs(u64::from(interval_days) * 86_400)
}

/// Re-downloads the umu-database CSV if the cached copy is missing or older
/// than `interval_days`. Best-effort: any failure (no network, timeout, bad
/// response) is swallowed with just a warning — GAMEID lookup silently
/// falling back to "no match" is far better than blocking a game launch on
/// a database refresh.
pub fn refresh_if_stale(interval_days: u32) {
    if let Err(err) = try_refresh(interval_days) {
        eprintln!("iprolaunch: warning: umu-database refresh skipped: {err:#}");
    }
}

fn try_refresh(interval_days: u32) -> Result<()> {
    refresh_path_if_stale(&cache_path()?, interval_days)
}

fn refresh_path_if_stale(path: &Path, interval_days: u32) -> Result<()> {
    if !is_stale(path, interval_days) {
        return Ok(());
    }

    let body: String = ureq::get(SOURCE_URL)
        .config()
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .call()
        .context("fetching umu-database.csv")?
        .body_mut()
        .read_to_string()
        .context("reading response body")?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    // Write-then-rename: a fetch interrupted partway never leaves a
    // truncated/corrupt cache in place of a previously-good one.
    let tmp = path.with_extension("csv.tmp");
    fs::write(&tmp, &body).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("renaming into {}", path.display()))?;
    Ok(())
}

/// Looks up a UMU_ID against the cached database: exact `TITLE` match on
/// `query` first (case-insensitive; the profile's `title`, falling back to
/// the exe's file stem when unset), then an `EXE_STRINGS` match against
/// `target`'s path. Returns `None` on no match, no cache yet, or a malformed
/// cache row — never an error, since this is inherently best-effort and must
/// not block a launch.
pub fn lookup_gameid(query: &str, target: &Path) -> Option<String> {
    lookup_gameid_in(&cache_path().ok()?, query, target)
}

fn lookup_gameid_in(path: &Path, query: &str, target: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let mut reader = csv::Reader::from_reader(file);
    let target_lower = target.to_string_lossy().to_lowercase();

    let mut exe_match: Option<String> = None;
    for record in reader.records().flatten() {
        let (Some(title), Some(umu_id)) = (record.get(0), record.get(3)) else {
            continue;
        };

        if title.eq_ignore_ascii_case(query) {
            return Some(umu_id.to_string());
        }

        if exe_match.is_none()
            && let Some(exe_string) = record.get(6)
            && path_ends_with(&target_lower, &exe_string.to_lowercase())
        {
            exe_match = Some(umu_id.to_string());
        }
    }
    exe_match
}

/// `EXE_STRINGS` entries are path tails like `Duet Night Abyss/EMLauncher.exe`
/// — match only on whole components, since a raw substring test let a generic
/// stem like `Launcher` match `EMLauncher.exe` and pull in another game's fixes.
fn path_ends_with(target_lower: &str, exe_string_lower: &str) -> bool {
    !exe_string_lower.is_empty()
        && target_lower
            .strip_suffix(exe_string_lower)
            .is_some_and(|rest| rest.is_empty() || rest.ends_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_CSV: &str = "TITLE,STORE,CODENAME,UMU_ID,COMMON ACRONYM (Optional),NOTE (Optional),EXE_STRINGS (Optional)
Grand Theft Auto V,egs,9d2d0eb64d5c44529cece33fe2a46482,umu-271590,gtav,,
Duet Night Abyss,none,none,umu-999999,,,Duet Night Abyss/EMLauncher.exe
Speed 2,none,none,umu-888888,,,speed2.exe
";
    const NO_TARGET: &str = "/games/unrelated/Game.exe";

    fn write_sample(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "iprolaunch-gamedb-test-{name}-{}.csv",
            std::process::id()
        ));
        fs::write(&path, SAMPLE_CSV).unwrap();
        path
    }

    #[test]
    fn exact_title_match_wins() {
        let path = write_sample("exact");
        assert_eq!(
            lookup_gameid_in(&path, "Grand Theft Auto V", Path::new(NO_TARGET)),
            Some("umu-271590".to_string())
        );
        assert_eq!(
            lookup_gameid_in(&path, "grand theft auto v", Path::new(NO_TARGET)),
            Some("umu-271590".to_string())
        );
        fs::remove_file(&path).ok();
    }

    #[test]
    fn exe_strings_match_target_path_tail_as_fallback() {
        let path = write_sample("exe-strings");
        assert_eq!(
            lookup_gameid_in(
                &path,
                "EMLauncher",
                Path::new("/games/Duet Night Abyss/EMLauncher.exe")
            ),
            Some("umu-999999".to_string())
        );
        assert_eq!(
            lookup_gameid_in(&path, "SPEED2", Path::new("/games/NFS/SPEED2.EXE")),
            Some("umu-888888".to_string())
        );
        fs::remove_file(&path).ok();
    }

    #[test]
    fn exe_strings_need_whole_path_components() {
        let path = write_sample("exe-components");
        // `Launcher` is a substring of `EMLauncher.exe` — must not match.
        assert_eq!(
            lookup_gameid_in(
                &path,
                "Launcher",
                Path::new("/games/GRYPHLINK/Launcher.exe")
            ),
            None
        );
        // Right exe name, wrong parent folder.
        assert_eq!(
            lookup_gameid_in(
                &path,
                "EMLauncher",
                Path::new("/games/Other/EMLauncher.exe")
            ),
            None
        );
        assert_eq!(
            lookup_gameid_in(&path, "xspeed2", Path::new("/games/xspeed2.exe")),
            None
        );
        fs::remove_file(&path).ok();
    }

    #[test]
    fn no_match_returns_none() {
        let path = write_sample("no-match");
        assert_eq!(
            lookup_gameid_in(&path, "Some Totally Unrelated Game", Path::new(NO_TARGET)),
            None
        );
        fs::remove_file(&path).ok();
    }

    #[test]
    fn missing_cache_returns_none_not_error() {
        assert_eq!(
            lookup_gameid_in(
                Path::new("/nonexistent/umu-database.csv"),
                "anything",
                Path::new(NO_TARGET)
            ),
            None
        );
    }

    #[test]
    fn is_stale_true_when_cache_missing() {
        assert!(is_stale(Path::new("/nonexistent/umu-database.csv"), 7));
    }

    #[test]
    fn is_stale_false_for_a_freshly_written_cache() {
        let path =
            std::env::temp_dir().join(format!("iprolaunch-fresh-{}.csv", std::process::id()));
        fs::write(&path, "x").unwrap();
        assert!(!is_stale(&path, 7));
        fs::remove_file(&path).ok();
    }

    #[test]
    fn refresh_skips_network_entirely_when_cache_is_fresh() {
        // No network mocking here on purpose — a fresh cache must short-circuit
        // before ever reaching the `ureq::get` call, so this stays a safe,
        // no-network test rather than one that'd need `#[ignore]`.
        let path = std::env::temp_dir().join(format!(
            "iprolaunch-refresh-fresh-{}.csv",
            std::process::id()
        ));
        fs::write(&path, "already-here").unwrap();
        refresh_path_if_stale(&path, 7).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "already-here");
        fs::remove_file(&path).ok();
    }
}
