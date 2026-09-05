use std::path::{Path, PathBuf};

use crate::config::{Effective, PrefixMode, expand_home};

/// Derives the folder slug used for both per-exe prefixes and profile storage:
/// lowercased exe stem, non-alphanumerics collapsed to `-`. Collision handling
/// (two different exes stemming to the same slug) is the caller's job — this
/// is pure derivation, not disk lookup.
pub fn slug_from_exe(target: &Path) -> String {
    let stem = target.file_stem().and_then(|s| s.to_str()).unwrap_or("app");
    let mut slug: String = stem
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }
    slug.trim_matches('-').to_string()
}

pub fn resolve(effective: &Effective, target: &Path) -> PathBuf {
    match effective.prefix_mode {
        PrefixMode::Single => expand_home(&effective.prefix_path),
        PrefixMode::PerExe => expand_home(&effective.prefixes_root).join(slug_from_exe(target)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_sanitizes_and_lowercases() {
        assert_eq!(
            slug_from_exe(Path::new("/games/Elden Ring/eldenring.exe")),
            "eldenring"
        );
        assert_eq!(slug_from_exe(Path::new("C:/Foo Bar!!.exe")), "foo-bar");
    }
}
