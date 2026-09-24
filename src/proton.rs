use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use directories::UserDirs;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtonBuild {
    /// What gets stored as `defaults.proton` (or a profile override) and
    /// passed straight through as `PROTONPATH`, which `man umu` documents
    /// as accepting a path, a version name, or a codename. For a
    /// `compatibilitytools.d` entry under one of `steam_roots()` this is
    /// just the folder name — confirmed from `umu-run`'s own source
    /// (`resolve_runtime`): a non-absolute `PROTONPATH` is resolved by
    /// joining it against a single hardcoded `STEAM_COMPAT` path, which is
    /// exactly `<the user's own Steam root>/compatibilitytools.d`, so the
    /// bare name only resolves for builds actually living there. For an
    /// official Steam-shipped build under `steamapps/common`, or a build
    /// found under a *system-wide* compat-tools dir (see
    /// `system_compatibilitytools_dirs`, e.g. CachyOS's `proton-cachyos-slr`
    /// package under `/usr/share/steam/compatibilitytools.d`) — neither of
    /// which `STEAM_COMPAT` ever points at — the bare name doesn't resolve
    /// at all (confirmed by a real failure: `PROTONPATH 'proton-cachyos-slr'
    /// is not valid, toolmanifest.vdf not found`), so this is the build's
    /// absolute path instead, which `resolve_runtime` accepts unconditionally.
    pub id: String,
    pub display_name: String,
}

/// Every place this machine might have a Steam install — the standard
/// native path, the `~/.steam/steam` symlink some distros set up pointing
/// at it (deduped below since it'd otherwise double-count every build), and
/// the Flatpak sandbox's data dir. Checked online against Valve's/Flatpak's
/// own documented layouts rather than assumed.
pub(crate) fn steam_roots() -> Result<Vec<PathBuf>> {
    let home = UserDirs::new().context("could not determine home directory")?;
    let home = home.home_dir();
    let candidates = [
        home.join(".local/share/Steam"),
        home.join(".steam/steam"),
        home.join(".steam/root"),
        home.join(".var/app/com.valvesoftware.Steam/data/Steam"),
    ];

    let mut seen = HashSet::new();
    Ok(candidates
        .into_iter()
        .filter(|p| p.is_dir())
        .filter_map(|p| p.canonicalize().ok())
        .filter(|p| seen.insert(p.clone()))
        .collect())
}

/// System-wide (not per-user) compat-tool locations some distro packages
/// install a default Proton build into — confirmed present on this machine
/// at `/usr/share/steam/compatibilitytools.d` (CachyOS's
/// `proton-cachyos-slr` package installs there).
fn system_compatibilitytools_dirs() -> Vec<PathBuf> {
    vec![PathBuf::from("/usr/share/steam/compatibilitytools.d")]
}

/// Every library folder Steam itself knows about, parsed from
/// `steamapps/libraryfolders.vdf` under `root` — a small, predictable
/// manifest, not arbitrary input, so this just scans for `"path"` lines
/// (same approach `read_display_name` already uses for
/// `compatibilitytool.vdf`) rather than a full VDF/KeyValues parser.
/// Official Proton builds are ordinary Steam "apps" like any game, and can
/// be (and, on a real machine checked while building this, were) installed
/// to a secondary library on another drive rather than the main Steam
/// root — `steam_roots()` alone never finds those. Missing/unreadable
/// (no `libraryfolders.vdf`, e.g. a fresh or non-native install) just
/// means no *additional* libraries beyond `root` itself, not an error.
fn steam_library_paths(root: &Path) -> Vec<PathBuf> {
    let Ok(text) = fs::read_to_string(root.join("steamapps/libraryfolders.vdf")) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("\"path\"")?;
            Some(PathBuf::from(rest.trim().trim_matches('"')))
        })
        .collect()
}

/// Scans every known Steam root for both community Proton builds
/// (`compatibilitytools.d`) and official Valve-shipped ones
/// (`steamapps/common/Proton*` — in *every* library folder Steam has
/// configured for that root, not just the root's own, see
/// `steam_library_paths`), plus any system-wide compat-tool directory —
/// this is the one scan used everywhere a Proton build list is needed (the
/// `proton list` CLI command and the TUI's proton picker alike), so
/// improving it here improves both.
pub fn scan() -> Result<Vec<ProtonBuild>> {
    let mut builds = Vec::new();
    let mut common_dirs_seen = HashSet::new();
    for root in steam_roots()? {
        builds.extend(
            scan_compatibilitytools(&root.join("compatibilitytools.d"), false).unwrap_or_default(),
        );
        let mut common_dirs = vec![root.join("steamapps/common")];
        common_dirs.extend(
            steam_library_paths(&root)
                .into_iter()
                .map(|p| p.join("steamapps/common")),
        );
        for common_dir in common_dirs {
            if let Ok(canon) = common_dir.canonicalize()
                && common_dirs_seen.insert(canon)
            {
                builds.extend(scan_official_proton(&common_dir).unwrap_or_default());
            }
        }
    }
    for dir in system_compatibilitytools_dirs() {
        builds.extend(scan_compatibilitytools(&dir, true).unwrap_or_default());
    }
    builds.sort_by(|a, b| {
        a.display_name
            .to_lowercase()
            .cmp(&b.display_name.to_lowercase())
    });
    builds.dedup_by(|a, b| a.id == b.id);
    Ok(builds)
}

/// Resolves a stored `defaults.proton`/profile-override value (as returned
/// by `ProtonBuild::id`, or the special "system"/empty value) into the
/// directory containing that build's own `files/bin/wine` — needed for
/// anything (like winetricks) that has to invoke wine directly rather than
/// going through `umu-run`'s own resolution. Mirrors `umu-run`'s own
/// `resolve_runtime` logic for a *bare* name (join against `STEAM_COMPAT`,
/// i.e. the user's own Steam root's `compatibilitytools.d` — never a
/// system-wide dir, see `ProtonBuild::id`'s doc comment); an
/// already-absolute `id` (an official Steam build, or a system-wide compat
/// dir) is used as-is. Errors for "system"/empty — `umu-run` auto-manages
/// its own UMU-Proton then, with no fixed directory to point at.
pub fn resolve_binary_dir(id: &str) -> Result<PathBuf> {
    if id.is_empty() || id == "system" {
        bail!(
            "no explicit Proton build selected (defaults.proton is \"system\") — \
             pick one first via `proton list`/the TUI's proton picker"
        );
    }
    let path = Path::new(id);
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let home = UserDirs::new().context("could not determine home directory")?;
    Ok(home
        .home_dir()
        .join(".local/share/Steam/compatibilitytools.d")
        .join(id))
}

/// The distro's own `wine`/`wineserver` on `$PATH` — `run_winetricks`'s
/// fallback when `proton` is `"system"`, since `resolve_binary_dir` has no
/// pinned build to point winetricks at there at all (that setting means
/// "let `umu-run` auto-manage its own UMU-Proton build", not "use the
/// distro's Wine"). Worth having as a real fallback rather than just
/// failing outright: unlike `umu-launcher`/`umu-run` itself (actively
/// maintained), the specific UMU-Proton build it downloads for `"system"`
/// hasn't had a new release since 2026-03 — a distro Wine package is often
/// the more current, more reliable thing to point winetricks at anyway.
pub fn system_wine_binaries() -> Option<(PathBuf, PathBuf)> {
    let path = std::env::var_os("PATH")?;
    Some((
        find_on_path(&path, "wine")?,
        find_on_path(&path, "wineserver")?,
    ))
}

/// Pure PATH search, kept separate from `system_wine_binaries` so it's
/// testable without mutating the real process-wide `$PATH` — parallel tests
/// mutating a global env var would race each other.
fn find_on_path(path_var: &std::ffi::OsStr, name: &str) -> Option<PathBuf> {
    std::env::split_paths(path_var)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

/// Community Proton builds (GE-Proton, CachyOS Proton, umu's own
/// auto-downloaded UMU-Proton, etc). A directory only counts if it has a
/// `toolmanifest.vdf` — the same file Steam itself uses to recognize a
/// compatibility tool; confirmed present in every real build and absent
/// from anything else on this machine.
///
/// `absolute_id`: whether `dir` is one `umu-run` can't resolve a bare
/// folder name against (see `ProtonBuild::id`'s doc comment) — `false` for
/// a per-Steam-root `compatibilitytools.d` (the one directory umu-run's own
/// `STEAM_COMPAT` constant points at), `true` for anywhere else (system-wide
/// dirs), storing the build's absolute path as `id` instead.
fn scan_compatibilitytools(dir: &Path, absolute_id: bool) -> Result<Vec<ProtonBuild>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut builds = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path();
        if !path.join("toolmanifest.vdf").is_file() {
            continue;
        }
        let id = if absolute_id {
            path.to_string_lossy().into_owned()
        } else {
            entry.file_name().to_string_lossy().into_owned()
        };
        let display_name = read_display_name(&path).unwrap_or_else(|| id.clone());
        builds.push(ProtonBuild { id, display_name });
    }
    Ok(builds)
}

/// Official Steam-installed Proton (`Proton 9.0`, `Proton - Experimental`,
/// etc.) under `<steam-root>/steamapps/common`. Recognized by a `proton`
/// script directly inside the folder — the same executable umu-run/Steam
/// itself invokes — which is also what rules out an incomplete/pending
/// download (confirmed on this machine: a `Proton 10.0` folder existed with
/// no `proton` script at all, just Steam's own bookkeeping files, and is
/// correctly skipped here).
fn scan_official_proton(common_dir: &Path) -> Result<Vec<ProtonBuild>> {
    if !common_dir.exists() {
        return Ok(Vec::new());
    }

    let mut builds = Vec::new();
    for entry in
        fs::read_dir(common_dir).with_context(|| format!("reading {}", common_dir.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("Proton") || !path.join("proton").is_file() {
            continue;
        }
        builds.push(ProtonBuild {
            id: path.to_string_lossy().into_owned(),
            display_name: name,
        });
    }
    Ok(builds)
}

/// Extracts `"display_name" "..."` from `compatibilitytool.vdf` with a
/// simple text search rather than a full VDF/KeyValues parser — the file is
/// a small, predictably-formatted manifest, not arbitrary input.
fn read_display_name(build_dir: &Path) -> Option<String> {
    let text = fs::read_to_string(build_dir.join("compatibilitytool.vdf")).ok()?;
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix("\"display_name\"") {
            return Some(rest.trim().trim_matches('"').to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_display_name_from_real_manifest_format() {
        let dir =
            std::env::temp_dir().join(format!("iprolaunch-proton-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("compatibilitytool.vdf"),
            "\"compatibilitytools\"\n{\n  \"compat_tools\"\n  {\n    \"GE-Proton10-34\"\n    {\n      \"install_path\" \".\"\n      \"display_name\" \"GE-Proton10-34\"\n      \"from_oslist\"  \"windows\"\n      \"to_oslist\"    \"linux\"\n    }\n  }\n}\n",
        )
        .unwrap();

        assert_eq!(read_display_name(&dir), Some("GE-Proton10-34".to_string()));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn steam_library_paths_extracts_every_path_from_real_manifest_format() {
        let dir = std::env::temp_dir().join(format!(
            "iprolaunch-proton-library-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(dir.join("steamapps")).unwrap();
        fs::write(
            dir.join("steamapps/libraryfolders.vdf"),
            "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"/home/user/.local/share/Steam\"\n\t\t\"label\"\t\t\"\"\n\t\t\"apps\"\n\t\t{\n\t\t\t\"228980\"\t\t\"451331579\"\n\t\t}\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"/media/game/SteamLibrary\"\n\t\t\"label\"\t\t\"Viper 1TB\"\n\t}\n}\n",
        )
        .unwrap();

        assert_eq!(
            steam_library_paths(&dir),
            vec![
                PathBuf::from("/home/user/.local/share/Steam"),
                PathBuf::from("/media/game/SteamLibrary"),
            ]
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn steam_library_paths_is_empty_without_a_manifest() {
        assert_eq!(
            steam_library_paths(Path::new("/nonexistent-iprolaunch-test-path")),
            Vec::<PathBuf>::new()
        );
    }

    #[test]
    fn missing_manifest_returns_none() {
        assert_eq!(read_display_name(Path::new("/nonexistent")), None);
    }

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "iprolaunch-proton-test-{name}-{}",
            std::process::id()
        ))
    }

    #[test]
    fn official_proton_needs_both_the_name_prefix_and_a_proton_script() {
        let common = temp_dir("official-common");
        fs::create_dir_all(common.join("Proton 9.0")).unwrap();
        fs::write(common.join("Proton 9.0").join("proton"), "#!/bin/sh\n").unwrap();
        // A same-named but incomplete/pending download: no `proton` script.
        fs::create_dir_all(common.join("Proton 10.0")).unwrap();
        // An unrelated app that isn't a Proton build at all.
        fs::create_dir_all(common.join("Some Other Game")).unwrap();
        fs::write(common.join("Some Other Game").join("proton"), "decoy").unwrap();

        let builds = scan_official_proton(&common).unwrap();
        assert_eq!(builds.len(), 1);
        assert_eq!(builds[0].display_name, "Proton 9.0");
        assert_eq!(
            builds[0].id,
            common.join("Proton 9.0").to_string_lossy().into_owned()
        );

        fs::remove_dir_all(&common).ok();
    }

    #[test]
    fn official_proton_missing_common_dir_returns_empty_not_error() {
        assert_eq!(
            scan_official_proton(Path::new("/nonexistent")).unwrap(),
            Vec::new()
        );
    }

    #[test]
    fn resolve_binary_dir_passes_an_absolute_id_through_unchanged() {
        let dir =
            resolve_binary_dir("/usr/share/steam/compatibilitytools.d/proton-cachyos-slr").unwrap();
        assert_eq!(
            dir,
            Path::new("/usr/share/steam/compatibilitytools.d/proton-cachyos-slr")
        );
    }

    #[test]
    fn resolve_binary_dir_joins_a_bare_name_against_the_users_steam_root() {
        let dir = resolve_binary_dir("GE-Proton10-34").unwrap();
        assert!(
            dir.ends_with(".local/share/Steam/compatibilitytools.d/GE-Proton10-34"),
            "got {}",
            dir.display()
        );
    }

    #[test]
    fn resolve_binary_dir_rejects_system_and_empty() {
        assert!(resolve_binary_dir("system").is_err());
        assert!(resolve_binary_dir("").is_err());
    }

    #[test]
    fn find_on_path_locates_a_file_in_one_of_several_search_dirs() {
        let dir = temp_dir("find-on-path-hit");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("wine"), "").unwrap();
        let path_var = std::env::join_paths([Path::new("/nonexistent"), dir.as_path()]).unwrap();

        assert_eq!(find_on_path(&path_var, "wine"), Some(dir.join("wine")));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn find_on_path_returns_none_when_absent_from_every_search_dir() {
        let path_var = std::env::join_paths([Path::new("/nonexistent")]).unwrap();
        assert_eq!(find_on_path(&path_var, "wine"), None);
    }

    /// Regression test for a real failure: a build found under a
    /// system-wide `compatibilitytools.d` (CachyOS's `proton-cachyos-slr`)
    /// was stored as just its bare folder name, which `umu-run` can't
    /// resolve (it only expands a relative `PROTONPATH` against the user's
    /// own Steam root, never a system dir) — real error was `PROTONPATH
    /// 'proton-cachyos-slr' is not valid, toolmanifest.vdf not found`.
    #[test]
    fn absolute_id_uses_the_full_path_not_just_the_folder_name() {
        let dir = temp_dir("system-compat");
        fs::create_dir_all(dir.join("proton-cachyos-slr")).unwrap();
        fs::write(dir.join("proton-cachyos-slr").join("toolmanifest.vdf"), "").unwrap();

        let builds = scan_compatibilitytools(&dir, true).unwrap();
        assert_eq!(builds.len(), 1);
        assert_eq!(
            builds[0].id,
            dir.join("proton-cachyos-slr")
                .to_string_lossy()
                .into_owned()
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn non_absolute_id_uses_just_the_folder_name() {
        let dir = temp_dir("user-compat");
        fs::create_dir_all(dir.join("GE-Proton10-34")).unwrap();
        fs::write(dir.join("GE-Proton10-34").join("toolmanifest.vdf"), "").unwrap();

        let builds = scan_compatibilitytools(&dir, false).unwrap();
        assert_eq!(builds.len(), 1);
        assert_eq!(builds[0].id, "GE-Proton10-34");

        fs::remove_dir_all(&dir).ok();
    }
}
