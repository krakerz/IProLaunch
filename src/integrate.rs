use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Windows PE executables are already recognized under these on a stock
/// Linux system (shared-mime-info sniffs the "MZ" header, not the `.exe`
/// extension) — we only need to register as the *default handler*, not
/// teach the OS a new file type.
const MIME_TYPES: [&str; 2] = [
    "application/x-msdownload",
    "application/x-ms-dos-executable",
];
const DESKTOP_FILE_NAME: &str = "iprolaunch.desktop";

fn base_dirs() -> Result<directories::BaseDirs> {
    directories::BaseDirs::new().context("could not determine home directory")
}

/// `~/.local/share/applications` — the shared, non-app-specific desktop
/// entry directory (not `~/.local/share/iprolaunch/...`, which is what
/// `config::project_dirs()` would give us).
fn applications_dir() -> Result<PathBuf> {
    Ok(base_dirs()?.data_dir().join("applications"))
}

fn desktop_file_path() -> Result<PathBuf> {
    Ok(applications_dir()?.join(DESKTOP_FILE_NAME))
}

fn mimeapps_list_path() -> Result<PathBuf> {
    Ok(base_dirs()?.config_dir().join("mimeapps.list"))
}

/// Where `install` records whichever app was the default *before* it ran,
/// per mimetype — so `uninstall` can put it back. Lives under our own
/// app-specific config dir (not the shared `mimeapps.list`/`applications`
/// locations above), since it's purely our own bookkeeping.
fn backup_path() -> Result<PathBuf> {
    Ok(crate::config::project_dirs()?
        .config_dir()
        .join("integrate-backup.json"))
}

/// Whether IProLaunch's own desktop entry is currently present. A simple
/// file-existence check rather than an `xdg-mime query` round-trip — this
/// is used to render the TUI's toggle on every frame, so it needs to be
/// cheap, and "did *we* install it" is what the toggle actually means (not
/// "are we still definitely the live default", which something else could
/// have changed since).
pub fn is_installed() -> bool {
    desktop_file_path().map(|p| p.exists()).unwrap_or(false)
}

/// Registers `iprolaunch` as the default handler for Windows `.exe` files:
/// writes a `.desktop` file pointing at this exact binary, then uses
/// `xdg-mime default` (part of `xdg-utils`, standard on any desktop Linux —
/// this is a freedesktop.org mechanism, not KDE-specific, so it should work
/// on GNOME/XFCE too) to register it. Launches are detached (`Terminal=false`)
/// by design — see `launch::open_in_pager`'s `IsTerminal` guard, added
/// specifically so a detached launch's failure path doesn't try to spawn a
/// pager with nothing to attach to.
///
/// Before changing anything, records whatever was the previous default for
/// each mimetype (if any) to `backup_path()` — but only if that file doesn't
/// already exist. Without that guard, running `install` a second time
/// without an `uninstall` in between would overwrite the real original with
/// "iprolaunch.desktop" itself (since by then *we're* the current default),
/// permanently losing what `uninstall` is supposed to restore.
pub fn install() -> Result<()> {
    let exe = std::env::current_exe().context("resolving iprolaunch's own binary path")?;
    let dir = applications_dir()?;
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    let backup_file = backup_path()?;
    if !backup_file.exists() {
        let prior = capture_prior_defaults();
        if !prior.is_empty() {
            write_backup(&backup_file, &prior)?;
        }
    }

    let desktop_path = desktop_file_path()?;
    let contents = desktop_file_contents(&exe);
    fs::write(&desktop_path, contents)
        .with_context(|| format!("writing {}", desktop_path.display()))?;

    for mime in MIME_TYPES {
        set_default(DESKTOP_FILE_NAME, mime)?;
    }

    refresh_desktop_database(&dir);

    println!("Installed {}", desktop_path.display());
    println!("Default handler for:");
    for mime in MIME_TYPES {
        println!("  {mime}");
    }
    println!(
        "Launches run detached (no terminal window) — see `iprolaunch running list` to check on one."
    );
    Ok(())
}

/// Removes the `.desktop` file and this specific desktop entry from
/// `mimeapps.list`'s associations — never wipes the whole file, so any
/// other app the user has associated with the same mimetype is left
/// untouched. Restores whatever was the default before `install` ran, per
/// the backup it wrote (if any), then deletes that backup file so the next
/// `install` starts fresh rather than restoring stale data a second time.
pub fn uninstall() -> Result<()> {
    let desktop_path = desktop_file_path()?;
    let removed_desktop = desktop_path.exists();
    if removed_desktop {
        fs::remove_file(&desktop_path)
            .with_context(|| format!("removing {}", desktop_path.display()))?;
    }

    let backup_file = backup_path()?;
    let restored = restore_prior_defaults(&backup_file)?;
    let cleared = remove_default_associations()?;
    let _ = fs::remove_file(&backup_file); // fresh slate for the next `install`

    let dir = applications_dir()?;
    refresh_desktop_database(&dir);

    if removed_desktop || restored || cleared {
        println!("Uninstalled.");
    } else {
        println!("Nothing to uninstall — it wasn't installed.");
    }
    Ok(())
}

fn set_default(desktop_file: &str, mime: &str) -> Result<()> {
    let status = Command::new("xdg-mime")
        .args(["default", desktop_file, mime])
        .status()
        .context("running xdg-mime (is xdg-utils installed?)")?;
    if !status.success() {
        bail!("`xdg-mime default {desktop_file} {mime}` exited with {status}");
    }
    Ok(())
}

fn query_current_default(mime: &str) -> Option<String> {
    let output = Command::new("xdg-mime")
        .args(["query", "default", mime])
        .output()
        .ok()?;
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// Queries the current default for each of our mimetypes, keeping only the
/// ones that actually have one set (nothing to restore for a mimetype that
/// had no prior default at all).
fn capture_prior_defaults() -> BTreeMap<String, String> {
    MIME_TYPES
        .iter()
        .filter_map(|mime| query_current_default(mime).map(|prior| (mime.to_string(), prior)))
        .collect()
}

fn write_backup(path: &Path, entries: &BTreeMap<String, String>) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(entries)?;
    fs::write(path, json).with_context(|| format!("writing {}", path.display()))
}

fn read_backup(path: &Path) -> BTreeMap<String, String> {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Re-applies whatever was recorded as each mimetype's prior default (via
/// `xdg-mime default`, same mechanism `install` uses to set ours — so this
/// is a real, standard registration, not a hand-rolled substitute). Mimetypes
/// with no backup entry (no prior default existed) are left for
/// `remove_default_associations` to clear instead.
fn restore_prior_defaults(backup_file: &Path) -> Result<bool> {
    let entries = read_backup(backup_file);
    let mut restored = false;
    for (mime, prior) in &entries {
        set_default(prior, mime)?;
        restored = true;
    }
    Ok(restored)
}

fn refresh_desktop_database(applications_dir: &Path) {
    // Best-effort: refreshes the mimetype/desktop-file index some DEs use
    // for menus/search. `xdg-mime default` itself doesn't depend on this.
    let _ = Command::new("update-desktop-database")
        .arg(applications_dir)
        .status();
}

fn remove_default_associations() -> Result<bool> {
    let path = mimeapps_list_path()?;
    let Ok(original) = fs::read_to_string(&path) else {
        return Ok(false);
    };
    let (new_content, changed) = strip_desktop_entry_from_mimeapps(&original);
    if changed {
        fs::write(&path, new_content).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(changed)
}

fn desktop_file_contents(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
Type=Application\n\
Name=IProLaunch\n\
Comment=Launch Windows apps/games through Proton via umu-run\n\
Exec=\"{}\" run %f\n\
Terminal=false\n\
NoDisplay=true\n\
Categories=Game;Utility;\n\
MimeType={};\n",
        exe.display(),
        MIME_TYPES.join(";"),
    )
}

/// Pure text transform over `mimeapps.list`'s content: within
/// `[Default Applications]` *and* `[Added Associations]` (confirmed by
/// testing that `xdg-mime default` writes to both — leaving the latter
/// alone would dangle a reference to a `.desktop` file `uninstall` just
/// deleted), strips `iprolaunch.desktop` out of any of our mimetypes'
/// semicolon-separated desktop-id list, dropping the line entirely if
/// nothing else is left mapped to it. Other sections (e.g.
/// `[Removed Associations]`) are left alone. Kept separate from the real
/// read/write (`remove_default_associations`) so the actual parsing logic —
/// the part actually worth getting right — is unit-testable without
/// touching the user's real `mimeapps.list`.
fn strip_desktop_entry_from_mimeapps(content: &str) -> (String, bool) {
    let mut changed = false;
    let mut in_relevant_section = false;
    let mut out_lines = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_relevant_section =
                trimmed == "[Default Applications]" || trimmed == "[Added Associations]";
            out_lines.push(line.to_string());
            continue;
        }

        let matched = in_relevant_section
            && trimmed
                .split_once('=')
                .is_some_and(|(key, _)| MIME_TYPES.contains(&key.trim()));

        if matched {
            let (key, value) = trimmed.split_once('=').unwrap();
            let before: Vec<&str> = value
                .split(';')
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .collect();
            let after: Vec<&str> = before
                .iter()
                .copied()
                .filter(|d| *d != DESKTOP_FILE_NAME)
                .collect();

            if after.len() != before.len() {
                changed = true;
            }
            if !after.is_empty() {
                out_lines.push(format!("{}={};", key.trim(), after.join(";")));
            }
            // else: drop the line — nothing left mapped to this mimetype.
            continue;
        }

        out_lines.push(line.to_string());
    }

    let mut new_content = out_lines.join("\n");
    new_content.push('\n');
    (new_content, changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_file_contents_includes_the_exact_binary_path_and_both_mimetypes() {
        let contents = desktop_file_contents(Path::new("/opt/iprolaunch/iprolaunch"));
        assert!(contents.contains("Exec=\"/opt/iprolaunch/iprolaunch\" run %f"));
        assert!(contents.contains("Terminal=false"));
        assert!(contents.contains("application/x-msdownload"));
        assert!(contents.contains("application/x-ms-dos-executable"));
    }

    #[test]
    fn strips_our_entry_when_it_is_the_only_one_mapped() {
        let input = "[Default Applications]\napplication/x-msdownload=iprolaunch.desktop;\n";
        let (out, changed) = strip_desktop_entry_from_mimeapps(input);
        assert!(changed);
        assert!(!out.contains("application/x-msdownload"));
    }

    #[test]
    fn leaves_other_apps_mapped_to_the_same_mimetype_alone() {
        let input = "[Default Applications]\napplication/x-msdownload=otherapp.desktop;iprolaunch.desktop;\n";
        let (out, changed) = strip_desktop_entry_from_mimeapps(input);
        assert!(changed);
        assert!(out.contains("application/x-msdownload=otherapp.desktop;"));
        assert!(!out.contains("iprolaunch.desktop"));
    }

    #[test]
    fn leaves_unrelated_mimetypes_and_unrelated_sections_untouched() {
        let input = "[Default Applications]\ntext/plain=gedit.desktop;\n\n[Removed Associations]\napplication/x-msdownload=iprolaunch.desktop;\n";
        let (out, changed) = strip_desktop_entry_from_mimeapps(input);
        // Unrelated mimetype (text/plain), and a section we don't touch
        // ([Removed Associations] has different semantics) — both untouched.
        assert!(!changed);
        assert_eq!(out.trim_end(), input.trim_end());
    }

    #[test]
    fn also_strips_from_added_associations_not_just_default_applications() {
        // Confirmed by testing against the real xdg-mime/mimeapps.list on this
        // machine: `xdg-mime default` writes our desktop-id into *both*
        // sections, so uninstall must clean up both — otherwise
        // [Added Associations] is left with a dangling reference to a
        // `.desktop` file that no longer exists.
        let input =
            "[Added Associations]\napplication/x-msdownload=iprolaunch.desktop;wine.desktop;\n";
        let (out, changed) = strip_desktop_entry_from_mimeapps(input);
        assert!(changed);
        assert_eq!(
            out.trim_end(),
            "[Added Associations]\napplication/x-msdownload=wine.desktop;"
        );
    }

    #[test]
    fn no_change_when_our_entry_is_not_present() {
        let input = "[Default Applications]\napplication/x-msdownload=otherapp.desktop;\n";
        let (_, changed) = strip_desktop_entry_from_mimeapps(input);
        assert!(!changed);
    }

    #[test]
    fn backup_round_trips_through_json() {
        let dir =
            std::env::temp_dir().join(format!("iprolaunch-integrate-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("integrate-backup.json");

        let mut entries = BTreeMap::new();
        entries.insert(
            "application/x-msdownload".to_string(),
            "wine.desktop".to_string(),
        );
        write_backup(&path, &entries).unwrap();

        let read_back = read_backup(&path);
        assert_eq!(read_back, entries);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_backup_is_empty_when_file_is_missing() {
        let entries = read_backup(Path::new("/nonexistent/integrate-backup.json"));
        assert!(entries.is_empty());
    }
}
