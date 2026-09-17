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
///
/// Falls back to `"app"` (same as when there's no stem at all) when
/// `sanitize` reduces the stem to nothing — a real reported case: `.exe`
/// stems made up entirely of non-ASCII characters (e.g. a Japanese title)
/// sanitize down to an empty string, since only ASCII alphanumerics survive.
/// Left unguarded, `ensure_profile` would call `Profile::save("")`, which
/// joins onto `profiles_dir()` itself — a `profile.toml` written directly
/// into the profiles *root*, invisible to `Profile::load_all()` (which only
/// descends into subdirectories) and silently clobbered by the next
/// same-stem exe with the same fate, rather than living in its own folder
/// like every other profile.
pub fn slug_from_exe(target: &Path) -> String {
    let stem = target.file_stem().and_then(|s| s.to_str()).unwrap_or("app");
    let sanitized = sanitize(stem);
    if sanitized.is_empty() {
        "app".to_string()
    } else {
        sanitized
    }
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
    fn slug_from_exe_falls_back_to_app_when_the_stem_is_entirely_non_ascii() {
        // Real reported case: a stem made up entirely of non-ASCII
        // characters (e.g. a Japanese title) sanitizes down to nothing —
        // only ASCII alphanumerics survive `sanitize` — which used to slip
        // through unguarded into `ensure_profile` calling
        // `Profile::save("")`, writing a `profile.toml` directly into the
        // profiles *root* instead of its own folder.
        assert_eq!(slug_from_exe(Path::new("/games/真・痴漢の極み.exe")), "app");
    }

    #[test]
    fn per_slug_mode_uses_the_given_slug_directly_not_the_exe_path() {
        let effective = crate::config::Effective {
            proton: "system".to_string(),
            prefix_mode: PrefixMode::PerSlug,
            prefix_path: "~/unused".to_string(),
            prefixes_root: "~/prefixes".to_string(),
            windows_version: crate::config::WindowsVersion::Win10,
            gamescope: Default::default(),
            gamescope_settings: Default::default(),
            launch_wrapper: None,
            log_mode: crate::config::LogMode::Single,
            keep: 0,
            record: crate::config::RecordMode::Off,
            auto_open: false,
            auto_open_scope: Default::default(),
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
