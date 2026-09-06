use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::UserDirs;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtonBuild {
    /// Folder name under `compatibilitytools.d` — this is what gets stored
    /// as `defaults.proton` (or a profile override) and passed straight
    /// through as `PROTONPATH`, which `man umu` documents as accepting a
    /// path, a version name, or a codename; a folder name here is a valid
    /// version name.
    pub id: String,
    pub display_name: String,
}

fn compatibilitytools_dir() -> Result<PathBuf> {
    let home = UserDirs::new().context("could not determine home directory")?;
    Ok(home
        .home_dir()
        .join(".local/share/Steam/compatibilitytools.d"))
}

/// Scans `~/.local/share/Steam/compatibilitytools.d` for installed
/// community Proton builds (GE-Proton, CachyOS Proton, umu's own
/// auto-downloaded UMU-Proton, etc). A directory only counts if it has a
/// `toolmanifest.vdf` — the same file Steam itself uses to recognize a
/// compatibility tool; confirmed present in every real build and absent
/// from anything else on this machine.
///
/// Doesn't cover Proton versions Steam installs directly under
/// `steamapps/common/` (official "Proton 9.0", "Proton - Experimental",
/// etc.) — on this machine those had no `proton` script or manifest files at
/// all (an incomplete/pending Steam-side install state), so there was
/// nothing reliable to detect there. Revisit if that turns out to be the
/// common case elsewhere.
pub fn scan() -> Result<Vec<ProtonBuild>> {
    let dir = compatibilitytools_dir()?;
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut builds = Vec::new();
    for entry in fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path();
        if !path.join("toolmanifest.vdf").is_file() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        let display_name = read_display_name(&path).unwrap_or_else(|| id.clone());
        builds.push(ProtonBuild { id, display_name });
    }
    builds.sort_by(|a, b| {
        a.display_name
            .to_lowercase()
            .cmp(&b.display_name.to_lowercase())
    });
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
}
