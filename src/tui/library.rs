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
        KeyCode::Enter => launch_selected(app, terminal),
        _ => {}
    }
}

fn refresh(app: &mut App) {
    app.refresh_profiles();
    app.status = Some("Refreshed.".to_string());
}

fn edit_selected(app: &mut App) {
    if let Some((slug, _)) = app.profiles.get(app.library_selected) {
        app.profile_editor = Some(slug.clone());
        app.profile_field_selected = 0;
    }
}

fn prompt_delete_selected(app: &mut App) {
    if let Some((slug, profile)) = app.profiles.get(app.library_selected) {
        app.mode = Mode::ConfirmDeleteProfile {
            slug: slug.clone(),
            name: profile.name.clone(),
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
    let Some((_, profile)) = app.profiles.get(app.library_selected) else {
        return;
    };
    let target = profile.target_path.clone();
    let name = profile.name.clone();
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
}
