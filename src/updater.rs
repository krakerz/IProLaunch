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
//!
//! Trust model: GitHub's sha256 digest only proves the transfer was intact.
//! The archive must also carry a valid Ed25519 signature (`<archive>.sig`,
//! made by the release workflow) against the public key embedded below.

use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;
use std::time::Duration;

const REPO: &str = "krakerz/IProLaunch";

/// Largest archive the updater will accept; the real bundle is a few MiB.
const MAX_DOWNLOAD_BYTES: u64 = 256 * 1024 * 1024;

/// Release-signing public key (base64 of the raw 32-byte Ed25519 key).
const RELEASE_PUBLIC_KEY_B64: &str = "SxxPrzXqNCKENC5tmCheVQKgZrUK0l/AIoD4kPjZDHA=";

/// A raw Ed25519 signature is exactly 64 bytes.
const SIGNATURE_LEN: usize = 64;

/// The signature asset is tiny; anything bigger is not one.
const MAX_SIGNATURE_BYTES: u64 = 4 * 1024;

/// One agent for this module: https only (redirects included), bounded connect and header waits.
fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .https_only(true)
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .build()
            .new_agent()
    })
}

/// This binary's own path. After `apply_update` renames a new build over the
/// running one, `/proc/self/exe` reads `<path> (deleted)` — the new build sits
/// at the original path, so that's what generated launchers must point at.
pub(crate) fn own_exe() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("resolving iprolaunch's own binary path")?;
    Ok(strip_deleted_suffix(exe))
}

fn strip_deleted_suffix(exe: PathBuf) -> PathBuf {
    match exe.to_str().and_then(|s| s.strip_suffix(" (deleted)")) {
        Some(original) => PathBuf::from(original),
        None => exe,
    }
}

#[derive(Debug, Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    digest: Option<String>,
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
        digest: Option<String>,
        signature_url: Option<String>,
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
    let release: Release = agent()
        .get(&url)
        .config()
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
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

    let wanted = asset_name_for_tag(&release.tag_name);
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == wanted)
        .with_context(|| format!("release {} has no {wanted} asset", release.tag_name))?;

    let signature_url = find_signature_url(&release.assets, &wanted);

    Ok(UpdateCheck::Available {
        current,
        latest: release.tag_name,
        asset_name: asset.name.clone(),
        asset_url: asset.browser_download_url.clone(),
        digest: asset.digest.clone(),
        signature_url,
    })
}

/// The download URL of the asset named exactly `<archive_name>.sig`, if the release has one.
fn find_signature_url(assets: &[ReleaseAsset], archive_name: &str) -> Option<String> {
    let wanted = format!("{archive_name}.sig");
    assets
        .iter()
        .find(|a| a.name == wanted)
        .map(|a| a.browser_download_url.clone())
}

fn release_public_key() -> Result<Vec<u8>> {
    let key = BASE64
        .decode(RELEASE_PUBLIC_KEY_B64)
        .context("decoding the embedded release public key")?;
    if key.len() != 32 {
        bail!("embedded release public key is {} bytes, not 32", key.len());
    }
    Ok(key)
}

fn verify_release_signature(public_key: &[u8], message: &[u8], signature: &[u8]) -> Result<()> {
    if signature.len() != SIGNATURE_LEN {
        bail!(
            "signature is {} bytes, not {SIGNATURE_LEN}",
            signature.len()
        );
    }
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, public_key)
        .verify(message, signature)
        .map_err(|_| anyhow::anyhow!("signature does not match"))
}

/// Downloads the small signature asset into memory, capped at `MAX_SIGNATURE_BYTES`.
fn download_signature(url: &str) -> Result<Vec<u8>> {
    let mut response = agent()
        .get(url)
        .config()
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .header("User-Agent", user_agent())
        .call()
        .with_context(|| format!("downloading {url}"))?;
    let mut bytes = Vec::new();
    response
        .body_mut()
        .with_config()
        .limit(MAX_SIGNATURE_BYTES)
        .reader()
        .read_to_end(&mut bytes)
        .context("reading the release signature")?;
    Ok(bytes)
}

/// The archive name `.github/workflows/build.yml` publishes for a release tag.
fn asset_name_for_tag(tag: &str) -> String {
    let tag = tag.strip_prefix('v').unwrap_or(tag);
    format!("iprolaunch-v{tag}.tar.gz")
}

fn is_release_download_url(url: &str) -> bool {
    url.starts_with(&format!("https://github.com/{REPO}/releases/download/"))
}

/// The expected SHA-256 (lowercase hex) from GitHub's `sha256:<hex>` digest, if it is one.
fn parse_sha256_digest(digest: &str) -> Option<String> {
    let hex = digest.strip_prefix("sha256:")?;
    (hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| hex.to_ascii_lowercase())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .with_context(|| format!("reading {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn download(url: &str, dest: &Path) -> Result<()> {
    let mut response = agent()
        .get(url)
        .config()
        .timeout_global(Some(Duration::from_secs(15 * 60)))
        .build()
        .header("User-Agent", user_agent())
        .call()
        .with_context(|| format!("downloading {url}"))?;
    let mut file =
        fs::File::create(dest).with_context(|| format!("creating {}", dest.display()))?;
    // Hitting the cap surfaces as a read error, never a silent truncation.
    let mut reader = response
        .body_mut()
        .with_config()
        .limit(MAX_DOWNLOAD_BYTES)
        .reader();
    io::copy(&mut reader, &mut file).with_context(|| {
        format!(
            "writing {} (download failed or exceeded {} MiB)",
            dest.display(),
            MAX_DOWNLOAD_BYTES / (1024 * 1024)
        )
    })?;
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
            // `file_type()` doesn't follow symlinks: a link named `iprolaunch` is not the binary.
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_file() && path.file_name().is_some_and(|n| n == "iprolaunch") {
                return Some(path);
            }
            if file_type.is_dir()
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
pub fn apply_update(
    asset_url: &str,
    digest: Option<&str>,
    signature_url: Option<&str>,
) -> Result<()> {
    if !is_release_download_url(asset_url) {
        bail!("refusing to download from an unexpected URL: {asset_url}");
    }
    let Some(signature_url) = signature_url else {
        bail!("release has no signature (<name>.sig); refusing to update");
    };
    if !is_release_download_url(signature_url) {
        bail!("refusing to download from an unexpected URL: {signature_url}");
    }
    let target = own_exe()?
        .canonicalize()
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
        match digest.and_then(parse_sha256_digest) {
            Some(expected) => {
                let actual = sha256_file(&archive_path)?;
                if actual != expected {
                    bail!("checksum mismatch: expected sha256 {expected}, got {actual}");
                }
                println!("Checksum verified.");
            }
            None => {
                println!("Could not verify the download's checksum (GitHub gave no sha256 digest).")
            }
        }

        println!("Verifying signature ...");
        let signature = download_signature(signature_url)?;
        let archive = fs::read(&archive_path)
            .with_context(|| format!("reading {}", archive_path.display()))?;
        verify_release_signature(&release_public_key()?, &archive, &signature)
            .context("signature verification failed: the download is not an official release")?;
        println!("Signature verified.");

        println!("Extracting ...");
        let status = Command::new("tar")
            .arg("--no-same-owner")
            .arg("--no-same-permissions")
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
        if let Err(err) = fs::rename(&swap_path, &target) {
            fs::remove_file(&swap_path).ok();
            return Err(err).context("swapping the new binary into place");
        }
        Ok(())
    });

    fs::remove_dir_all(&staging_dir).ok();
    result?;
    crate::vdesktop::restart_after_update();
    println!("Updated. Restart iprolaunch to run the new version.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_deleted_suffix_recovers_the_original_path_after_a_self_update() {
        assert_eq!(
            strip_deleted_suffix(PathBuf::from("/home/u/My Apps/iprolaunch (deleted)")),
            PathBuf::from("/home/u/My Apps/iprolaunch")
        );
        assert_eq!(
            strip_deleted_suffix(PathBuf::from("/usr/bin/iprolaunch")),
            PathBuf::from("/usr/bin/iprolaunch")
        );
    }

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

    const SELF_TEST_MSG: &[u8] = b"iprolaunch release signing self-test";
    const SELF_TEST_SIG_B64: &str =
        "JGv6VT01JUr/bXhyzdA31F5YG7iJthFXXnTilhnroCLzDvl3VwOcyLK1WE0htG6eE/7k0Nw+rM4Bi2IJsHG7AA==";

    #[test]
    fn embedded_public_key_is_32_bytes() {
        assert_eq!(release_public_key().unwrap().len(), 32);
    }

    #[test]
    fn openssl_signature_verifies_with_the_embedded_key() {
        let key = release_public_key().unwrap();
        let sig = BASE64.decode(SELF_TEST_SIG_B64).unwrap();
        assert!(verify_release_signature(&key, SELF_TEST_MSG, &sig).is_ok());

        let mut tampered = SELF_TEST_MSG.to_vec();
        tampered[0] ^= 1;
        assert!(verify_release_signature(&key, &tampered, &sig).is_err());
        assert!(verify_release_signature(&key, SELF_TEST_MSG, &sig[..63]).is_err());
    }

    #[test]
    fn signature_asset_is_matched_by_exact_name() {
        let asset = |name: &str| ReleaseAsset {
            name: name.to_string(),
            browser_download_url: format!("https://example.invalid/{name}"),
            digest: None,
        };
        let assets = vec![
            asset("iprolaunch-v1.48.0.tar.gz"),
            asset("x-iprolaunch-v1.48.0.tar.gz.sig"),
            asset("iprolaunch-v1.48.0.tar.gz.sig"),
        ];
        assert_eq!(
            find_signature_url(&assets, "iprolaunch-v1.48.0.tar.gz").as_deref(),
            Some("https://example.invalid/iprolaunch-v1.48.0.tar.gz.sig")
        );
        assert_eq!(
            find_signature_url(&assets[..2], "iprolaunch-v1.48.0.tar.gz"),
            None
        );
    }

    #[test]
    fn asset_name_is_derived_from_the_tag() {
        assert_eq!(asset_name_for_tag("v1.31.1"), "iprolaunch-v1.31.1.tar.gz");
        assert_eq!(asset_name_for_tag("1.31.1"), "iprolaunch-v1.31.1.tar.gz");
    }

    #[test]
    fn download_url_must_be_this_repos_release_asset() {
        let ok =
            format!("https://github.com/{REPO}/releases/download/v1.0.0/iprolaunch-v1.0.0.tar.gz");
        assert!(is_release_download_url(&ok));
        assert!(!is_release_download_url(
            "http://github.com/krakerz/IProLaunch/releases/download/x"
        ));
        assert!(!is_release_download_url(
            "https://evil.example/krakerz/IProLaunch/releases/download/x"
        ));
        assert!(!is_release_download_url(
            "https://github.com/other/repo/releases/download/x"
        ));
    }

    #[test]
    fn digest_parsing_accepts_only_sha256_hex() {
        let hex = "A".repeat(64);
        assert_eq!(
            parse_sha256_digest(&format!("sha256:{hex}")),
            Some("a".repeat(64))
        );
        assert_eq!(parse_sha256_digest(&format!("sha512:{hex}")), None);
        assert_eq!(parse_sha256_digest("sha256:abc"), None);
        assert_eq!(
            parse_sha256_digest(&format!("sha256:{}", "z".repeat(64))),
            None
        );
    }

    #[test]
    fn sha256_file_matches_a_known_digest() {
        let dir =
            std::env::temp_dir().join(format!("iprolaunch-updater-sha-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("f");
        fs::write(&file, b"abc").unwrap();
        assert_eq!(
            sha256_file(&file).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn find_binary_ignores_a_symlink_named_iprolaunch() {
        let dir = std::env::temp_dir().join(format!(
            "iprolaunch-updater-test-link-{}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("elsewhere"), b"target").unwrap();
        std::os::unix::fs::symlink(dir.join("elsewhere"), dir.join("iprolaunch")).unwrap();

        assert_eq!(find_binary(&dir), None);
        fs::remove_dir_all(&dir).ok();
    }
}
