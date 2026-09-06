use std::io::{self, Write};
use std::path::Path;
use std::process::Command;

use crossterm::event::KeyCode;

use super::app::{self, App, Mode, TextInputPurpose};
use super::{Term, resume, suspend};
use crate::config::Profile;
use crate::launch::{self, RunOptions};
use crate::{prefix, proton};

pub fn on_key(app: &mut App, code: KeyCode, terminal: &mut Term) {
    if let Some(slug) = app.profile_editor.clone() {
        return super::profile_editor::on_key(app, code, slug);
    }
    if app.library_filter_editing {
        return filter_key(app, code);
    }

    match code {
        KeyCode::Up => {
            let len = app.filtered_profile_indices().len();
            app.library_selected = app::move_selection(app.library_selected, len, -1);
        }
        KeyCode::Down => {
            let len = app.filtered_profile_indices().len();
            app.library_selected = app::move_selection(app.library_selected, len, 1);
        }
        KeyCode::Char('a') => {
            app.mode = Mode::TextInput {
                purpose: TextInputPurpose::AddLibraryPath,
                buffer: String::new(),
                cursor: 0,
            };
        }
        KeyCode::Char('r') => refresh(app),
        KeyCode::Char('e') => edit_selected(app),
        KeyCode::Char('d') | KeyCode::Delete => prompt_delete_selected(app),
        KeyCode::Char('f') => start_filter(app),
        KeyCode::Char('c') => copy_quick_launch(app),
        KeyCode::Char('p') => prompt_winetricks(app),
        KeyCode::Char('s') => prompt_add_to_steam(app),
        KeyCode::Enter => launch_selected(app, terminal),
        // Only meaningful once a filter is locked (still-typing Esc is
        // handled by `edit_filter` instead, via the early return above) —
        // a no-op otherwise, same as Esc always was here.
        KeyCode::Esc if app.library_filter.is_some() => app.clear_library_filter(),
        _ => {}
    }
}

fn start_filter(app: &mut App) {
    app.library_filter = Some(String::new());
    app.library_filter_editing = true;
    app.library_selected = 0;
}

/// The parts of quick-search editing that never need `terminal` — kept
/// separate so they're unit-testable without a real `Term` (same pattern
/// as `tui::edit_text_buffer`). Returns `true` if `code` was handled here
/// (so the caller shouldn't fall through to the Enter-launches handling).
fn edit_filter(app: &mut App, code: KeyCode) -> bool {
    match code {
        KeyCode::Esc => {
            app.library_filter = None;
            app.library_selected = 0;
            true
        }
        KeyCode::Backspace => {
            if let Some(filter) = &mut app.library_filter {
                filter.pop();
            }
            app.library_selected = 0;
            true
        }
        KeyCode::Char(c) => {
            if let Some(filter) = &mut app.library_filter {
                filter.push(c);
            }
            app.library_selected = 0;
            true
        }
        KeyCode::Up => {
            let len = app.filtered_profile_indices().len();
            app.library_selected = app::move_selection(app.library_selected, len, -1);
            true
        }
        KeyCode::Down => {
            let len = app.filtered_profile_indices().len();
            app.library_selected = app::move_selection(app.library_selected, len, 1);
            true
        }
        _ => false,
    }
}

/// Keys while actively typing the quick-search box (`f` was pressed, not
/// yet locked): typing edits the filter text directly (so `a`/`r`/`e`/`d` —
/// this tab's own shortcuts — are unavailable while typing, since they're
/// needed as literal characters instead), Up/Down navigate the filtered
/// subset, Esc clears the filter entirely. Enter *locks* it instead of
/// launching — the filtered view stays exactly as it is, but every other
/// key (including this same Enter, next press) goes back to meaning what
/// it normally does, now scoped to the filtered subset (see `on_key`'s
/// `library_filter_editing` check).
fn filter_key(app: &mut App, code: KeyCode) {
    if edit_filter(app, code) {
        return;
    }
    if code == KeyCode::Enter {
        app.library_filter_editing = false;
    }
}

fn refresh(app: &mut App) {
    app.refresh_profiles();
    app.status = Some("Refreshed.".to_string());
}

fn copy_quick_launch(app: &mut App) {
    let Some((slug, _)) = selected_slug_and_profile(app) else {
        return;
    };
    app.status = Some(match crate::quick_launch_cmd::copy_for_slug(&slug) {
        Ok(command) => format!("Copied: {command}"),
        Err(err) => format!("{err:#}"),
    });
}

fn prompt_winetricks(app: &mut App) {
    let Some((slug, profile)) = selected_slug_and_profile(app) else {
        return;
    };
    app.mode = Mode::ConfirmWinetricks {
        slug,
        name: profile.name,
    };
}

/// Keys while `Mode::ConfirmWinetricks` is up: `y` confirms, anything else
/// (including Esc) cancels. Not a one-keystroke action since it launches an
/// external GUI tool — a stray `p` shouldn't fire it silently.
pub fn confirm_winetricks_key(app: &mut App, code: KeyCode, terminal: &mut Term) {
    let Mode::ConfirmWinetricks { slug, name } = &app.mode else {
        return;
    };
    match code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            let slug = slug.clone();
            let name = name.clone();
            app.mode = Mode::Normal;
            run_winetricks(app, terminal, &slug, &name);
        }
        _ => app.mode = Mode::Normal,
    }
}

/// Runs `winetricks` against the *exact* prefix a normal launch of `slug`
/// would use — resolved through the same `Config::effective`/`prefix::resolve`
/// path as `launch::run`, so there's no chance of it drifting onto a
/// different prefix than the game itself runs in. In `Single` prefix mode a
/// profile's own `proton` override is ignored (mirrors `Config::effective`),
/// so this always resolves to the one shared prefix/Proton pair regardless
/// of which profile was selected when `p` was pressed.
fn run_winetricks(app: &mut App, terminal: &mut Term, slug: &str, name: &str) {
    let Some(profile) = app
        .profiles
        .iter()
        .find(|(s, _)| s == slug)
        .map(|(_, p)| p.clone())
    else {
        return;
    };
    let effective = app.cfg.effective(Some(&profile));
    let prefix_path = prefix::resolve(&effective, slug);
    let proton_dir = match proton::resolve_binary_dir(&effective.proton) {
        Ok(dir) => dir,
        Err(err) => {
            app.status = Some(format!("Can't run winetricks: {err:#}"));
            return;
        }
    };
    let wine = proton_dir.join("files/bin/wine");
    let wineserver = proton_dir.join("files/bin/wineserver");

    if suspend(terminal).is_err() {
        app.status = Some("Failed to suspend the TUI for winetricks.".to_string());
        return;
    }

    println!("Launching winetricks for {name}...");
    let result = Command::new("winetricks")
        .env("WINEPREFIX", &prefix_path)
        .env("WINE", &wine)
        .env("WINESERVER", &wineserver)
        .status();
    match &result {
        Ok(status) if status.success() => println!("\nwinetricks exited normally."),
        Ok(status) => println!("\nwinetricks exited with {status}."),
        Err(err) => println!("\nfailed to launch winetricks: {err}"),
    }
    print!("\nPress Enter to return to iprolaunch. ");
    io::stdout().flush().ok();
    let mut discard = String::new();
    io::stdin().read_line(&mut discard).ok();

    if resume(terminal).is_err() {
        app.status = Some("Failed to restore the TUI after winetricks.".to_string());
        return;
    }

    app.status = Some(match result {
        Ok(status) if status.success() => format!("winetricks for {name} exited normally."),
        Ok(status) => format!("winetricks for {name} exited with {status}."),
        Err(err) => format!("failed to launch winetricks for {name}: {err:#}"),
    });
}

/// Library `s`: opens `Mode::ConfirmAddToSteam` for the selected profile —
/// unless `app.steam_slugs` (populated at startup/refresh, see
/// `App::refresh_steam_status`) already lists its slug, in which case this
/// just reports it instead (there's no `steam://` URL to update an existing
/// shortcut, only to add a new — necessarily duplicate — one, so re-adding
/// isn't offered at all, per the user's own call on this).
fn prompt_add_to_steam(app: &mut App) {
    let Some((slug, profile)) = selected_slug_and_profile(app) else {
        return;
    };
    if app.steam_slugs.contains(&slug) {
        app.status = Some(format!(
            "\"{}\" already looks added to Steam — remove it there first if you want to re-add.",
            profile.name
        ));
        return;
    }
    app.mode = Mode::ConfirmAddToSteam {
        slug,
        name: profile.name,
        selected: 0,
    };
}

/// Keys while `Mode::ConfirmAddToSteam` is up: Up/Down move the selection
/// among `app::CONFIRM_ADD_TO_STEAM_OPTIONS` (wrapping via the same
/// `app::move_selection` every other list uses — already gamepad-ready via
/// the D-pad, no new mapping needed for navigation itself), Enter activates
/// whichever's highlighted, Esc always cancels regardless of selection.
pub fn confirm_add_to_steam_key(app: &mut App, code: KeyCode) {
    let Mode::ConfirmAddToSteam {
        slug,
        name,
        selected,
    } = &app.mode
    else {
        return;
    };
    match code {
        KeyCode::Up => {
            let selected =
                app::move_selection(*selected, app::CONFIRM_ADD_TO_STEAM_OPTIONS.len(), -1);
            if let Mode::ConfirmAddToSteam { selected: s, .. } = &mut app.mode {
                *s = selected;
            }
        }
        KeyCode::Down => {
            let selected =
                app::move_selection(*selected, app::CONFIRM_ADD_TO_STEAM_OPTIONS.len(), 1);
            if let Mode::ConfirmAddToSteam { selected: s, .. } = &mut app.mode {
                *s = selected;
            }
        }
        KeyCode::Enter => {
            let (slug, name, selected) = (slug.clone(), name.clone(), *selected);
            app.mode = Mode::Normal;
            match selected {
                0 => add_to_steam(app, &slug, &name, false),
                1 => add_to_steam(app, &slug, &name, true),
                _ => {} // "Cancel" (or anything out of range) — do nothing
            }
        }
        KeyCode::Esc => app.mode = Mode::Normal,
        _ => {}
    }
}

/// Actually calls `steam_shortcut::add_profile` and reports the result —
/// re-checks `app.profiles` for the profile fresh (rather than trusting the
/// clone captured when the popup opened) since it's still cheap and this
/// only runs once, on confirm.
///
/// The `refresh_steam_status()` right after a successful add is
/// best-effort, not a guarantee the "S" marker shows up *immediately*:
/// `steam_shortcut::add_profile`'s `steam <url>` call only waits for the
/// short-lived launcher process that hands the URL to Steam's own
/// already-running client, not for that client to actually finish parsing
/// the wrapper and writing `shortcuts.vdf` — confirmed for real, that write
/// can trail the launcher's own exit by up to roughly a second. A stray `r`
/// (or just waiting a moment) picks it up if this particular refresh ran
/// too early.
fn add_to_steam(app: &mut App, slug: &str, name: &str, with_gamescope_flags: bool) {
    let Some(profile) = app.profile(slug).cloned() else {
        app.status = Some(format!("couldn't find profile \"{name}\" to add"));
        return;
    };
    app.status = Some(
        match crate::steam_shortcut::add_profile(&app.cfg, &profile, slug, with_gamescope_flags) {
            Ok(_) => {
                app.refresh_steam_status();
                format!("Sent \"{name}\" to Steam — check your Steam library.")
            }
            Err(err) => format!("couldn't add \"{name}\" to Steam: {err:#}"),
        },
    );
}

/// Every action below looks the currently-selected entry up through
/// `filtered_profile_indices()` rather than indexing `app.profiles`
/// directly with `library_selected` — when a filter is active,
/// `library_selected` is a position in the *filtered* list, not the real
/// one.
fn selected_slug_and_profile(app: &App) -> Option<(String, Profile)> {
    let indices = app.filtered_profile_indices();
    let &real_index = indices.get(app.library_selected)?;
    app.profiles.get(real_index).cloned()
}

fn edit_selected(app: &mut App) {
    if let Some((slug, _)) = selected_slug_and_profile(app) {
        app.profile_editor = Some(slug);
        app.profile_field_selected = 0;
    }
}

fn prompt_delete_selected(app: &mut App) {
    if let Some((slug, profile)) = selected_slug_and_profile(app) {
        app.mode = Mode::ConfirmDeleteProfile {
            slug,
            name: profile.name,
        };
    }
}

/// Keys while `Mode::ConfirmDeleteProfile` is up: `y` confirms, anything
/// else (including Esc) cancels — deleting a profile can't be undone by
/// just backing out like every other edit here, so this is deliberately not
/// a one-keystroke action from the plain list.
pub fn confirm_delete_key(app: &mut App, code: KeyCode) {
    let Mode::ConfirmDeleteProfile { slug, name } = &app.mode else {
        return;
    };
    match code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            let slug = slug.clone();
            let name = name.clone();
            app.mode = Mode::Normal;
            delete_profile(app, &slug, &name);
        }
        _ => app.mode = Mode::Normal,
    }
}

/// Removes `~/.config/iprolaunch/profiles/<slug>/` entirely — the exe
/// itself is never touched, only IProLaunch's own settings/history for it.
fn delete_profile(app: &mut App, slug: &str, name: &str) {
    let dir = match Profile::profiles_dir() {
        Ok(dir) => dir.join(slug),
        Err(err) => {
            app.status = Some(format!("couldn't resolve profiles dir: {err:#}"));
            return;
        }
    };
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => {
            app.status = Some(format!("Deleted \"{name}\"."));
            if app.profile_editor.as_deref() == Some(slug) {
                app.profile_editor = None;
            }
            app.refresh_profiles();
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            app.status = Some(format!("\"{name}\" was already gone."));
            app.refresh_profiles();
        }
        Err(err) => app.status = Some(format!("couldn't delete \"{name}\": {err}")),
    }
}

fn launch_selected(app: &mut App, terminal: &mut Term) {
    let Some((_, profile)) = selected_slug_and_profile(app) else {
        return;
    };
    let target = profile.target_path;
    let name = profile.name;
    launch_path(app, terminal, &target, &name);
}

/// Suspends the TUI (leaves the alternate screen so `umu-run`'s own output
/// is visible normally), runs the launch synchronously, waits for the user
/// to acknowledge the result, then restores the TUI. Used both for
/// launching an existing library entry and for `a` (add-by-path), since
/// adding a game *is* just running it once (`launch::run` auto-creates the
/// profile — same as the CLI's `run` subcommand).
pub fn launch_path(app: &mut App, terminal: &mut Term, target: &str, label: &str) {
    let is_new = !app.profiles.iter().any(|(_, p)| p.target_path == target);

    if suspend(terminal).is_err() {
        app.status = Some("Failed to suspend the TUI for launch.".to_string());
        return;
    }

    println!("Launching {label}...");
    let result = launch::run(&app.cfg, Path::new(target), RunOptions::default());
    match &result {
        Ok(()) => println!("\n{label} exited normally."),
        Err(err) => println!("\n{label} failed: {err:#}"),
    }
    print!("\nPress Enter to return to iprolaunch. ");
    io::stdout().flush().ok();
    let mut discard = String::new();
    io::stdin().read_line(&mut discard).ok();

    if resume(terminal).is_err() {
        app.status = Some("Failed to restore the TUI after launch.".to_string());
        return;
    }

    app.status = Some(match result {
        Ok(()) => format!("{label} exited normally."),
        Err(err) => format!("{label} failed: {err:#}"),
    });
    app.refresh_profiles();
    app.refresh_running();

    // `launch::run` auto-creates (and saves) the profile before it even
    // checks the exit status, so it exists here regardless of whether the
    // launch itself succeeded — prompt for a title either way, since that's
    // what a fresh add is missing (weak exe-stem-only GAMEID matching until
    // one's set).
    if is_new
        && let Some((slug, profile)) = app.profiles.iter().find(|(_, p)| p.target_path == target)
        && profile.title.is_none()
    {
        app.mode = Mode::TextInput {
            purpose: TextInputPurpose::ProfileTitle(slug.clone()),
            buffer: String::new(),
            cursor: 0,
        };
    }
}

/// Applies the follow-up title prompt after a fresh add-by-path. An empty
/// buffer (Enter with nothing typed, same as Esc) just skips — the profile
/// keeps `title: None`, exactly as if this prompt didn't exist.
pub fn apply_profile_title(app: &mut App, slug: &str, buffer: String) {
    if buffer.trim().is_empty() {
        return;
    }
    let Ok(mut profile) = Profile::load(slug) else {
        app.status = Some(format!("couldn't load profile \"{slug}\" to set its title"));
        return;
    };
    profile.title = Some(buffer.trim().to_string());
    match profile.save(slug) {
        Ok(()) => {
            app.status = Some(format!("Title saved for \"{slug}\"."));
            app.refresh_profiles();
        }
        Err(err) => app.status = Some(format!("couldn't save title: {err:#}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    // Every test below only exercises paths that never call `Profile::save`
    // or remove a real directory (`app.cfg.save`/`profile.save`/
    // `fs::remove_dir_all` all write to or delete real files under
    // `~/.config/iprolaunch/`, which nothing here has a way to redirect —
    // same rule as `tui::config`'s tests). `confirm_delete_key`'s `y` arm
    // and `delete_profile` are deliberately NOT covered here; verified by
    // hand instead (see project NOTES.md). Same rule for
    // `confirm_add_to_steam_key`'s selected-0/1 arms and `add_to_steam`
    // itself — both would shell out to the real `steam` binary and write a
    // real file under `~/.local/share/iprolaunch/`.

    fn test_app_with_profile(slug: &str, name: &str) -> App {
        let mut app = App::new(Config::default());
        app.profiles = vec![(
            slug.to_string(),
            Profile {
                name: name.to_string(),
                target_path: format!("/tmp/{slug}.exe"),
                title: None,
                last_launched: None,
                args: Vec::new(),
                defaults: Default::default(),
                logging: Default::default(),
                env: Default::default(),
                winedlloverride: Default::default(),
            },
        )];
        app.library_selected = 0;
        app
    }

    #[test]
    fn e_opens_the_profile_editor_for_the_selected_slug() {
        let mut app = test_app_with_profile("game-1", "Game#1");
        edit_selected(&mut app);
        assert_eq!(app.profile_editor.as_deref(), Some("game-1"));
        assert_eq!(app.profile_field_selected, 0);
    }

    #[test]
    fn s_opens_the_confirm_popup_for_a_profile_not_yet_in_steam() {
        let mut app = test_app_with_profile("game-1", "Game#1");
        app.steam_slugs.clear();
        prompt_add_to_steam(&mut app);
        match &app.mode {
            Mode::ConfirmAddToSteam {
                slug,
                name,
                selected,
            } => {
                assert_eq!(slug, "game-1");
                assert_eq!(name, "Game#1");
                assert_eq!(*selected, 0);
            }
            _ => panic!("expected ConfirmAddToSteam"),
        }
    }

    #[test]
    fn s_just_reports_already_added_without_opening_a_popup() {
        let mut app = test_app_with_profile("game-1", "Game#1");
        app.steam_slugs.insert("game-1".to_string());
        prompt_add_to_steam(&mut app);
        assert!(matches!(app.mode, Mode::Normal));
        assert!(
            app.status
                .as_deref()
                .unwrap()
                .contains("already looks added")
        );
    }

    #[test]
    fn confirm_add_to_steam_up_down_wrap_within_the_three_options() {
        let mut app = test_app_with_profile("game-1", "Game#1");
        app.mode = Mode::ConfirmAddToSteam {
            slug: "game-1".to_string(),
            name: "Game#1".to_string(),
            selected: 0,
        };
        confirm_add_to_steam_key(&mut app, KeyCode::Up); // already at 0
        assert!(matches!(
            app.mode,
            Mode::ConfirmAddToSteam { selected: 0, .. }
        ));
        confirm_add_to_steam_key(&mut app, KeyCode::Down);
        confirm_add_to_steam_key(&mut app, KeyCode::Down);
        assert!(matches!(
            app.mode,
            Mode::ConfirmAddToSteam { selected: 2, .. }
        ));
        confirm_add_to_steam_key(&mut app, KeyCode::Down); // already at the last option
        assert!(matches!(
            app.mode,
            Mode::ConfirmAddToSteam { selected: 2, .. }
        ));
    }

    #[test]
    fn confirm_add_to_steam_esc_cancels_without_touching_anything() {
        let mut app = test_app_with_profile("game-1", "Game#1");
        app.mode = Mode::ConfirmAddToSteam {
            slug: "game-1".to_string(),
            name: "Game#1".to_string(),
            selected: 1,
        };
        confirm_add_to_steam_key(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn confirm_add_to_steam_enter_on_cancel_returns_to_normal_without_side_effects() {
        // Selecting "Cancel" (index 2) and pressing Enter must behave
        // exactly like Esc — never reach `add_to_steam` (which would shell
        // out to the real `steam` binary and write a real file under
        // `~/.local/share/iprolaunch/`, neither of which anything here can
        // safely redirect — same rule as `confirm_delete_key`'s `y` arm).
        let mut app = test_app_with_profile("game-1", "Game#1");
        app.mode = Mode::ConfirmAddToSteam {
            slug: "game-1".to_string(),
            name: "Game#1".to_string(),
            selected: 2,
        };
        confirm_add_to_steam_key(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Normal));
        assert_eq!(app.status, None);
    }

    #[test]
    fn d_opens_a_confirm_prompt_naming_the_selected_profile() {
        let mut app = test_app_with_profile("game-1", "Game#1");
        prompt_delete_selected(&mut app);
        match &app.mode {
            Mode::ConfirmDeleteProfile { slug, name } => {
                assert_eq!(slug, "game-1");
                assert_eq!(name, "Game#1");
            }
            _ => panic!("expected ConfirmDeleteProfile"),
        }
    }

    #[test]
    fn confirm_delete_cancels_on_anything_but_y_without_touching_disk() {
        let mut app = test_app_with_profile("game-1", "Game#1");
        app.mode = Mode::ConfirmDeleteProfile {
            slug: "game-1".to_string(),
            name: "Game#1".to_string(),
        };
        confirm_delete_key(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Normal));
        // Still there — cancelling must not have deleted anything.
        assert_eq!(app.profiles.len(), 1);
    }

    #[test]
    fn r_refreshes_without_crashing() {
        // refresh_profiles() re-reads the real (but untouched-by-this-test)
        // profiles dir — read-only, so safe to call here (same as
        // `App::new` already does for every test in this crate).
        let mut app = test_app_with_profile("game-1", "Game#1");
        refresh(&mut app);
        assert_eq!(app.status.as_deref(), Some("Refreshed."));
    }

    fn test_app_with_two_profiles() -> App {
        let mut app = App::new(Config::default());
        app.profiles = vec![
            (
                "eldenring".to_string(),
                Profile {
                    name: "eldenring#1".to_string(),
                    target_path: "/tmp/eldenring.exe".to_string(),
                    title: None,
                    last_launched: None,
                    args: Vec::new(),
                    defaults: Default::default(),
                    logging: Default::default(),
                    env: Default::default(),
                    winedlloverride: Default::default(),
                },
            ),
            (
                "ktsysview".to_string(),
                Profile {
                    name: "ktsysview#1".to_string(),
                    target_path: "/tmp/ktsysview.exe".to_string(),
                    title: None,
                    last_launched: None,
                    args: Vec::new(),
                    defaults: Default::default(),
                    logging: Default::default(),
                    env: Default::default(),
                    winedlloverride: Default::default(),
                },
            ),
        ];
        app
    }

    #[test]
    fn f_starts_an_empty_filter_and_resets_selection() {
        let mut app = test_app_with_two_profiles();
        app.library_selected = 1;
        start_filter(&mut app);
        assert_eq!(app.library_filter.as_deref(), Some(""));
        assert_eq!(app.library_selected, 0);
    }

    #[test]
    fn typing_narrows_the_filtered_list_case_insensitively() {
        let mut app = test_app_with_two_profiles();
        start_filter(&mut app);
        for c in "KT".chars() {
            edit_filter(&mut app, KeyCode::Char(c));
        }
        assert_eq!(app.library_filter.as_deref(), Some("KT"));
        let indices = app.filtered_profile_indices();
        assert_eq!(indices, vec![1]); // only "ktsysview#1" matches
    }

    #[test]
    fn backspace_removes_the_last_filter_character() {
        let mut app = test_app_with_two_profiles();
        start_filter(&mut app);
        edit_filter(&mut app, KeyCode::Char('x'));
        edit_filter(&mut app, KeyCode::Char('y'));
        edit_filter(&mut app, KeyCode::Backspace);
        assert_eq!(app.library_filter.as_deref(), Some("x"));
    }

    #[test]
    fn esc_clears_the_filter_entirely() {
        let mut app = test_app_with_two_profiles();
        start_filter(&mut app);
        edit_filter(&mut app, KeyCode::Char('k'));
        edit_filter(&mut app, KeyCode::Esc);
        assert_eq!(app.library_filter, None);
        assert_eq!(app.library_selected, 0);
    }

    #[test]
    fn up_down_navigate_within_the_filtered_subset_only() {
        let mut app = test_app_with_two_profiles();
        app.library_filter = Some("elden".to_string()); // only 1 match
        edit_filter(&mut app, KeyCode::Down);
        assert_eq!(app.library_selected, 0, "only one match — nowhere to go");
    }

    #[test]
    fn enter_locks_the_filter_instead_of_launching() {
        let mut app = test_app_with_two_profiles();
        start_filter(&mut app);
        for c in "kt".chars() {
            edit_filter(&mut app, KeyCode::Char(c));
        }
        filter_key(&mut app, KeyCode::Enter);
        assert!(!app.library_filter_editing);
        // Still filtered — locking keeps the narrowed view, not just
        // resets it. `edit_filter` doesn't handle Enter itself (it's
        // `filter_key`'s own job), confirmed by `edit_filter` returning
        // false for it.
        assert_eq!(app.library_filter.as_deref(), Some("kt"));
        assert!(!edit_filter(&mut app, KeyCode::Enter));
    }

    #[test]
    fn once_locked_r_still_means_refresh_not_edit_the_filter() {
        // `on_key` itself needs a real `Term` (not safely constructible in
        // a unit test), but once locked, `library_filter_editing` is what
        // routes `r` to this function instead of back into `filter_key` —
        // exercising that function directly is the meaningful part.
        let mut app = test_app_with_two_profiles();
        start_filter(&mut app);
        filter_key(&mut app, KeyCode::Enter); // locks with an empty filter
        assert!(!app.library_filter_editing);
        refresh(&mut app);
        assert_eq!(app.status.as_deref(), Some("Refreshed."));
        // The filter itself (even though empty) is still in place — only
        // Esc/switching tabs should clear it, not `r`.
        assert!(app.library_filter.is_some());
    }

    #[test]
    fn clear_library_filter_resets_both_the_text_and_the_editing_flag() {
        // What `on_key`'s `KeyCode::Esc if app.library_filter.is_some()`
        // arm calls once a filter's locked — the routing itself needs a
        // real `Term` to exercise directly (see `App::new` test rules),
        // verified by hand instead (project NOTES.md).
        let mut app = test_app_with_two_profiles();
        start_filter(&mut app);
        edit_filter(&mut app, KeyCode::Char('x'));
        filter_key(&mut app, KeyCode::Enter); // locks
        assert!(app.library_filter.is_some());
        app.clear_library_filter();
        assert_eq!(app.library_filter, None);
        assert!(!app.library_filter_editing);
    }
}
