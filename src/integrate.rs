use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Windows PE executables are already recognized under the first two of
/// these on a stock Linux system (shared-mime-info sniffs the "MZ" header,
/// not the `.exe` extension) — we only need to register as the *default
/// handler*, not teach the OS a new file type. `.bat`/`.cmd` both resolve
/// to the third, and `.msi` to the fourth (all confirmed via `xdg-mime
/// query filetype` on real files) — `iprolaunch run` already launches all
/// of these correctly with zero extra wrapping (`wine <path>` recognizes
/// `.bat`/`.msi` by extension and dispatches to `cmd`/`msiexec` itself
/// internally — confirmed directly: a real `.msi` engaged wine's MSI
/// installer UI, not a generic "unrecognized file" error — see project
/// NOTES.md), this just extends the *file-manager* double-click
/// association to cover them too, matching the `.exe` experience. Modeled
/// after `wine.desktop`'s own `MimeType=` list (`/usr/share/applications/
/// wine.desktop`, which additionally claims `application/x-ms-shortcut`
/// (`.lnk`) and `application/x-mswinurl` (`.url`) — the latter is a URL/
/// website shortcut, clearly out of scope for a game/exe launcher; `.lnk`
/// is deliberately NOT included yet — see TODO.md, needs a real generated
/// shortcut to verify properly rather than the inconclusive synthetic file
/// tested so far).
const MIME_TYPES: [&str; 4] = [
    "application/x-msdownload",
    "application/x-ms-dos-executable",
    "application/x-bat",
    "application/x-msi",
];
const DESKTOP_FILE_NAME: &str = "iprolaunch.desktop";
/// A second, separate `.desktop` file from the mimetype-handler one above:
/// that one is `NoDisplay=true` and its `Exec=` line ends in `run %f`, which
/// only makes sense invoked by a file manager with a real file to hand it —
/// launched bare from an app menu (no `%f` to substitute) it'd just run
/// `iprolaunch run` with no path and fail immediately. This entry instead
/// runs plain `iprolaunch` (opens the TUI) inside a terminal
/// (`Terminal=true` — it's a TUI, not a GUI) and is left visible
/// (`NoDisplay` omitted/false) so it shows up in the app menu/start menu —
/// installed and removed alongside the handler entry, not standalone.
const MENU_DESKTOP_FILE_NAME: &str = "iprolaunch-menu.desktop";

/// Freedesktop icon name (no extension) both `.desktop` entries' `Icon=`
/// lines reference — resolved via the standard hicolor icon theme lookup,
/// not a hardcoded path, so it works the same way any other installed
/// app's icon does. The actual image bytes are embedded in the binary
/// (`ICON_SVG`/`ICON_PNG`) and written out to the theme directories by
/// `install`, so there's no runtime dependency on `assets/` existing next
/// to the binary.
const ICON_NAME: &str = "iprolaunch";
const ICON_SVG: &[u8] = include_bytes!("../assets/icon.svg");
const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

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

fn menu_desktop_file_path() -> Result<PathBuf> {
    Ok(applications_dir()?.join(MENU_DESKTOP_FILE_NAME))
}

/// `~/.local/share/icons/hicolor` — the user-local override of the
/// standard fallback icon theme every desktop environment ships, so
/// anything installed under it is picked up regardless of whichever icon
/// theme is actually active (GNOME/KDE/XFCE all fall back to hicolor).
fn hicolor_dir() -> Result<PathBuf> {
    Ok(base_dirs()?.data_dir().join("icons").join("hicolor"))
}

/// The scalable SVG variant — sized-agnostic, so this alone is enough for
/// any toolkit that honors scalable icons (GTK/Qt both do).
fn icon_svg_path() -> Result<PathBuf> {
    Ok(hicolor_dir()?
        .join("scalable/apps")
        .join(format!("{ICON_NAME}.svg")))
}

/// A rendered 256x256 fallback for anything that only looks at fixed-size
/// buckets (e.g. some file managers' thumbnailers) rather than scalable.
fn icon_png_path() -> Result<PathBuf> {
    Ok(hicolor_dir()?
        .join("256x256/apps")
        .join(format!("{ICON_NAME}.png")))
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

/// The binary path the installed `.desktop` entry's `Exec=` line actually
/// points at — read from that file, not this process's own
/// `current_exe()`, since they can differ if the registered binary was
/// moved (or a different `iprolaunch` build is running now) since
/// `install` last wrote it. `None` if not installed or the file's somehow
/// unparsable.
pub fn registered_binary_path() -> Option<String> {
    let path = desktop_file_path().ok()?;
    let content = fs::read_to_string(path).ok()?;
    parse_exec_path(&content)
}

/// Pure text parse of a `.desktop` file's `Exec="<path>" run %f` line, kept
/// separate from the real read (`registered_binary_path`) so the parsing
/// itself — the part actually worth getting right — is unit-testable
/// without needing a real installed `.desktop` file.
fn parse_exec_path(content: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let rest = line.strip_prefix("Exec=\"")?;
        let end = rest.find('"')?;
        Some(rest[..end].to_string())
    })
}

/// Registers `iprolaunch` as the default handler for Windows `.exe`
/// files (and `.bat`/`.cmd` — see `MIME_TYPES`): writes a `.desktop` file
/// pointing at this exact binary, then uses
/// `xdg-mime default` (part of `xdg-utils`, standard on any desktop Linux —
/// this is a freedesktop.org mechanism, not KDE-specific, so it should work
/// on GNOME/XFCE too) to register it. Launches are detached (`Terminal=false`)
/// by design — see `launch::open_in_pager`'s `IsTerminal` guard, added
/// specifically so a detached launch's failure path doesn't try to spawn a
/// pager with nothing to attach to.
///
/// Before changing anything, records whatever was the previous default for
/// each mimetype (if any) to `backup_path()` — but *only for mimetypes not
/// already captured there*, never overwriting an existing entry. Without
/// that "don't overwrite" rule, running `install` a second time without an
/// `uninstall` in between would overwrite the real original with
/// "iprolaunch.desktop" itself (since by then *we're* the current
/// default), permanently losing what `uninstall` is supposed to restore.
/// The "top up missing entries" half (rather than skipping the whole
/// capture once the file exists at all) matters when `MIME_TYPES` itself
/// grows — confirmed by hand: adding `.bat`/`.cmd` support to an already-
/// installed system left the *existing* backup covering only the original
/// two mimetypes, and a plain "skip if file exists" would have left the
/// newly-added one with nothing to restore on a later `uninstall`.
pub fn install() -> Result<()> {
    let exe = std::env::current_exe().context("resolving iprolaunch's own binary path")?;
    let dir = applications_dir()?;
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    let backup_file = backup_path()?;
    let mut backup = read_backup(&backup_file);
    let missing = capture_prior_defaults_for(
        MIME_TYPES
            .iter()
            .filter(|m| !backup.contains_key(**m))
            .copied(),
    );
    if !missing.is_empty() {
        backup.extend(missing);
        write_backup(&backup_file, &backup)?;
    }

    let desktop_path = desktop_file_path()?;
    let contents = desktop_file_contents(&exe);
    fs::write(&desktop_path, contents)
        .with_context(|| format!("writing {}", desktop_path.display()))?;

    let menu_path = menu_desktop_file_path()?;
    fs::write(&menu_path, menu_desktop_file_contents(&exe))
        .with_context(|| format!("writing {}", menu_path.display()))?;

    install_icon()?;

    for mime in MIME_TYPES {
        set_default(DESKTOP_FILE_NAME, mime)?;
    }

    refresh_desktop_database(&dir);
    refresh_icon_cache();

    println!("Installed {}", desktop_path.display());
    println!("Default handler for:");
    for mime in MIME_TYPES {
        println!("  {mime}");
    }
    println!(
        "Launches run detached (no terminal window) — see `iprolaunch running list` to check on one."
    );
    println!("Added to the app menu (start menu) as IProLaunch.");
    Ok(())
}

fn install_icon() -> Result<()> {
    let svg_path = icon_svg_path()?;
    if let Some(parent) = svg_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(&svg_path, ICON_SVG).with_context(|| format!("writing {}", svg_path.display()))?;

    let png_path = icon_png_path()?;
    if let Some(parent) = png_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(&png_path, ICON_PNG).with_context(|| format!("writing {}", png_path.display()))?;

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

    let menu_path = menu_desktop_file_path()?;
    let removed_menu = menu_path.exists();
    if removed_menu {
        fs::remove_file(&menu_path).with_context(|| format!("removing {}", menu_path.display()))?;
    }

    let removed_icon = remove_icon()?;

    let backup_file = backup_path()?;
    let restored = restore_prior_defaults(&backup_file)?;
    let cleared = remove_default_associations()?;
    let _ = fs::remove_file(&backup_file); // fresh slate for the next `install`

    let dir = applications_dir()?;
    refresh_desktop_database(&dir);
    refresh_icon_cache();

    if removed_desktop || removed_menu || removed_icon || restored || cleared {
        println!("Uninstalled.");
    } else {
        println!("Nothing to uninstall — it wasn't installed.");
    }
    Ok(())
}

fn remove_icon() -> Result<bool> {
    let svg_path = icon_svg_path()?;
    let had_svg = svg_path.exists();
    if had_svg {
        fs::remove_file(&svg_path).with_context(|| format!("removing {}", svg_path.display()))?;
    }

    let png_path = icon_png_path()?;
    let had_png = png_path.exists();
    if had_png {
        fs::remove_file(&png_path).with_context(|| format!("removing {}", png_path.display()))?;
    }

    Ok(had_svg || had_png)
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

/// Queries the current default for each of `mimes`, keeping only the ones
/// that actually have one set (nothing to restore for a mimetype that had
/// no prior default at all).
fn capture_prior_defaults_for<'a>(
    mimes: impl Iterator<Item = &'a str>,
) -> BTreeMap<String, String> {
    mimes
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

fn refresh_icon_cache() {
    // Best-effort, same reasoning as `refresh_desktop_database`: icon
    // lookup itself doesn't strictly require this (hicolor is scanned
    // directly by most toolkits), but a few DEs use the cache for speed
    // and won't notice a new icon until it's rebuilt. `gtk-update-icon-cache`
    // isn't guaranteed to be installed outside GTK-based desktops, hence
    // ignoring the error entirely rather than surfacing it.
    if let Ok(dir) = hicolor_dir() {
        let _ = Command::new("gtk-update-icon-cache")
            .arg("-f")
            .arg(&dir)
            .status();
    }
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
Icon={ICON_NAME}\n\
Categories=Game;Utility;\n\
MimeType={};\n",
        exe.display(),
        MIME_TYPES.join(";"),
    )
}

/// The app-menu-visible entry (see `MENU_DESKTOP_FILE_NAME`): bare `Exec=`
/// (no `%f`, nothing to substitute from a menu launch) inside a terminal,
/// since `iprolaunch` with no arguments opens the TUI. `NoDisplay` is
/// omitted (defaults to false) so it actually shows up, unlike the
/// handler entry above.
fn menu_desktop_file_contents(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
Type=Application\n\
Name=IProLaunch\n\
Comment=Launch Windows apps/games through Proton via umu-run\n\
Exec=\"{}\"\n\
Terminal=true\n\
Icon={ICON_NAME}\n\
Categories=Game;Utility;\n",
        exe.display(),
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
    fn desktop_file_contents_includes_the_exact_binary_path_and_every_mimetype() {
        let contents = desktop_file_contents(Path::new("/opt/iprolaunch/iprolaunch"));
        assert!(contents.contains("Exec=\"/opt/iprolaunch/iprolaunch\" run %f"));
        assert!(contents.contains("Terminal=false"));
        for mime in MIME_TYPES {
            assert!(contents.contains(mime), "missing {mime}");
        }
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

    #[test]
    fn parse_exec_path_extracts_the_quoted_binary_path() {
        let content = desktop_file_contents(Path::new("/opt/iprolaunch/iprolaunch"));
        assert_eq!(
            parse_exec_path(&content),
            Some("/opt/iprolaunch/iprolaunch".to_string())
        );
    }

    #[test]
    fn parse_exec_path_is_none_without_an_exec_line() {
        assert_eq!(parse_exec_path("[Desktop Entry]\nType=Application\n"), None);
    }
}
