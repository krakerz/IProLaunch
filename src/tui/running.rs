use crossterm::event::KeyCode;

use super::app::{self, App};
use crate::running;

pub fn on_key(app: &mut App, code: KeyCode) {
    if app.running_filter.is_some() {
        return filter_key(app, code);
    }

    match code {
        KeyCode::Up => {
            app.running_selected = app::move_selection(app.running_selected, app.running.len(), -1);
        }
        KeyCode::Down => {
            app.running_selected = app::move_selection(app.running_selected, app.running.len(), 1);
        }
        KeyCode::Char('r') => app.refresh_running(),
        KeyCode::Char('f') => start_filter(app),
        KeyCode::Enter | KeyCode::Char('k') | KeyCode::Delete => kill_selected(app),
        _ => {}
    }
}

fn start_filter(app: &mut App) {
    app.running_filter = Some(String::new());
    app.running_selected = 0;
}

/// Keys while the quick-search box is active (`f` was pressed): typing
/// edits the filter text directly (so `r`/`k` — this tab's own shortcuts —
/// are unavailable while filtering, since they're needed as literal
/// characters instead; `Esc` to leave filter mode restores them), Up/Down
/// navigate the filtered subset, Enter/Delete still kill the selected one.
fn filter_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => {
            app.running_filter = None;
            app.running_selected = 0;
        }
        KeyCode::Backspace => {
            if let Some(filter) = &mut app.running_filter {
                filter.pop();
            }
            app.running_selected = 0;
        }
        KeyCode::Char(c) => {
            if let Some(filter) = &mut app.running_filter {
                filter.push(c);
            }
            app.running_selected = 0;
        }
        KeyCode::Up => {
            let len = app.filtered_running_indices().len();
            app.running_selected = app::move_selection(app.running_selected, len, -1);
        }
        KeyCode::Down => {
            let len = app.filtered_running_indices().len();
            app.running_selected = app::move_selection(app.running_selected, len, 1);
        }
        KeyCode::Enter | KeyCode::Delete => kill_selected(app),
        _ => {}
    }
}

fn kill_selected(app: &mut App) {
    let indices = app.filtered_running_indices();
    let Some(&real_index) = indices.get(app.running_selected) else {
        return;
    };
    let Some(entry) = app.running.get(real_index) else {
        return;
    };
    let name = entry.name.clone();
    match running::terminate(&entry.prefix_path) {
        Ok(()) => app.status = Some(format!("Killed {name}.")),
        Err(err) => app.status = Some(format!("Failed to kill {name}: {err:#}")),
    }
    app.refresh_running();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::running::RunningEntry;

    // `kill_selected` -> `running::terminate` is safe to exercise directly:
    // with a fake `prefix_path` that matches no real process, `terminate`
    // just returns an `Err("nothing running against prefix ...")` (see
    // `running.rs`) — no real process is ever touched. `refresh_running`
    // (called afterwards) only re-scans real state, which is read-only.

    fn entry(name: &str) -> RunningEntry {
        RunningEntry {
            pid: 0,
            name: name.to_string(),
            target_path: format!("/tmp/{name}.exe"),
            prefix_path: format!("/tmp/iprolaunch-test-nonexistent-prefix/{name}"),
            started_at: String::new(),
        }
    }

    fn test_app_with_two_entries() -> App {
        let mut app = App::new(Config::default());
        app.running = vec![entry("Elden Ring"), entry("Guildmaster")];
        app.running_selected = 0;
        app
    }

    #[test]
    fn f_starts_an_empty_filter_and_resets_selection() {
        let mut app = test_app_with_two_entries();
        app.running_selected = 1;
        start_filter(&mut app);
        assert_eq!(app.running_filter.as_deref(), Some(""));
        assert_eq!(app.running_selected, 0);
    }

    #[test]
    fn typing_narrows_the_filtered_list_case_insensitively() {
        let mut app = test_app_with_two_entries();
        start_filter(&mut app);
        for c in "guild".chars() {
            filter_key(&mut app, KeyCode::Char(c));
        }
        let indices = app.filtered_running_indices();
        assert_eq!(indices, vec![1]);
    }

    #[test]
    fn backspace_removes_the_last_filter_character() {
        let mut app = test_app_with_two_entries();
        start_filter(&mut app);
        filter_key(&mut app, KeyCode::Char('x'));
        filter_key(&mut app, KeyCode::Char('x'));
        filter_key(&mut app, KeyCode::Backspace);
        assert_eq!(app.running_filter.as_deref(), Some("x"));
    }

    #[test]
    fn esc_clears_the_filter_entirely() {
        let mut app = test_app_with_two_entries();
        start_filter(&mut app);
        filter_key(&mut app, KeyCode::Char('x'));
        filter_key(&mut app, KeyCode::Esc);
        assert_eq!(app.running_filter, None);
        assert_eq!(app.filtered_running_indices().len(), 2);
    }

    #[test]
    fn up_down_navigate_within_the_filtered_subset_only() {
        let mut app = test_app_with_two_entries();
        app.running.push(entry("Guilty Gear"));
        start_filter(&mut app);
        for c in "guil".chars() {
            filter_key(&mut app, KeyCode::Char(c));
        }
        // "Guildmaster" and "Guilty Gear" both match; "Elden Ring" doesn't.
        assert_eq!(app.filtered_running_indices(), vec![1, 2]);
        assert_eq!(app.running_selected, 0);
        filter_key(&mut app, KeyCode::Down);
        assert_eq!(app.running_selected, 1);
        // Clamps at the last index of the filtered subset (2 matches), not
        // the full list (3 entries).
        filter_key(&mut app, KeyCode::Down);
        assert_eq!(app.running_selected, 1);
        filter_key(&mut app, KeyCode::Up);
        assert_eq!(app.running_selected, 0);
    }

    #[test]
    fn r_and_k_become_literal_filter_characters_while_searching() {
        let mut app = test_app_with_two_entries();
        start_filter(&mut app);
        filter_key(&mut app, KeyCode::Char('r'));
        filter_key(&mut app, KeyCode::Char('k'));
        assert_eq!(app.running_filter.as_deref(), Some("rk"));
        // Neither refreshed nor killed anything as a side effect.
        assert_eq!(app.running.len(), 2);
    }

    #[test]
    fn enter_kills_the_selected_entry_within_the_filtered_subset() {
        let mut app = test_app_with_two_entries();
        start_filter(&mut app);
        for c in "guild".chars() {
            filter_key(&mut app, KeyCode::Char(c));
        }
        filter_key(&mut app, KeyCode::Enter);
        assert_eq!(
            app.status.as_deref(),
            Some(
                "Failed to kill Guildmaster: nothing running against prefix /tmp/iprolaunch-test-nonexistent-prefix/Guildmaster"
            )
        );
    }
}
