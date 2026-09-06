use std::io::{self, Write};
use std::path::Path;

use crossterm::event::KeyCode;

use super::app::{self, App, Mode, TextInputPurpose};
use super::{Term, resume, suspend};
use crate::config::Profile;
use crate::launch::{self, RunOptions};

pub fn on_key(app: &mut App, code: KeyCode, terminal: &mut Term) {
    if let Some(slug) = app.profile_editor.clone() {
        return super::profile_editor::on_key(app, code, slug);
    }
    if app.library_filter.is_some() {
        return filter_key(app, code, terminal);
    }

    match code {
        KeyCode::Up => {
            app.library_selected =
                app::move_selection(app.library_selected, app.profiles.len(), -1);
        }
        KeyCode::Down => {
            app.library_selected = app::move_selection(app.library_selected, app.profiles.len(), 1);
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
        KeyCode::Char('d') => prompt_delete_selected(app),
        KeyCode::Char('f') => start_filter(app),
        KeyCode::Enter => launch_selected(app, terminal),
        _ => {}
    }
}

fn start_filter(app: &mut App) {
    app.library_filter = Some(String::new());
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

/// Keys while the quick-search box is active (`f` was pressed): typing
/// edits the filter text directly (so `a`/`r`/`e`/`d` — this tab's own
/// shortcuts — are unavailable while filtering, since they're needed as
/// literal characters instead; `Esc` to leave filter mode restores them),
/// Up/Down navigate the filtered subset, Enter still launches the
/// selected one.
fn filter_key(app: &mut App, code: KeyCode, terminal: &mut Term) {
    if edit_filter(app, code) {
        return;
    }
    if code == KeyCode::Enter {
        launch_selected(app, terminal);
    }
}

fn refresh(app: &mut App) {
    app.refresh_profiles();
    app.status = Some("Refreshed.".to_string());
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
    // hand instead (see project NOTES.md).

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
    fn unhandled_keys_fall_through_so_enter_can_still_launch() {
        let mut app = test_app_with_two_profiles();
        start_filter(&mut app);
        assert!(!edit_filter(&mut app, KeyCode::Enter));
    }
}
