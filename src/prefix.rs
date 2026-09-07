use std::path::{Path, PathBuf};

use crate::config::{Effective, PrefixMode, expand_home};

/// Lowercases, collapses non-alphanumerics to `-`, and trims leading/
/// trailing `-` — the sanitization rule for anything used as a folder
/// slug, whether derived from an exe path (`slug_from_exe`) or typed
/// directly by the user (`tui::profile_editor`'s slug rename).
pub fn sanitize(text: &str) -> String {
    let mut s: String = text
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    s.trim_matches('-').to_string()
}

/// Derives a *starting-point* slug from an exe path (lowercased stem,
/// non-alphanumerics collapsed to `-`) for a brand-new profile —
/// `launch::ensure_profile` disambiguates it against existing profiles
/// (`game`, `game-2`, ...) before it's ever used as a real folder name.
/// Pure derivation, not disk lookup.
pub fn slug_from_exe(target: &Path) -> String {
    let stem = target.file_stem().and_then(|s| s.to_str()).unwrap_or("app");
    sanitize(stem)
}

/// Resolves the prefix directory for one launch. `PrefixMode::PerSlug` uses
/// the profile's own (already-disambiguated) `slug` directly — deliberately
/// *not* re-derived from the exe path here, unlike an earlier version of
/// this function: two different exes that happen to share a file stem
/// (e.g. `a/game.exe`, `b/game.exe`) get distinct profiles (`game`,
/// `game-2`) via `ensure_profile`'s own disambiguation, and using the
/// profile's real slug here means they now also get distinct prefixes
/// instead of silently sharing one (confirmed as a real gap in the
/// previous version — see project NOTES.md, 2026-09-06). This is also
/// exactly why `tui::profile_editor`'s slug rename can — and, in
/// `PerSlug` mode, must — rename the prefix directory to match: this
/// function will look for the prefix under whatever the slug *currently*
/// is, not wherever it used to be.
pub fn resolve(effective: &Effective, slug: &str) -> PathBuf {
    match effective.prefix_mode {
        PrefixMode::Single => expand_home(&effective.prefix_path),
        PrefixMode::PerSlug => expand_home(&effective.prefixes_root).join(slug),
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

    #[test]
    fn per_slug_mode_uses_the_given_slug_directly_not_the_exe_path() {
        let effective = crate::config::Effective {
            proton: "system".to_string(),
            prefix_mode: PrefixMode::PerSlug,
            prefix_path: "~/unused".to_string(),
            prefixes_root: "~/prefixes".to_string(),
            windows_version: None,
            gamescope: Default::default(),
            gamescope_settings: Default::default(),
            launch_wrapper: None,
            log_mode: crate::config::LogMode::Single,
            keep: 0,
            record: crate::config::RecordMode::Off,
            auto_open: false,
            env: Default::default(),
            winedlloverride: Default::default(),
        };
        // Two different exes sharing a file stem still get distinct
        // prefixes as long as the caller passes their own distinct slugs
        // (`launch::ensure_profile`'s job, not this function's) — this is
        // the actual fix for the collision gap: `resolve` no longer
        // re-derives anything from the exe path itself.
        assert_eq!(
            resolve(&effective, "game"),
            expand_home("~/prefixes").join("game")
        );
        assert_eq!(
            resolve(&effective, "game-2"),
            expand_home("~/prefixes").join("game-2")
        );
    }
}
