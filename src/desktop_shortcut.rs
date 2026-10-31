//! Per-game app-launcher entries, all under `~/.local/share/applications/iprolaunch/`
//! so they're grouped on disk and easy to clean up.

use std::collections::HashSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::Profile;

/// Subfolder of `applications/`; the Desktop Entry spec turns
/// `iprolaunch/game-x.desktop` into the desktop-file ID `iprolaunch-game-x.desktop`.
const SUBDIR: &str = "iprolaunch";
/// Keeps IDs clear of `integrate`'s own `iprolaunch-menu.desktop` (a slug `menu` would collide).
const FILE_PREFIX: &str = "game-";
const FILE_SUFFIX: &str = ".desktop";
const MENU_FILE_NAME: &str = "iprolaunch.menu";
const DIRECTORY_FILE_NAME: &str = "iprolaunch.directory";

/// XDG menu merge: an "IProLaunch" submenu in menu-based launchers (KDE, XFCE, LXQt).
const MENU_FILE_CONTENTS: &str = r#"<!DOCTYPE Menu PUBLIC "-//freedesktop//DTD Menu 1.0//EN"
 "http://www.freedesktop.org/standards/menu-spec/1.0/menu.dtd">
<Menu>
  <Name>Applications</Name>
  <Menu>
    <Name>IProLaunch</Name>
    <Directory>iprolaunch.directory</Directory>
    <Include>
      <Category>X-IProLaunch</Category>
    </Include>
  </Menu>
</Menu>
"#;

const DIRECTORY_FILE_CONTENTS: &str =
    "[Desktop Entry]\nType=Directory\nName=IProLaunch\nIcon=iprolaunch\n";

/// Whether a write made a new shortcut or replaced an existing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Written {
    Created,
    Updated,
}

/// `~/.local/share/applications/iprolaunch`.
pub fn shortcuts_dir() -> Result<PathBuf> {
    Ok(crate::integrate::base_dirs()?
        .data_dir()
        .join("applications")
        .join(SUBDIR))
}

fn menu_file_path() -> Result<PathBuf> {
    Ok(crate::integrate::base_dirs()?
        .config_dir()
        .join("menus/applications-merged")
        .join(MENU_FILE_NAME))
}

fn directory_file_path() -> Result<PathBuf> {
    Ok(crate::integrate::base_dirs()?
        .data_dir()
        .join("desktop-directories")
        .join(DIRECTORY_FILE_NAME))
}

fn file_name(slug: &str) -> String {
    format!("{FILE_PREFIX}{slug}{FILE_SUFFIX}")
}

fn slug_from_file_name(name: &str) -> Option<&str> {
    name.strip_prefix(FILE_PREFIX)?
        .strip_suffix(FILE_SUFFIX)
        .filter(|slug| !slug.is_empty())
}

/// The `Icon=` value: the cover's absolute path, else IProLaunch's own themed icon.
pub fn icon_value(cover: Option<&Path>) -> String {
    cover.map_or_else(
        || crate::integrate::ICON_NAME.to_string(),
        |path| path.display().to_string(),
    )
}

/// Escapes a `string`/`localestring` value per the Desktop Entry spec.
fn escape_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

fn entry_contents(exe: &Path, slug: &str, title: &str, icon: &str) -> String {
    format!(
        "[Desktop Entry]\n\
Type=Application\n\
Name={}\n\
Comment=Launch with IProLaunch\n\
Exec={} {}\n\
Terminal=false\n\
Icon={}\n\
Categories=Game;X-IProLaunch;\n",
        escape_value(title),
        crate::integrate::desktop_exec_quote(exe),
        crate::integrate::desktop_exec_quote_str(slug),
        escape_value(icon),
    )
}

fn write_entry(dir: &Path, slug: &str, contents: &str) -> Result<Written> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(file_name(slug));
    let written = if path.exists() {
        Written::Updated
    } else {
        Written::Created
    };
    crate::config::write_atomic(&path, contents.as_bytes())?;
    Ok(written)
}

/// Removes `slug`'s entry from `dir`; `Ok(false)` when there was none.
fn remove_entry(dir: &Path, slug: &str) -> Result<bool> {
    remove_if_present(&dir.join(file_name(slug)))
}

fn remove_if_present(path: &Path) -> Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err).with_context(|| format!("removing {}", path.display())),
    }
}

fn slugs_in(dir: &Path) -> HashSet<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return HashSet::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            slug_from_file_name(name.to_str()?).map(str::to_string)
        })
        .collect()
}

/// Slugs that currently have a shortcut; empty when the folder is missing.
pub fn installed_slugs() -> HashSet<String> {
    shortcuts_dir()
        .map(|dir| slugs_in(&dir))
        .unwrap_or_default()
}

fn write_menu_files() -> Result<()> {
    for (path, contents) in [
        (menu_file_path()?, MENU_FILE_CONTENTS),
        (directory_file_path()?, DIRECTORY_FILE_CONTENTS),
    ] {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        crate::config::write_atomic(&path, contents.as_bytes())?;
    }
    Ok(())
}

fn remove_menu_files() -> Result<()> {
    remove_if_present(&menu_file_path()?)?;
    remove_if_present(&directory_file_path()?)?;
    Ok(())
}

/// Creates or rewrites `slug`'s launcher entry, pointing at this binary.
pub fn add(profile: &Profile, slug: &str, icon: &str) -> Result<Written> {
    let exe = crate::updater::own_exe()?.canonicalize()?;
    let contents = entry_contents(&exe, slug, profile.display_title(), icon);
    let written = write_entry(&shortcuts_dir()?, slug, &contents)?;
    write_menu_files()?;
    Ok(written)
}

/// Removes `slug`'s entry, and the folder and menu files once no shortcut is left.
pub fn remove(slug: &str) -> Result<bool> {
    let dir = shortcuts_dir()?;
    let removed = remove_entry(&dir, slug)?;
    if slugs_in(&dir).is_empty() {
        let _ = fs::remove_dir(&dir); // stays if the user keeps other files there
        remove_menu_files()?;
    }
    Ok(removed)
}

/// Moves `old`'s entry to `new` after a slug rename, keeping its cover icon.
pub fn rename(old: &str, new: &str, profile: &Profile) -> Result<()> {
    let cover = crate::covers::cached_cover(new).or_else(|| crate::covers::cached_cover(old));
    add(profile, new, &icon_value(cover.as_deref()))?;
    remove_entry(&shortcuts_dir()?, old)?;
    Ok(())
}

/// Removes every game shortcut, the folder and the menu files; returns how many entries went.
pub fn remove_all() -> Result<usize> {
    let dir = shortcuts_dir()?;
    let slugs = slugs_in(&dir);
    for slug in &slugs {
        remove_entry(&dir, slug)?;
    }
    let _ = fs::remove_dir(&dir);
    remove_menu_files()?;
    Ok(slugs.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "iprolaunch-desktop-shortcut-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn entry_quotes_exec_args_and_escapes_values() {
        let contents = entry_contents(
            Path::new("/opt/my $game/iprolaunch"),
            "elden-ring",
            "Elden\nRing \\ DLC",
            "/covers/a b.png",
        );
        assert!(
            contents.contains("Exec=\"/opt/my \\\\$game/iprolaunch\" \"elden-ring\"\n"),
            "{contents}"
        );
        assert!(
            contents.contains("Name=Elden\\nRing \\\\ DLC\n"),
            "{contents}"
        );
        assert!(contents.contains("Icon=/covers/a b.png\n"), "{contents}");
        assert!(
            contents.contains("Categories=Game;X-IProLaunch;\n"),
            "{contents}"
        );
        assert!(contents.contains("Terminal=false\n"), "{contents}");
    }

    #[test]
    fn file_name_round_trips_and_other_files_are_ignored() {
        assert_eq!(slug_from_file_name(&file_name("game-1")), Some("game-1"));
        assert_eq!(slug_from_file_name("iprolaunch-menu.desktop"), None);
        assert_eq!(slug_from_file_name("game-.desktop"), None);
        assert_eq!(slug_from_file_name("game-x.txt"), None);
    }

    #[test]
    fn icon_value_prefers_the_cover_and_falls_back_to_the_app_icon() {
        assert_eq!(icon_value(Some(Path::new("/c/x.png"))), "/c/x.png");
        assert_eq!(icon_value(None), crate::integrate::ICON_NAME);
    }

    #[test]
    fn rewriting_reports_updated_and_removal_is_idempotent() {
        let dir = temp_dir("write");
        assert_eq!(write_entry(&dir, "a", "one").unwrap(), Written::Created);
        assert_eq!(write_entry(&dir, "a", "two").unwrap(), Written::Updated);
        assert_eq!(
            fs::read_to_string(dir.join("game-a.desktop")).unwrap(),
            "two"
        );
        assert_eq!(slugs_in(&dir), HashSet::from(["a".to_string()]));
        assert!(remove_entry(&dir, "a").unwrap());
        assert!(!remove_entry(&dir, "a").unwrap());
        assert!(slugs_in(&dir).is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }
}
