use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::config::{Config, Profile, project_dirs};

/// Directory holding the small per-profile `.desktop` wrapper files used
/// only to hand a game to Steam's own `steam://addnonsteamgame/` importer
/// (see `add_profile`'s doc comment for why) — deliberately *not*
/// `~/.local/share/applications` (the shared desktop-entry dir
/// `integrate::install`/`context_menu::install` use for the real app-menu/
/// file-handler/right-click entries), since these wrappers aren't meant to
/// show up in any menu themselves, only ever be read once by Steam.
fn wrappers_dir() -> Result<PathBuf> {
    Ok(project_dirs()?.data_dir().join("steam-shortcuts"))
}

/// Plain, non-executable `.desktop` file content Steam reads `Name=`/`Exec=`
/// straight out of — this is the exact same path a `.desktop` file dropped
/// into Steam's own "Add a Non-Steam Game" browse dialog takes (confirmed
/// real Steam-for-Linux behavior researched directly, not guessed), so
/// iprolaunch never executes this file itself. With no `extra_flags`,
/// `Exec=` is byte-for-byte the same `"<binary>" <slug>` format the TUI's
/// `c` key/`add`'s clipboard copy already produce
/// (`quick_launch_cmd::command_for`) — built directly here instead of
/// splicing into that function's own string output, since `exe` itself can
/// contain spaces (a real, common case — e.g. a game under `.../Downloads/
/// Programs/`) and naively splitting on the first space would cut through
/// the quoted exe path itself. Any `extra_flags` (see
/// `launch::iprolaunch_cli_flags_for`) go between the exe and the slug,
/// each its own separate token — confirmed for real that Steam's Launch
/// Options box then shows each as its own separately-quoted segment (e.g.
/// `"-w" "<slug>"`), which is required: one quoted segment containing more
/// than one value (e.g. `"-w <slug>"`) does not work.
fn wrapper_contents(name: &str, exe: &Path, extra_flags: &[&str], slug: &str) -> String {
    let mut exec = format!("\"{}\"", exe.display());
    for flag in extra_flags {
        exec.push(' ');
        exec.push_str(flag);
    }
    exec.push(' ');
    exec.push_str(slug);
    format!(
        "[Desktop Entry]\nType=Application\nName={}\nExec={exec}\n",
        name.replace('\n', " "), // .desktop keys are single-line; a name can't legally contain one
    )
}

/// Percent-encodes every byte that isn't in the URL "unreserved" set
/// (letters/digits/`-_.~`), including `/` — matches
/// `steam-add-nonsteam-game`'s own `quote(str(path), safe='')`, confirmed
/// against its actual source. Encoding the whole path (not just the parts
/// that need it) is deliberate: simpler than guessing which characters are
/// safe to leave bare in a `steam://` URL, and unambiguously correct either way.
fn percent_encode_path(path: &Path) -> String {
    path.to_string_lossy()
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn steam_url(wrapper_path: &Path) -> String {
    format!(
        "steam://addnonsteamgame/{}",
        percent_encode_path(wrapper_path)
    )
}

/// Writes (or overwrites) `slug`'s wrapper `.desktop` file, returning its path.
fn write_wrapper(profile: &Profile, extra_flags: &[&str], slug: &str) -> Result<PathBuf> {
    let dir = wrappers_dir()?;
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let exe = std::env::current_exe().context("resolving iprolaunch's own binary path")?;
    let path = dir.join(format!("{slug}.desktop"));
    let contents = wrapper_contents(profile.display_title(), &exe, extra_flags, slug);
    fs::write(&path, contents).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Steam's `addnonsteamgame` URL handler silently no-ops unless this
/// sentinel file exists first (confirmed for real: without it, Steam's own
/// log shows `ExecuteSteamURL` firing, yet `shortcuts.vdf` is never
/// touched) — almost certainly a guard against a random web page/link
/// silently adding programs to a user's library just by getting them to
/// click a `steam://addnonsteamgame/...` URL. `steam-add-nonsteam-game`
/// (the reference tool this whole approach is modeled on) touches the same
/// path before every call, mode `0o600`, `exist_ok` — mirrored exactly here.
/// `std::env::temp_dir()` (not a hardcoded `/tmp`) matches Python's
/// `tempfile.gettempdir()`, which is what that tool actually uses.
fn touch_add_sentinel() -> Result<()> {
    let path = std::env::temp_dir().join("addnonsteamgamefile");
    let file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
        .with_context(|| format!("creating sentinel {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .with_context(|| format!("chmod 600 {}", path.display()))?;
    }
    Ok(())
}

/// Hands `slug` to the *running* Steam client's own `steam://
/// addnonsteamgame/` URL importer, via a small `.desktop` wrapper file
/// (`write_wrapper`) rather than editing `shortcuts.vdf` directly.
///
/// Why: `shortcuts.vdf` is a single binary file holding *every* existing
/// non-Steam shortcut, and Steam only re-reads it at startup — editing it
/// directly while Steam is already running (the common case) risks Steam
/// silently overwriting the edit from its own in-memory copy the next time
/// it saves anything (this exact risk is why a prior version of this idea
/// was dropped — see project NOTES.md). The `steam://` URL handler sidesteps
/// that entirely: Steam itself performs the write, from inside its own
/// already-running process, live, no restart required.
///
/// The URL handler only accepts a single literal path (it does not split on
/// whitespace into an exe + arguments the way a shell would — confirmed via
/// a real Steam-for-Linux bug report about exactly that breaking on a
/// space), so `"<iprolaunch>" <slug>` can't be passed directly. Steam
/// separately has native support for importing a `.desktop` file's own
/// `Name=`/`Exec=` fields when *that's* the path handed to it (the same
/// path a `.desktop` file dragged into Steam's own "Add a Non-Steam Game"
/// dialog takes) — so the wrapper file is the thing actually pointed at.
///
/// `with_gamescope_flags` bakes this profile's own effective `-f`/`-w`/`-b`
/// into the shortcut's Launch Options (see `launch::iprolaunch_cli_flags_for`)
/// — safe even for a shortcut that might later be launched from Steam Game
/// Mode, since `launch::run` now automatically skips the nested-gamescope
/// wrap whenever it detects it's already running under gamescope, instead
/// of crashing with "Gamescope WSI Layer Error".
///
/// Does NOT check for an existing entry first — this only ever *adds*, and
/// calling it again for an already-added `slug` creates a duplicate rather
/// than replacing anything (there is no `steam://` URL to update or remove
/// a shortcut, confirmed — only to add one). Callers (the TUI's `s` key,
/// `library add-to-steam`) are expected to check `slugs_in_steam()` first
/// and refuse a repeat add themselves.
pub fn add_profile(
    cfg: &Config,
    profile: &Profile,
    slug: &str,
    with_gamescope_flags: bool,
) -> Result<PathBuf> {
    let extra_flags = if with_gamescope_flags {
        crate::launch::iprolaunch_cli_flags_for(&cfg.effective(Some(profile)))
    } else {
        Vec::new()
    };
    let wrapper_path = write_wrapper(profile, &extra_flags, slug)?;
    touch_add_sentinel()?;
    let url = steam_url(&wrapper_path);
    let status = Command::new("steam")
        .arg(&url)
        .status()
        .context("failed to run steam (is it installed and on $PATH?)")?;
    if !status.success() {
        bail!("steam exited with {status}");
    }
    Ok(wrapper_path)
}

/// Steam's binary VDF format (distinct from the plain-text KeyValues VDF
/// used elsewhere) — every "object" is a sequence of typed entries ended by
/// `0x08`: `0x00` = nested object (recurse), `0x01` = string (null-
/// terminated key, then null-terminated value), `0x02` = int32 (null-
/// terminated key, then 4 raw little-endian bytes). Confirmed against a
/// real `shortcuts.vdf` on this machine (byte-for-byte: `strings` output
/// matched exactly this key order) and cross-checked with Python's `vdf`
/// library's own `binary_loads`. Read-only — this project never writes
/// `shortcuts.vdf` itself (see `add_profile`'s doc comment for why).
enum VdfValue {
    Str(String),
    /// The numeric value itself is never needed here — only its fixed
    /// 4-byte width, to stay correctly positioned for whatever follows.
    Int,
    Obj(Vec<(String, VdfValue)>),
}

fn read_cstr(bytes: &[u8], pos: &mut usize) -> Option<String> {
    let start = *pos;
    while *pos < bytes.len() && bytes[*pos] != 0 {
        *pos += 1;
    }
    if *pos >= bytes.len() {
        return None;
    }
    let s = String::from_utf8_lossy(&bytes[start..*pos]).into_owned();
    *pos += 1; // skip the null terminator
    Some(s)
}

/// Parses one VDF "object" starting at `*pos`, stopping at (and consuming)
/// its closing `0x08`, or at the end of `bytes` for the outermost call
/// (`shortcuts.vdf` doesn't strictly need a final `0x08` for this to work —
/// running out of bytes ends the object the same way). Malformed/truncated
/// input just ends the object early rather than panicking — this is a
/// best-effort convenience scan (see `slugs_in_steam`'s doc comment), never
/// something that should be able to crash the TUI over a real user's file.
fn parse_object(bytes: &[u8], pos: &mut usize) -> Vec<(String, VdfValue)> {
    let mut entries = Vec::new();
    while *pos < bytes.len() {
        let tag = bytes[*pos];
        *pos += 1;
        if tag == 0x08 {
            break;
        }
        let Some(key) = read_cstr(bytes, pos) else {
            break;
        };
        match tag {
            0x00 => entries.push((key, VdfValue::Obj(parse_object(bytes, pos)))),
            0x01 => {
                let Some(s) = read_cstr(bytes, pos) else {
                    break;
                };
                entries.push((key, VdfValue::Str(s)));
            }
            0x02 => {
                if *pos + 4 > bytes.len() {
                    break;
                }
                *pos += 4;
                entries.push((key, VdfValue::Int));
            }
            _ => break, // unrecognized tag — can't know its size, stop here
        }
    }
    entries
}

fn find_str<'a>(entries: &'a [(String, VdfValue)], key: &str) -> Option<&'a str> {
    entries
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .and_then(|(_, v)| match v {
            VdfValue::Str(s) => Some(s.as_str()),
            _ => None,
        })
}

fn find_obj<'a>(entries: &'a [(String, VdfValue)], key: &str) -> Option<&'a [(String, VdfValue)]> {
    entries
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .and_then(|(_, v)| match v {
            VdfValue::Obj(o) => Some(o.as_slice()),
            _ => None,
        })
}

/// Whether a `shortcuts.vdf` entry with these `exe`/`launch_options` values
/// is one of ours, and if so, which slug it's for. Matches on the `exe`
/// field's file stem being exactly `iprolaunch` (tolerant of the binary
/// having moved/been rebuilt since the entry was added — matches by name,
/// not the exact absolute path) and the last whitespace-separated token of
/// `LaunchOptions` (any gamescope flags baked in via `add_profile`'s
/// `with_gamescope_flags` always come *before* the slug, per
/// `wrapper_contents`) — both trimmed of the surrounding `"..."` Steam
/// itself adds around each Launch-Options token (confirmed for real: a
/// freshly-added shortcut's own Launch Options box literally shows
/// `"<slug>"`, quotes included).
fn matching_slug(exe: &str, launch_options: &str) -> Option<String> {
    let exe_stem = Path::new(exe.trim_matches('"')).file_stem()?.to_str()?;
    if exe_stem != "iprolaunch" {
        return None;
    }
    let last = launch_options.split_whitespace().last()?;
    Some(last.trim_matches('"').to_string())
}

/// Every `userdata/<account-id>/config/shortcuts.vdf` across every known
/// Steam install (`proton::steam_roots()` — the exact same root-detection
/// `proton list`/the proton picker already use) — usually just one, but
/// every account found is scanned without trying to single out "the active
/// one": this is read-only and low-stakes, a slug detected in any account's
/// file still correctly means "already added somewhere".
fn shortcuts_vdf_paths() -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for root in crate::proton::steam_roots()? {
        let userdata = root.join("userdata");
        let Ok(entries) = fs::read_dir(&userdata) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                let candidate = entry.path().join("config").join("shortcuts.vdf");
                if candidate.is_file() {
                    paths.push(candidate);
                }
            }
        }
    }
    Ok(paths)
}

/// The pure extraction step of `slugs_in_steam`, kept separate so it's
/// unit-testable against a hand-built byte fixture instead of a real
/// `shortcuts.vdf` (see the project's standing rule against tests touching
/// real `project_dirs()`-resolved *or* real external app paths).
fn slugs_from_shortcuts_bytes(bytes: &[u8]) -> HashSet<String> {
    let mut found = HashSet::new();
    let mut pos = 0;
    let root = parse_object(bytes, &mut pos);
    let Some(shortcuts) = find_obj(&root, "shortcuts") else {
        return found;
    };
    for (_, value) in shortcuts {
        let VdfValue::Obj(entry) = value else {
            continue;
        };
        let (Some(exe), Some(launch_options)) =
            (find_str(entry, "exe"), find_str(entry, "LaunchOptions"))
        else {
            continue;
        };
        if let Some(slug) = matching_slug(exe, launch_options) {
            found.insert(slug);
        }
    }
    found
}

/// Every slug already added to Steam via `add_profile` — read-only,
/// best-effort: a missing/unreadable/malformed `shortcuts.vdf` just means
/// "nothing detected" rather than an error, since this is a convenience
/// marker (the TUI's per-row "S" indicator, and the dedup check before
/// `add_profile`), never something that should block the Library tab from
/// rendering or a fresh add from working.
pub fn slugs_in_steam() -> HashSet<String> {
    let Ok(paths) = shortcuts_vdf_paths() else {
        return HashSet::new();
    };
    let mut found = HashSet::new();
    for path in paths {
        if let Ok(bytes) = fs::read(&path) {
            found.extend(slugs_from_shortcuts_bytes(&bytes));
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapper_contents_reuses_the_exact_quick_launch_command_format() {
        let contents = wrapper_contents(
            "Elden Ring#1",
            Path::new("/opt/iprolaunch"),
            &[],
            "eldenring",
        );
        assert_eq!(
            contents,
            "[Desktop Entry]\nType=Application\nName=Elden Ring#1\nExec=\"/opt/iprolaunch\" eldenring\n"
        );
    }

    #[test]
    fn wrapper_contents_inserts_extra_flags_as_separate_tokens_before_the_slug() {
        let contents = wrapper_contents(
            "Game#1",
            Path::new("/opt/iprolaunch"),
            &["-w", "-b"],
            "game",
        );
        assert!(contents.contains("Exec=\"/opt/iprolaunch\" -w -b game\n"));
    }

    #[test]
    fn wrapper_contents_handles_an_exe_path_containing_a_space() {
        // A naive split-on-first-space splice (an earlier draft of this
        // function) would cut through the quoted exe path itself here.
        let contents = wrapper_contents(
            "Game#1",
            Path::new("/home/user/My Games/iprolaunch"),
            &["-f"],
            "game",
        );
        assert!(contents.contains("Exec=\"/home/user/My Games/iprolaunch\" -f game\n"));
    }

    #[test]
    fn wrapper_contents_collapses_a_newline_in_the_name_to_a_space() {
        // `.desktop` keys are single-line — a literal newline inside `Name=`
        // would corrupt the file (the rest would be read as a new key).
        let contents = wrapper_contents("weird\nname", Path::new("/opt/iprolaunch"), &[], "game");
        let name_line = contents.lines().nth(2).expect("Name= is the 3rd line");
        assert_eq!(name_line, "Name=weird name");
    }

    #[test]
    fn percent_encode_path_leaves_unreserved_characters_bare() {
        assert_eq!(percent_encode_path(Path::new("abc-_.~XYZ")), "abc-_.~XYZ");
    }

    #[test]
    fn percent_encode_path_encodes_slashes_and_spaces() {
        assert_eq!(
            percent_encode_path(Path::new("/home/user/my game.desktop")),
            "%2Fhome%2Fuser%2Fmy%20game.desktop"
        );
    }

    #[test]
    fn steam_url_has_the_real_scheme_and_the_encoded_path() {
        assert_eq!(
            steam_url(Path::new("/tmp/game.desktop")),
            "steam://addnonsteamgame/%2Ftmp%2Fgame.desktop"
        );
    }

    /// Builds a minimal but structurally real binary-VDF `shortcuts.vdf`:
    /// one `shortcuts` object holding one numbered entry with `exe`/
    /// `LaunchOptions` string fields (each stored the way Steam itself
    /// stores them, `"..."`-quoted) plus an `appid` int32 field between
    /// them — exercising that the parser correctly skips a fixed-size int
    /// field and keeps the rest in sync, exactly like a real file's
    /// `appid`/`IsHidden`/etc. fields do.
    fn fake_shortcuts_vdf(exe: &str, launch_options: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.push(0x00);
        bytes.extend(b"shortcuts\0");
        bytes.push(0x00);
        bytes.extend(b"0\0");
        bytes.push(0x02);
        bytes.extend(b"appid\0");
        bytes.extend(42i32.to_le_bytes());
        bytes.push(0x01);
        bytes.extend(b"appname\0");
        bytes.extend(b"Some Game\0");
        bytes.push(0x01);
        bytes.extend(b"exe\0");
        bytes.extend(exe.as_bytes());
        bytes.push(0);
        bytes.push(0x01);
        bytes.extend(b"LaunchOptions\0");
        bytes.extend(launch_options.as_bytes());
        bytes.push(0);
        bytes.push(0x08); // close "0"
        bytes.push(0x08); // close "shortcuts"
        bytes
    }

    #[test]
    fn parse_object_reads_nested_strings_across_an_interleaved_int_field() {
        let bytes = fake_shortcuts_vdf("\"/opt/iprolaunch\"", "\"ktsysview\"");
        let mut pos = 0;
        let root = parse_object(&bytes, &mut pos);
        let shortcuts = find_obj(&root, "shortcuts").expect("shortcuts object");
        let (_, VdfValue::Obj(entry)) = &shortcuts[0] else {
            panic!("expected the first shortcut to be an object")
        };
        assert_eq!(find_str(entry, "appname"), Some("Some Game"));
        assert_eq!(find_str(entry, "exe"), Some("\"/opt/iprolaunch\""));
        assert_eq!(find_str(entry, "LaunchOptions"), Some("\"ktsysview\""));
    }

    #[test]
    fn matching_slug_strips_quotes_and_requires_an_iprolaunch_exe() {
        assert_eq!(
            matching_slug("\"/opt/iprolaunch\"", "\"ktsysview\""),
            Some("ktsysview".to_string())
        );
        // gamescope flags baked in before the slug — only the last token counts.
        assert_eq!(
            matching_slug("\"/opt/iprolaunch\"", "\"-w\" \"ktsysview\""),
            Some("ktsysview".to_string())
        );
        // A totally unrelated shortcut (not ours) never matches.
        assert_eq!(
            matching_slug("\"/opt/some-other-game.exe\"", "MANGOHUD=1 %command%"),
            None
        );
    }

    #[test]
    fn slugs_from_shortcuts_bytes_extracts_the_slug_from_a_real_shaped_file() {
        let bytes = fake_shortcuts_vdf("\"/home/user/iprolaunch\"", "\"eldenring\"");
        let found = slugs_from_shortcuts_bytes(&bytes);
        assert!(found.contains("eldenring"));
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn slugs_from_shortcuts_bytes_is_empty_for_garbage_input() {
        assert!(slugs_from_shortcuts_bytes(b"not a real vdf file at all").is_empty());
        assert!(slugs_from_shortcuts_bytes(b"").is_empty());
    }
}
