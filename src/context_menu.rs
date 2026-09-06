use std::fs;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};
use clap::ValueEnum;

use crate::integrate::{self, MIME_TYPES};

/// Which file managers `context-menu install`/`uninstall` know how to
/// integrate with — see `helper-script/README.md` for what each one
/// actually does and why. `install`/`uninstall` with no explicit choice
/// act on all three; each is independent of the others (a system without
/// KDE installed just never has anything read the KDE files).
#[derive(Clone, Copy, ValueEnum)]
pub enum DesktopEnv {
    Kde,
    Gnome,
    Xfce,
}

const ALL: [DesktopEnv; 3] = [DesktopEnv::Kde, DesktopEnv::Gnome, DesktopEnv::Xfce];

const KDE_TEMPLATE: &str = include_str!("../helper-script/kde/iprolaunch-add.desktop");
const GNOME_SCRIPT_TEMPLATE: &str =
    include_str!("../helper-script/gnome/Add to IProLaunch Library");
const GNOME_SCRIPT_NAME: &str = "Add to IProLaunch Library";
const XFCE_ACTION_TEMPLATE: &str = include_str!("../helper-script/xfce/uca-action.xml");
const XFCE_UNIQUE_ID: &str = "com.iprolaunch.add-to-library";

/// Installs the "Add to IProLaunch Library" right-click action — every DE
/// in `ALL` when `de` is `None` (best-effort across all three, since a
/// system missing one DE just has nothing read its files), or exactly the
/// one requested (propagating its error directly, since an explicit choice
/// failing should be loud).
pub fn install(de: Option<DesktopEnv>) -> Result<()> {
    let exe = std::env::current_exe().context("resolving iprolaunch's own binary path")?;
    match de {
        Some(de) => install_one(de, &exe),
        None => run_all(&exe, install_one),
    }
}

pub fn uninstall(de: Option<DesktopEnv>) -> Result<()> {
    let exe = std::env::current_exe().context("resolving iprolaunch's own binary path")?;
    match de {
        Some(de) => uninstall_one(de, &exe),
        None => run_all(&exe, uninstall_one),
    }
}

/// Runs `action` (install or uninstall) against every DE in `ALL`,
/// reporting each one's outcome individually rather than stopping at the
/// first failure — this is the "no explicit choice" path, so a DE that
/// isn't actually present on this system (its target directories just
/// don't exist, or writing there fails) shouldn't block the others.
fn run_all(
    exe: &std::path::Path,
    action: fn(DesktopEnv, &std::path::Path) -> Result<()>,
) -> Result<()> {
    let mut any_ok = false;
    for de in ALL {
        match action(de, exe) {
            Ok(()) => any_ok = true,
            Err(err) => println!("{}: failed: {err:#}", de.label()),
        }
    }
    if any_ok {
        Ok(())
    } else {
        bail!("nothing succeeded for any desktop environment")
    }
}

impl DesktopEnv {
    fn label(self) -> &'static str {
        match self {
            DesktopEnv::Kde => "KDE (Dolphin)",
            DesktopEnv::Gnome => "GNOME/Cinnamon/MATE (Nautilus/Nemo/Caja)",
            DesktopEnv::Xfce => "XFCE (Thunar)",
        }
    }
}

fn install_one(de: DesktopEnv, exe: &std::path::Path) -> Result<()> {
    match de {
        DesktopEnv::Kde => install_kde(exe)?,
        DesktopEnv::Gnome => install_gnome_family(exe)?,
        DesktopEnv::Xfce => install_xfce(exe)?,
    }
    println!("{}: installed.", de.label());
    Ok(())
}

fn uninstall_one(de: DesktopEnv, _exe: &std::path::Path) -> Result<()> {
    match de {
        DesktopEnv::Kde => uninstall_kde()?,
        DesktopEnv::Gnome => uninstall_gnome_family()?,
        DesktopEnv::Xfce => uninstall_xfce()?,
    }
    println!("{}: removed.", de.label());
    Ok(())
}

fn kde_service_menu_paths() -> Result<Vec<PathBuf>> {
    let data_dir = integrate::base_dirs()?.data_dir().to_path_buf();
    Ok(vec![
        data_dir.join("kio/servicemenus/iprolaunch-add.desktop"),
        data_dir.join("kservices5/ServiceMenus/iprolaunch-add.desktop"),
    ])
}

fn kde_desktop_contents(exe: &std::path::Path) -> String {
    KDE_TEMPLATE
        .replace("{mimetypes}", &MIME_TYPES.join(";"))
        .replace("{bin}", &exe.display().to_string())
}

/// Writes the same service-menu file to both the Plasma 6 (`kio/
/// servicemenus`) and Plasma 5 (`kservices5/ServiceMenus`) locations —
/// see `helper-script/README.md` for why. Best-effort `kbuildsycoca`
/// refresh afterwards (KDE's own service cache, separate from
/// `update-desktop-database`/`gtk-update-icon-cache` — a new service menu
/// otherwise isn't picked up until something else rebuilds it).
///
/// Marked executable (like every other real service menu already in this
/// directory) — KDE refuses to run a non-executable one at all: confirmed
/// for real, without `+x` clicking the menu item itself popped "You are not
/// authorized to execute this file" (KDE's own trust check on the
/// `.desktop` file, unrelated to the `+x`-on-the-target-exe fix in
/// `launch.rs` — a completely different file being checked here).
fn install_kde(exe: &std::path::Path) -> Result<()> {
    let contents = kde_desktop_contents(exe);
    for path in kde_service_menu_paths()? {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::write(&path, &contents).with_context(|| format!("writing {}", path.display()))?;
        make_executable(&path)?;
    }
    refresh_kde_sycoca();
    Ok(())
}

fn uninstall_kde() -> Result<()> {
    for path in kde_service_menu_paths()? {
        if path.exists() {
            fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        }
    }
    refresh_kde_sycoca();
    Ok(())
}

fn refresh_kde_sycoca() {
    // Best-effort, same reasoning as `integrate::refresh_desktop_database`:
    // whichever of these actually exists on this system rebuilds KDE's own
    // service cache so the new menu entry shows up without a re-login.
    let _ = Command::new("kbuildsycoca6").status();
    let _ = Command::new("kbuildsycoca5").status();
}

fn gnome_script_dirs() -> Result<Vec<PathBuf>> {
    let base = integrate::base_dirs()?;
    Ok(vec![
        base.data_dir().join("nautilus/scripts"),
        base.data_dir().join("nemo/scripts"),
        base.config_dir().join("caja/scripts"),
    ])
}

fn gnome_script_contents(exe: &std::path::Path) -> String {
    GNOME_SCRIPT_TEMPLATE.replace("{bin}", &exe.display().to_string())
}

/// Writes (and marks executable) the same script into Nautilus's, Nemo's,
/// and Caja's own "scripts" folders — see `helper-script/README.md`; one
/// script covers all three since they share the drop-a-script-in convention
/// (and Nemo understands Nautilus's own selection env var).
fn install_gnome_family(exe: &std::path::Path) -> Result<()> {
    let contents = gnome_script_contents(exe);
    for dir in gnome_script_dirs()? {
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let path = dir.join(GNOME_SCRIPT_NAME);
        fs::write(&path, &contents).with_context(|| format!("writing {}", path.display()))?;
        make_executable(&path)?;
    }
    Ok(())
}

fn uninstall_gnome_family() -> Result<()> {
    for dir in gnome_script_dirs()? {
        let path = dir.join(GNOME_SCRIPT_NAME);
        if path.exists() {
            fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        }
    }
    Ok(())
}

fn make_executable(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)
        .with_context(|| format!("reading permissions for {}", path.display()))?
        .permissions();
    perms.set_mode(perms.mode() | 0o111);
    fs::set_permissions(path, perms)
        .with_context(|| format!("marking {} executable", path.display()))
}

fn thunar_uca_path() -> Result<PathBuf> {
    Ok(integrate::base_dirs()?.config_dir().join("Thunar/uca.xml"))
}

fn xfce_action_xml(exe: &std::path::Path) -> String {
    XFCE_ACTION_TEMPLATE.replace("{bin}", &exe.display().to_string())
}

/// Thunar has no drop-a-file-in mechanism like KDE/Nautilus — every custom
/// action lives in one shared `uca.xml`, so this merges our `<action>`
/// block in (identified by `XFCE_UNIQUE_ID`) rather than overwriting
/// whatever the user already has configured there.
fn install_xfce(exe: &std::path::Path) -> Result<()> {
    let path = thunar_uca_path()?;
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let updated = upsert_uca_action(&existing, XFCE_UNIQUE_ID, &xfce_action_xml(exe));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(&path, updated).with_context(|| format!("writing {}", path.display()))
}

fn uninstall_xfce() -> Result<()> {
    let path = thunar_uca_path()?;
    let Ok(existing) = fs::read_to_string(&path) else {
        return Ok(());
    };
    let (updated, changed) = remove_uca_action(&existing, XFCE_UNIQUE_ID);
    if changed {
        fs::write(&path, updated).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

/// Pure text transform: strips any `<action>...</action>` block whose body
/// contains `<unique-id>{unique_id}</unique-id>`, leaving every other
/// action untouched. Kept separate from the real read/write so the parsing
/// logic is unit-testable without a real `uca.xml`.
fn remove_uca_action(content: &str, unique_id: &str) -> (String, bool) {
    let marker = format!("<unique-id>{unique_id}</unique-id>");
    let mut out = String::new();
    let mut rest = content;
    let mut changed = false;
    loop {
        let Some(start) = rest.find("<action>") else {
            out.push_str(rest);
            break;
        };
        let Some(end_rel) = rest[start..].find("</action>") else {
            out.push_str(rest);
            break;
        };
        let end = start + end_rel + "</action>".len();
        if rest[start..end].contains(&marker) {
            out.push_str(&rest[..start]);
            changed = true;
            // Also drop the one newline that terminated the removed
            // block's own line, so it doesn't leave a blank line behind —
            // otherwise repeated install/uninstall cycles would
            // accumulate them.
            rest = rest[end..].strip_prefix('\n').unwrap_or(&rest[end..]);
            continue;
        }
        out.push_str(&rest[..end]);
        rest = &rest[end..];
    }
    (out, changed)
}

/// Removes any existing action with `unique_id` (so re-running `install`
/// after the binary moved updates it in place instead of duplicating it),
/// then inserts the fresh `action_xml` just before the closing `</actions>`
/// — or, if `content` doesn't have one yet (no `uca.xml` existed), wraps it
/// in a fresh minimal document.
fn upsert_uca_action(content: &str, unique_id: &str, action_xml: &str) -> String {
    let (without_ours, _) = remove_uca_action(content, unique_id);
    match without_ours.rfind("</actions>") {
        Some(pos) => {
            // `action_xml` already ends in its own trailing newline (from
            // the embedded template file) — don't add a second, or a
            // blank line accumulates right before `</actions>`.
            let mut out = without_ours[..pos].to_string();
            out.push_str(action_xml);
            out.push_str(&without_ours[pos..]);
            out
        }
        None => {
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<actions>\n{action_xml}</actions>\n"
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kde_desktop_contents_substitutes_both_placeholders() {
        let contents = kde_desktop_contents(std::path::Path::new("/opt/iprolaunch/iprolaunch"));
        assert!(contents.contains("Exec=\"/opt/iprolaunch/iprolaunch\" add %f"));
        for mime in MIME_TYPES {
            assert!(contents.contains(mime), "missing {mime}");
        }
        assert!(!contents.contains("{bin}"));
        assert!(!contents.contains("{mimetypes}"));
    }

    #[test]
    fn gnome_script_contents_substitutes_the_binary_path() {
        let contents = gnome_script_contents(std::path::Path::new("/opt/iprolaunch/iprolaunch"));
        assert!(contents.contains("\"/opt/iprolaunch/iprolaunch\" add \"$path\""));
        assert!(!contents.contains("{bin}"));
    }

    #[test]
    fn xfce_action_xml_substitutes_the_binary_path() {
        let xml = xfce_action_xml(std::path::Path::new("/opt/iprolaunch/iprolaunch"));
        assert!(xml.contains("\"/opt/iprolaunch/iprolaunch\" add %f"));
        assert!(!xml.contains("{bin}"));
    }

    #[test]
    fn upsert_creates_a_fresh_document_when_none_existed() {
        let out = upsert_uca_action("", "my-id", "<action>X</action>");
        assert!(out.contains("<?xml"));
        assert!(out.contains("<actions>"));
        assert!(out.contains("<action>X</action>"));
        assert!(out.contains("</actions>"));
    }

    #[test]
    fn upsert_inserts_before_the_closing_actions_tag_leaving_others_intact() {
        let existing = "<?xml version=\"1.0\"?>\n<actions>\n<action><name>Other</name><unique-id>other</unique-id></action>\n</actions>\n";
        let out = upsert_uca_action(existing, "my-id", "<action>Mine</action>");
        assert!(out.contains("<action><name>Other</name><unique-id>other</unique-id></action>"));
        assert!(out.contains("<action>Mine</action>"));
        // Ours comes before the closing tag, after the other action.
        assert!(out.find("<action>Mine</action>").unwrap() < out.find("</actions>").unwrap());
    }

    #[test]
    fn upsert_replaces_a_prior_copy_of_our_own_action_instead_of_duplicating() {
        let existing = "<actions>\n<action><unique-id>my-id</unique-id><command>old</command></action>\n</actions>\n";
        let out = upsert_uca_action(
            existing,
            "my-id",
            "<action><unique-id>my-id</unique-id><command>new</command></action>",
        );
        assert_eq!(out.matches("<action>").count(), 1);
        assert!(out.contains("new"));
        assert!(!out.contains("old"));
    }

    #[test]
    fn remove_uca_action_strips_only_the_matching_block() {
        let existing = "<actions>\n<action><unique-id>keep</unique-id></action>\n<action><unique-id>drop-me</unique-id></action>\n</actions>\n";
        let (out, changed) = remove_uca_action(existing, "drop-me");
        assert!(changed);
        assert!(out.contains("keep"));
        assert!(!out.contains("drop-me"));
    }

    #[test]
    fn remove_uca_action_does_not_leave_a_blank_line_behind() {
        let existing = "<actions>\n<action><unique-id>drop-me</unique-id></action>\n</actions>\n";
        let (out, changed) = remove_uca_action(existing, "drop-me");
        assert!(changed);
        assert_eq!(out, "<actions>\n</actions>\n");
    }

    #[test]
    fn remove_uca_action_reports_unchanged_when_nothing_matches() {
        let existing = "<actions>\n<action><unique-id>keep</unique-id></action>\n</actions>\n";
        let (out, changed) = remove_uca_action(existing, "not-there");
        assert!(!changed);
        assert_eq!(out, existing);
    }
}
