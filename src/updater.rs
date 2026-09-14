//! Manually-triggered self-update — Config tab's Desktop integration table,
//! "check for update" row. Checks GitHub's real `/releases/latest` API,
//! and if newer, downloads and swaps in the new binary.
//!
//! Safe to run while this exact binary is the running process: never
//! writes to the target path directly (which would corrupt this process's
//! own mapped executable pages) — downloads and extracts into a staging
//! directory *next to* the real binary (same filesystem, required for the
//! final swap to be atomic — `rename()` fails outright across filesystems,
//! which a system-wide temp dir like `/tmp` often is), then a single
//! `rename()` swaps the new binary into place. The already-running process
//! keeps executing its old in-memory image regardless — same as replacing
//! any other running program's binary on Linux — hence the "restart to
//! apply" prompt this leaves for the caller to show.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

const REPO: &str = "krakerz/IProLaunch";

#[derive(Debug, Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheck {
    UpToDate {
        current: String,
    },
    Available {
        current: String,
        latest: String,
        asset_name: String,
        asset_url: String,
    },
}

/// GitHub's API 403s without a `User-Agent` — anything identifiable works;
/// this project's own name/version, so a real report shows which build
/// made the request if it's ever worth debugging.
fn user_agent() -> String {
    format!("iprolaunch/{}", env!("CARGO_PKG_VERSION"))
}

/// Parses a `MAJOR.MINOR.PATCH`-shaped version (a leading `v` stripped
/// first, so a GitHub tag like `v1.31.1` compares directly against
/// `CARGO_PKG_VERSION`) into a tuple for ordering. Not a general semver
/// parser — this project's own versions are always exactly this shape (see
/// the versioning convention every release so far has followed), so a
/// small hand-rolled parse avoids a real semver crate for a comparison
/// this simple.
fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let v = v.strip_prefix('v').unwrap_or(v);
    let mut parts = v.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    Some((major, minor, patch))
}

/// Queries GitHub's `/releases/latest` — deliberately never `/releases`
/// (every one, including drafts/prereleases): `/latest` only ever returns
/// a *published*, non-prerelease release, and this project's own CI
/// creates every release as a draft until a human publishes it from the
/// GitHub UI — so an unreviewed build can never get offered here.
pub fn check_for_update() -> Result<UpdateCheck> {
    let current = env!("CARGO_PKG_VERSION").to_string();
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let release: Release = ureq::get(&url)
        .header("User-Agent", user_agent())
        .call()
        .context("checking GitHub for the latest release")?
        .body_mut()
        .read_json()
        .context("parsing GitHub's release response")?;

    let current_parsed = parse_version(&current)
        .with_context(|| format!("couldn't parse this build's own version {current:?}"))?;
    let latest_parsed = parse_version(&release.tag_name).with_context(|| {
        format!(
            "couldn't parse the latest release's tag {:?}",
            release.tag_name
        )
    })?;

    if latest_parsed <= current_parsed {
        return Ok(UpdateCheck::UpToDate { current });
    }

    let asset = release
        .assets
        .iter()
        .find(|a| a.name.ends_with(".tar.gz"))
        .with_context(|| format!("release {} has no .tar.gz asset", release.tag_name))?;

    Ok(UpdateCheck::Available {
        current,
        latest: release.tag_name,
        asset_name: asset.name.clone(),
        asset_url: asset.browser_download_url.clone(),
    })
}

fn download(url: &str, dest: &Path) -> Result<()> {
    let mut response = ureq::get(url)
        .header("User-Agent", user_agent())
        .call()
        .with_context(|| format!("downloading {url}"))?;
    let mut file =
        fs::File::create(dest).with_context(|| format!("creating {}", dest.display()))?;
    io::copy(&mut response.body_mut().as_reader(), &mut file)
        .with_context(|| format!("writing {}", dest.display()))?;
    Ok(())
}

/// Finds a file named exactly `iprolaunch` somewhere under `dir` — the
/// release bundle's own top-level folder name (`iprolaunch-v<version>/`)
/// isn't assumed exactly, just that the binary is in there somewhere no
/// more than a couple of levels deep, matching how shallow the actual
/// bundle (see `.github/workflows/build.yml`'s assembly step) really is.
fn find_binary(dir: &Path) -> Option<PathBuf> {
    fn walk(dir: &Path, depth: u32) -> Option<PathBuf> {
        if depth == 0 {
            return None;
        }
        for entry in fs::read_dir(dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_file() && path.file_name().is_some_and(|n| n == "iprolaunch") {
                return Some(path);
            }
            if path.is_dir()
                && let Some(found) = walk(&path, depth - 1)
            {
                return Some(found);
            }
        }
        None
    }
    walk(dir, 3)
}

/// Downloads and installs `asset_url` over this exact running binary,
/// printing progress as it goes (see `tui::config::run_integrate_action`
/// for the same suspend-and-print pattern this is meant to be called
/// alongside). See this module's own doc comment for why the swap itself
/// is safe to do while running.
pub fn apply_update(asset_url: &str) -> Result<()> {
    let target = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .context("resolving iprolaunch's own binary path")?;
    let target_dir = target
        .parent()
        .context("iprolaunch's own binary path has no parent directory")?;

    let staging_dir = target_dir.join(".iprolaunch-update-tmp");
    if staging_dir.exists() {
        // Leftover from an interrupted previous attempt — start clean.
        fs::remove_dir_all(&staging_dir).ok();
    }
    fs::create_dir(&staging_dir).with_context(|| format!("creating {}", staging_dir.display()))?;

    println!("Downloading {asset_url} ...");
    let archive_path = staging_dir.join("update.tar.gz");
    let result = download(asset_url, &archive_path).and_then(|()| {
        println!("Extracting ...");
        let status = Command::new("tar")
            .arg("-xzf")
            .arg(&archive_path)
            .arg("-C")
            .arg(&staging_dir)
            .status()
            .context("running tar")?;
        if !status.success() {
            bail!("tar exited with {status}");
        }

        let new_binary = find_binary(&staging_dir)
            .context("couldn't find the iprolaunch binary inside the downloaded archive")?;
        let mut perms = fs::metadata(&new_binary)?.permissions();
        perms.set_mode(perms.mode() | 0o111);
        fs::set_permissions(&new_binary, perms)?;

        // A same-directory swap file, not the new binary's own staged
        // path directly — keeps the final rename a plain same-filesystem
        // move regardless of where exactly `find_binary` located it
        // inside the (potentially nested) extracted archive.
        let swap_path = target_dir.join(".iprolaunch-update-swap");
        fs::rename(&new_binary, &swap_path).context("staging the new binary")?;

        println!("Installing ...");
        fs::rename(&swap_path, &target).context("swapping the new binary into place")?;
        Ok(())
    });

    fs::remove_dir_all(&staging_dir).ok();
    result?;
    println!("Updated. Restart iprolaunch to run the new version.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_version_strips_a_leading_v() {
        assert_eq!(parse_version("v1.31.1"), Some((1, 31, 1)));
        assert_eq!(parse_version("1.31.1"), Some((1, 31, 1)));
    }

    #[test]
    fn parse_version_rejects_anything_else() {
        assert_eq!(parse_version("not-a-version"), None);
        assert_eq!(parse_version("1.31"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn find_binary_locates_it_inside_a_nested_bundle_folder() {
        let dir =
            std::env::temp_dir().join(format!("iprolaunch-updater-test-{}", std::process::id()));
        let bundle = dir.join("iprolaunch-v1.31.1");
        fs::create_dir_all(&bundle).unwrap();
        fs::write(bundle.join("iprolaunch"), b"fake binary").unwrap();
        fs::write(bundle.join("README.txt"), b"not it").unwrap();

        assert_eq!(find_binary(&dir), Some(bundle.join("iprolaunch")));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn find_binary_is_none_when_nothing_matches() {
        let dir = std::env::temp_dir().join(format!(
            "iprolaunch-updater-test-empty-{}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("README.txt"), b"nope").unwrap();

        assert_eq!(find_binary(&dir), None);
        fs::remove_dir_all(&dir).ok();
    }
}
