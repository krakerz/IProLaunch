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

/// Result of a GAMEID lookup. `Ambiguous` carries the conflicting UMU_IDs.
#[derive(Debug, PartialEq, Eq)]
pub enum GameIdMatch {
    Found(String),
    Ambiguous(Vec<String>),
    None,
}

/// Looks up a UMU_ID against the cached database: exact `TITLE` match on
/// `query` first (case-insensitive; the profile's `title`, falling back to
/// the exe's file stem when unset), then an `EXE_STRINGS` match against
/// `target`'s path. A tier whose rows resolve to more than one distinct
/// UMU_ID uses its `none`-store row (the standalone install) if that is
/// unique, else it is `Ambiguous` and the caller leaves GAMEID unset
/// (umu-default) rather than guess another game's protonfixes. No cache yet
/// or a malformed row is just `None` — never an error, since this must not
/// block a launch.
pub fn lookup_gameid(query: &str, target: &Path) -> GameIdMatch {
    match cache_path() {
        Ok(path) => lookup_gameid_in(&path, query, target),
        Err(_) => GameIdMatch::None,
    }
}

fn lookup_gameid_in(path: &Path, query: &str, target: &Path) -> GameIdMatch {
    let Ok(file) = fs::File::open(path) else {
        return GameIdMatch::None;
    };
    let mut reader = csv::Reader::from_reader(file);
    let target_lower = target.to_string_lossy().to_lowercase();

    // (UMU_ID, STORE) per matching row; tiers are resolved after the scan.
    let mut title_rows: Vec<(String, String)> = Vec::new();
    let mut exe_rows: Vec<(String, String)> = Vec::new();
    for record in reader.records().flatten() {
        let (Some(title), Some(store), Some(umu_id)) =
            (record.get(0), record.get(1), record.get(3))
        else {
            continue;
        };

        let bucket = if title.eq_ignore_ascii_case(query) {
            &mut title_rows
        } else if record
            .get(6)
            .is_some_and(|exe| path_ends_with(&target_lower, &exe.to_lowercase()))
        {
            &mut exe_rows
        } else {
            continue;
        };
        bucket.push((umu_id.to_string(), store.to_string()));
    }

    let rows = if title_rows.is_empty() {
        exe_rows
    } else {
        title_rows
    };
    resolve_tier(&rows)
}

/// Same title across stores usually shares one UMU_ID; only distinct IDs
/// count as ambiguous.
fn resolve_tier(rows: &[(String, String)]) -> GameIdMatch {
    let distinct = |rows: &mut dyn Iterator<Item = &(String, String)>| {
        let mut ids: Vec<String> = Vec::new();
        for (id, _) in rows {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
        ids
    };
    let ids = distinct(&mut rows.iter());
    if ids.len() > 1 {
        let standalone = distinct(&mut rows.iter().filter(|(_, store)| store == "none"));
        if let [id] = standalone.as_slice() {
            return GameIdMatch::Found(id.clone());
        }
    }
    match ids.len() {
        0 => GameIdMatch::None,
        1 => GameIdMatch::Found(ids.into_iter().next().unwrap()),
        _ => GameIdMatch::Ambiguous(ids),
    }
}

/// One umu-database game for the profile editor's GAMEID picker: rows sharing
/// a title and UMU_ID are merged, with their stores listed together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameDbEntry {
    pub title: String,
    pub umu_id: String,
    pub stores: Vec<String>,
}

/// Whether the umu-database cache has been downloaded yet, so the TUI can
/// hint at it instead of showing a bare "no matches".
pub fn cache_exists() -> bool {
    cache_path().is_ok_and(|path| path.is_file())
}

/// Case-insensitive substring search over `TITLE`, `COMMON ACRONYM` and
/// `UMU_ID`, in database order. An empty query lists everything. Missing
/// cache → empty list.
pub fn search(query: &str) -> Vec<GameDbEntry> {
    cache_path()
        .map(|path| search_in(&path, query))
        .unwrap_or_default()
}

fn search_in(path: &Path, query: &str) -> Vec<GameDbEntry> {
    let Ok(file) = fs::File::open(path) else {
        return Vec::new();
    };
    let query = query.trim().to_lowercase();
    let mut out: Vec<GameDbEntry> = Vec::new();
    for record in csv::Reader::from_reader(file).records().flatten() {
        let (Some(title), Some(store), Some(umu_id)) =
            (record.get(0), record.get(1), record.get(3))
        else {
            continue;
        };
        let acronym = record.get(4).unwrap_or("");
        let hit = [title, acronym, umu_id]
            .iter()
            .any(|field| field.to_lowercase().contains(&query));
        if !hit {
            continue;
        }
        match out
            .iter_mut()
            .find(|e| e.umu_id == umu_id && e.title.eq_ignore_ascii_case(title))
        {
            Some(entry) => {
                if !entry.stores.iter().any(|s| s == store) {
                    entry.stores.push(store.to_string());
                }
            }
            None => out.push(GameDbEntry {
                title: title.to_string(),
                umu_id: umu_id.to_string(),
                stores: vec![store.to_string()],
            }),
        }
    }
    out
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
Genshin Impact,egs,abc,umu-genshin-egs,,,
Genshin Impact,none,playpc,umu-genshin,,,
Split Game,egs,a,umu-split-egs,,,
Split Game,gog,b,umu-split-gog,,,
Age of Wonders,gog,1207658883,umu-61500,aow,,
Age of Wonders,humble,ageofwonders,umu-61500,aow,,
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
            GameIdMatch::Found("umu-271590".to_string())
        );
        assert_eq!(
            lookup_gameid_in(&path, "grand theft auto v", Path::new(NO_TARGET)),
            GameIdMatch::Found("umu-271590".to_string())
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
            GameIdMatch::Found("umu-999999".to_string())
        );
        assert_eq!(
            lookup_gameid_in(&path, "SPEED2", Path::new("/games/NFS/SPEED2.EXE")),
            GameIdMatch::Found("umu-888888".to_string())
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
            GameIdMatch::None
        );
        // Right exe name, wrong parent folder.
        assert_eq!(
            lookup_gameid_in(
                &path,
                "EMLauncher",
                Path::new("/games/Other/EMLauncher.exe")
            ),
            GameIdMatch::None
        );
        assert_eq!(
            lookup_gameid_in(&path, "xspeed2", Path::new("/games/xspeed2.exe")),
            GameIdMatch::None
        );
        fs::remove_file(&path).ok();
    }

    #[test]
    fn duplicate_title_with_one_id_still_matches() {
        let path = write_sample("dup-same-id");
        assert_eq!(
            lookup_gameid_in(&path, "Age of Wonders", Path::new(NO_TARGET)),
            GameIdMatch::Found("umu-61500".to_string())
        );
        fs::remove_file(&path).ok();
    }

    #[test]
    fn several_ids_prefer_the_standalone_row() {
        let path = write_sample("dup-standalone");
        assert_eq!(
            lookup_gameid_in(&path, "genshin impact", Path::new(NO_TARGET)),
            GameIdMatch::Found("umu-genshin".to_string())
        );
        fs::remove_file(&path).ok();
    }

    #[test]
    fn several_ids_without_standalone_row_are_ambiguous() {
        let path = write_sample("dup-many-ids");
        assert_eq!(
            lookup_gameid_in(&path, "Split Game", Path::new(NO_TARGET)),
            GameIdMatch::Ambiguous(vec![
                "umu-split-egs".to_string(),
                "umu-split-gog".to_string()
            ])
        );
        fs::remove_file(&path).ok();
    }

    #[test]
    fn search_merges_stores_and_matches_title_acronym_or_id() {
        let path = write_sample("search");
        let hits = search_in(&path, "aow");
        assert_eq!(
            hits,
            vec![GameDbEntry {
                title: "Age of Wonders".to_string(),
                umu_id: "umu-61500".to_string(),
                stores: vec!["gog".to_string(), "humble".to_string()],
            }]
        );
        assert_eq!(search_in(&path, "GENSHIN").len(), 2);
        assert_eq!(search_in(&path, "umu-888888")[0].title, "Speed 2");
        assert!(search_in(&path, "zzz-nothing").is_empty());
        assert!(search_in(Path::new("/nonexistent.csv"), "x").is_empty());
        fs::remove_file(&path).ok();
    }

    #[test]
    fn no_match_returns_none() {
        let path = write_sample("no-match");
        assert_eq!(
            lookup_gameid_in(&path, "Some Totally Unrelated Game", Path::new(NO_TARGET)),
            GameIdMatch::None
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
            GameIdMatch::None
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
