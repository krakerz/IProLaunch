mod app;
mod config;
mod library;
mod profile_editor;
mod running;
mod ui;

use std::io::{self, Stdout};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::config::Config;
use app::{App, Mode, TextInputPurpose};

pub type Term = Terminal<CrosstermBackend<Stdout>>;

pub fn run(cfg: Config) -> Result<()> {
    let mut app = App::new(cfg);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Restore the terminal even on panic, so a bug doesn't leave the user's
    // shell stuck in raw mode / the alternate screen.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        default_hook(info);
    }));

    let result = event_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn event_loop(terminal: &mut Term, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|f| ui::draw(f, app))?;

        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                on_key(app, key.code, terminal);
            }
        } else if matches!(app.tab, app::Tab::Running) {
            app.refresh_running();
        }

        if app.should_quit {
            return Ok(());
        }
    }
}

fn on_key(app: &mut App, code: KeyCode, terminal: &mut Term) {
    match &app.mode {
        Mode::TextInput { .. } => return handle_text_input(app, code, terminal),
        Mode::ProtonPicker { .. } => return handle_proton_picker(app, code),
        Mode::MapEditor { .. } => return config::map_editor_key(app, code),
        Mode::MapEntryInput { .. } => return config::map_entry_input_key(app, code),
        Mode::ConfirmDeleteProfile { .. } => return library::confirm_delete_key(app, code),
        Mode::Normal => {}
    }

    match code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char('1') => app.tab = app::Tab::Running,
        KeyCode::Char('2') => app.tab = app::Tab::Library,
        KeyCode::Char('3') => app.tab = app::Tab::Config,
        KeyCode::Char('4') => app.tab = app::Tab::Help,
        KeyCode::Tab => app.next_tab(),
        KeyCode::BackTab => app.prev_tab(),
        _ => match app.tab {
            app::Tab::Running => running::on_key(app, code),
            app::Tab::Library => library::on_key(app, code, terminal),
            app::Tab::Config => config::on_key(app, code, terminal),
            app::Tab::Help => {}
        },
    }
}

/// Cursor movement, insert, and delete for `Mode::TextInput` — everything
/// that never needs `terminal`, kept separate from `handle_text_input` so
/// it's unit-testable without needing a real `Term`. Operates on
/// `buffer.chars()` (char index, not byte offset) throughout. Returns
/// `true` if `code` was handled here (so the caller shouldn't fall through
/// to Enter/Esc handling).
fn edit_text_buffer(app: &mut App, code: KeyCode) -> bool {
    match code {
        KeyCode::Left => {
            if let Mode::TextInput { cursor, .. } = &mut app.mode {
                *cursor = cursor.saturating_sub(1);
            }
            true
        }
        KeyCode::Right => {
            if let Mode::TextInput { buffer, cursor, .. } = &mut app.mode {
                *cursor = (*cursor + 1).min(buffer.chars().count());
            }
            true
        }
        KeyCode::Backspace => {
            if let Mode::TextInput { buffer, cursor, .. } = &mut app.mode
                && *cursor > 0
            {
                let mut chars: Vec<char> = buffer.chars().collect();
                *cursor -= 1;
                chars.remove(*cursor);
                *buffer = chars.into_iter().collect();
            }
            true
        }
        KeyCode::Char(c) => {
            if let Mode::TextInput { buffer, cursor, .. } = &mut app.mode {
                let mut chars: Vec<char> = buffer.chars().collect();
                chars.insert((*cursor).min(chars.len()), c);
                *buffer = chars.into_iter().collect();
                *cursor += 1;
            }
            true
        }
        _ => false,
    }
}

fn handle_text_input(app: &mut App, code: KeyCode, terminal: &mut Term) {
    if code == KeyCode::Esc {
        app.mode = Mode::Normal;
        return;
    }
    if edit_text_buffer(app, code) {
        return;
    }
    if code != KeyCode::Enter {
        return;
    }

    let Mode::TextInput {
        purpose, buffer, ..
    } = std::mem::replace(&mut app.mode, Mode::Normal)
    else {
        return;
    };
    match purpose {
        TextInputPurpose::AddLibraryPath => {
            let label = buffer.clone();
            library::launch_path(app, terminal, &buffer, &label);
        }
        TextInputPurpose::ConfigField(field) => config::apply_text_field(app, field, buffer),
        TextInputPurpose::ProfileTitle(slug) => library::apply_profile_title(app, &slug, buffer),
        TextInputPurpose::ProfileField(slug, field) => {
            profile_editor::apply_text_field(app, &slug, field, buffer)
        }
    }
}

/// `builds.len()` plus 1 for the always-present "system" entry, plus one
/// more for "inherit" when picking a profile override (see
/// `ui::draw_proton_picker_popup`, which renders the same extra row).
fn proton_picker_len(
    builds: &[crate::proton::ProtonBuild],
    target: &app::ProtonPickerTarget,
) -> usize {
    builds.len()
        + if matches!(target, app::ProtonPickerTarget::Profile(_)) {
            2
        } else {
            1
        }
}

fn handle_proton_picker(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Up => {
            if let Mode::ProtonPicker {
                builds,
                selected,
                target,
            } = &mut app.mode
            {
                let len = proton_picker_len(builds, target);
                *selected = app::move_selection(*selected, len, -1);
            }
        }
        KeyCode::Down => {
            if let Mode::ProtonPicker {
                builds,
                selected,
                target,
            } = &mut app.mode
            {
                let len = proton_picker_len(builds, target);
                *selected = app::move_selection(*selected, len, 1);
            }
        }
        KeyCode::Enter => {
            let Mode::ProtonPicker {
                builds,
                selected,
                target,
            } = std::mem::replace(&mut app.mode, Mode::Normal)
            else {
                return;
            };
            config::apply_proton_choice(app, &builds, selected, &target);
        }
        _ => {}
    }
}

/// Leaves the alternate screen and raw mode so a subprocess's own stdout
/// (e.g. `umu-run`'s launch messages) prints normally instead of into a
/// buffer the user can't see.
pub fn suspend(terminal: &mut Term) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

pub fn resume(terminal: &mut Term) -> Result<()> {
    enable_raw_mode()?;
    execute!(terminal.backend_mut(), EnterAlternateScreen)?;
    terminal.hide_cursor()?;
    terminal.clear()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use app::TextInputPurpose;

    fn text_input_app(buffer: &str, cursor: usize) -> App {
        let mut app = App::new(crate::config::Config::default());
        app.mode = Mode::TextInput {
            purpose: TextInputPurpose::AddLibraryPath,
            buffer: buffer.to_string(),
            cursor,
        };
        app
    }

    fn cursor_and_buffer(app: &App) -> (usize, &str) {
        let Mode::TextInput { buffer, cursor, .. } = &app.mode else {
            panic!("expected TextInput")
        };
        (*cursor, buffer)
    }

    #[test]
    fn left_right_move_the_cursor_and_clamp_at_both_ends() {
        let mut app = text_input_app("abc", 1);
        edit_text_buffer(&mut app, KeyCode::Left);
        assert_eq!(cursor_and_buffer(&app).0, 0);
        edit_text_buffer(&mut app, KeyCode::Left); // already at 0
        assert_eq!(cursor_and_buffer(&app).0, 0);

        edit_text_buffer(&mut app, KeyCode::Right);
        edit_text_buffer(&mut app, KeyCode::Right);
        edit_text_buffer(&mut app, KeyCode::Right);
        assert_eq!(cursor_and_buffer(&app).0, 3); // clamped to buffer length
        edit_text_buffer(&mut app, KeyCode::Right);
        assert_eq!(cursor_and_buffer(&app).0, 3);
    }

    #[test]
    fn typing_inserts_at_the_cursor_not_just_at_the_end() {
        // The scenario the user asked for: fix one segment of a path
        // without retyping the whole thing.
        let mut app = text_input_app("a\\c.exe", 2); // cursor right after "a\"
        edit_text_buffer(&mut app, KeyCode::Char('b'));
        let (cursor, buffer) = cursor_and_buffer(&app);
        assert_eq!(buffer, "a\\bc.exe");
        assert_eq!(cursor, 3);
    }

    #[test]
    fn backspace_deletes_before_the_cursor_and_moves_it_back() {
        let mut app = text_input_app("abc", 2); // cursor between 'b' and 'c'
        edit_text_buffer(&mut app, KeyCode::Backspace);
        let (cursor, buffer) = cursor_and_buffer(&app);
        assert_eq!(buffer, "ac");
        assert_eq!(cursor, 1);
    }

    #[test]
    fn backspace_at_the_start_does_nothing() {
        let mut app = text_input_app("abc", 0);
        edit_text_buffer(&mut app, KeyCode::Backspace);
        let (cursor, buffer) = cursor_and_buffer(&app);
        assert_eq!(buffer, "abc");
        assert_eq!(cursor, 0);
    }
}
