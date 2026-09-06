mod app;
mod config;
mod library;
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
            app::Tab::Config => config::on_key(app, code),
            app::Tab::Help => {}
        },
    }
}

fn handle_text_input(app: &mut App, code: KeyCode, terminal: &mut Term) {
    match code {
        KeyCode::Esc => {
            app.mode = Mode::Normal;
            return;
        }
        KeyCode::Backspace => {
            if let Mode::TextInput { buffer, .. } = &mut app.mode {
                buffer.pop();
            }
            return;
        }
        KeyCode::Char(c) => {
            if let Mode::TextInput { buffer, .. } = &mut app.mode {
                buffer.push(c);
            }
            return;
        }
        KeyCode::Enter => {}
        _ => return,
    }

    let Mode::TextInput { purpose, buffer } = std::mem::replace(&mut app.mode, Mode::Normal) else {
        return;
    };
    match purpose {
        TextInputPurpose::AddLibraryPath => {
            let label = buffer.clone();
            library::launch_path(app, terminal, &buffer, &label);
        }
        TextInputPurpose::ConfigField(field) => config::apply_text_field(app, field, buffer),
    }
}

fn handle_proton_picker(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Up => {
            if let Mode::ProtonPicker { builds, selected } = &mut app.mode {
                *selected = app::move_selection(*selected, builds.len() + 1, -1);
            }
        }
        KeyCode::Down => {
            if let Mode::ProtonPicker { builds, selected } = &mut app.mode {
                *selected = app::move_selection(*selected, builds.len() + 1, 1);
            }
        }
        KeyCode::Enter => {
            let Mode::ProtonPicker { builds, selected } =
                std::mem::replace(&mut app.mode, Mode::Normal)
            else {
                return;
            };
            config::apply_proton_choice(app, &builds, selected);
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
