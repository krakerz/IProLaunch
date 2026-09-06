use crossterm::event::KeyCode;

use super::app::{self, App};
use crate::running;

pub fn on_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Up => {
            app.running_selected = app::move_selection(app.running_selected, app.running.len(), -1);
        }
        KeyCode::Down => {
            app.running_selected = app::move_selection(app.running_selected, app.running.len(), 1);
        }
        KeyCode::Char('r') => app.refresh_running(),
        KeyCode::Enter | KeyCode::Char('k') | KeyCode::Delete => kill_selected(app),
        _ => {}
    }
}

fn kill_selected(app: &mut App) {
    let Some(entry) = app.running.get(app.running_selected) else {
        return;
    };
    let name = entry.name.clone();
    match running::terminate(&entry.prefix_path) {
        Ok(()) => app.status = Some(format!("Killed {name}.")),
        Err(err) => app.status = Some(format!("Failed to kill {name}: {err:#}")),
    }
    app.refresh_running();
}
