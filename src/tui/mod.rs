mod app;
mod config;
mod library;
mod profile_editor;
mod running;
mod ui;

use std::io::{self, IsTerminal, Stdout};
use std::time::Duration;

use anyhow::{Result, bail};
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
    // Bare `iprolaunch` with no controlling terminal (confirmed for real:
    // Steam Game Mode launches a non-Steam-game shortcut this way) makes
    // `enable_raw_mode()` below fail with a bare, cryptic OS error
    // ("No such device or address (os error 6)") that's invisible there
    // anyway (no console shown) — this at least prints something
    // actionable if stderr *is* visible (e.g. run from a script), and the
    // exit is the same either way. Checked up front rather than after
    // already touching the real terminal, so there's nothing to undo.
    if !io::stdout().is_terminal() {
        bail!(
            "no terminal available to run the TUI in (this happens when launched without a \
             console, e.g. a Steam Game Mode/gamescope shortcut) — point it at \
             `iprolaunch <name-or-slug>` instead"
        );
    }

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
        Mode::ConfirmRenameSlug { .. } => {
            return profile_editor::confirm_rename_slug_key(app, code);
        }
        Mode::Help => return help_popup_key(app, code),
        Mode::Normal => {}
    }

    // While actively typing a Running/Library quick-search (not yet
    // locked by Enter), every key goes straight to it — including digits/
    // q/?/Tab, which the global shortcuts below would otherwise swallow
    // before the filter ever saw them (e.g. searching a game named
    // "Dark Souls 3"). Once locked (Enter pressed, or no filter at all),
    // these fall through to the normal global-key handling below, same as
    // ever.
    if app.tab == app::Tab::Running && app.running_filter_editing {
        return running::on_key(app, code);
    }
    if app.tab == app::Tab::Library && app.library_filter_editing {
        return library::on_key(app, code, terminal);
    }

    match code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char('1') => {
            app.clear_filters();
            app.tab = app::Tab::Running;
        }
        KeyCode::Char('2') => {
            app.clear_filters();
            app.tab = app::Tab::Library;
        }
        KeyCode::Char('3') => {
            app.clear_filters();
            app.tab = app::Tab::Config;
        }
        KeyCode::Char('4') => {
            app.clear_filters();
            app.tab = app::Tab::Help;
        }
        KeyCode::Char('?') => app.mode = Mode::Help,
        KeyCode::Tab => app.next_tab(),
        KeyCode::BackTab => app.prev_tab(),
        _ => match app.tab {
            app::Tab::Running => running::on_key(app, code),
            app::Tab::Library => library::on_key(app, code, terminal),
            app::Tab::Config => config::on_key(app, code, terminal),
            app::Tab::Help => help_scroll_key(app, code),
        },
    }
}

/// Esc closes the `?` popup (back to `Mode::Normal`); everything else is
/// just scrolling, shared with the Help tab itself.
fn help_popup_key(app: &mut App, code: KeyCode) {
    if code == KeyCode::Esc {
        app.mode = Mode::Normal;
        return;
    }
    help_scroll_key(app, code);
}

/// Up/Down by one line, PageUp/PageDown by a full page, Home/End to jump to
/// either end — used both by the Help tab (`Mode::Normal`) and the `?`
/// popup (`Mode::Help`). Soft-clamped against the *total* line count here;
/// `ui::draw_help`/`draw_help_popup` additionally clamp against the actual
/// visible height at render time, so an exact bound here isn't needed.
fn help_scroll_key(app: &mut App, code: KeyCode) {
    let total = ui::help_text_line_count();
    match code {
        KeyCode::Up => app.help_scroll = app.help_scroll.saturating_sub(1),
        KeyCode::Down => app.help_scroll = (app.help_scroll + 1).min(total),
        KeyCode::PageUp => app.help_scroll = app.help_scroll.saturating_sub(10),
        KeyCode::PageDown => app.help_scroll = (app.help_scroll + 10).min(total),
        KeyCode::Home => app.help_scroll = 0,
        KeyCode::End => app.help_scroll = total,
        _ => {}
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

    #[test]
    fn esc_closes_the_help_popup() {
        let mut app = App::new(crate::config::Config::default());
        app.mode = Mode::Help;
        help_popup_key(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn down_and_up_move_help_scroll_by_one_line() {
        let mut app = App::new(crate::config::Config::default());
        help_scroll_key(&mut app, KeyCode::Down);
        help_scroll_key(&mut app, KeyCode::Down);
        assert_eq!(app.help_scroll, 2);
        help_scroll_key(&mut app, KeyCode::Up);
        assert_eq!(app.help_scroll, 1);
    }

    #[test]
    fn up_at_the_top_does_not_go_negative() {
        let mut app = App::new(crate::config::Config::default());
        help_scroll_key(&mut app, KeyCode::Up);
        assert_eq!(app.help_scroll, 0);
    }

    #[test]
    fn page_down_and_page_up_move_by_ten_lines() {
        let mut app = App::new(crate::config::Config::default());
        help_scroll_key(&mut app, KeyCode::PageDown);
        assert_eq!(app.help_scroll, 10);
        help_scroll_key(&mut app, KeyCode::PageUp);
        assert_eq!(app.help_scroll, 0);
    }

    #[test]
    fn end_jumps_to_the_bottom_and_home_back_to_the_top() {
        let mut app = App::new(crate::config::Config::default());
        help_scroll_key(&mut app, KeyCode::End);
        assert_eq!(app.help_scroll, ui::help_text_line_count());
        help_scroll_key(&mut app, KeyCode::Home);
        assert_eq!(app.help_scroll, 0);
    }

    #[test]
    fn down_does_not_scroll_past_the_total_line_count() {
        let mut app = App::new(crate::config::Config::default());
        app.help_scroll = ui::help_text_line_count();
        help_scroll_key(&mut app, KeyCode::Down);
        assert_eq!(app.help_scroll, ui::help_text_line_count());
    }
}
