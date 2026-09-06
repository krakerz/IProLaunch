use std::io::{self, Write};
use std::path::Path;

use crossterm::event::KeyCode;

use super::app::{self, App, Mode, TextInputPurpose};
use super::{Term, resume, suspend};
use crate::launch::{self, RunOptions};

pub fn on_key(app: &mut App, code: KeyCode, terminal: &mut Term) {
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
            };
        }
        KeyCode::Enter => launch_selected(app, terminal),
        _ => {}
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
}
