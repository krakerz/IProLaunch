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
fn steam_roots() -> Result<Vec<PathBuf>> {
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

/// Scans every known Steam root for both community Proton builds
/// (`compatibilitytools.d`) and official Valve-shipped ones
/// (`steamapps/common/Proton*`), plus any system-wide compat-tool
/// directory — this is the one scan used everywhere a Proton build list is
/// needed (the `proton list` CLI command and the TUI's proton picker
/// alike), so improving it here improves both.
pub fn scan() -> Result<Vec<ProtonBuild>> {
    let mut builds = Vec::new();
    for root in steam_roots()? {
        builds.extend(
            scan_compatibilitytools(&root.join("compatibilitytools.d"), false).unwrap_or_default(),
        );
        builds.extend(scan_official_proton(&root.join("steamapps/common")).unwrap_or_default());
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
