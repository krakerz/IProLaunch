use crossterm::event::KeyCode;

use super::app::{self, App};
use crate::running;

pub fn on_key(app: &mut App, code: KeyCode) {
    if app.running_filter_editing {
        return filter_key(app, code);
    }

    match code {
        KeyCode::Up => {
            let len = app.filtered_running_indices().len();
            app.running_selected = app::move_selection(app.running_selected, len, -1);
        }
        KeyCode::Down => {
            let len = app.filtered_running_indices().len();
            app.running_selected = app::move_selection(app.running_selected, len, 1);
        }
        KeyCode::Char('r') => app.refresh_running(),
        KeyCode::Char('f') => start_filter(app),
        KeyCode::Enter | KeyCode::Char('k') | KeyCode::Delete => kill_selected(app),
        // Only meaningful once a filter is locked (still-typing Esc is
        // handled by `filter_key` instead, via the early return above) —
        // a no-op otherwise, same as Esc always was here.
        KeyCode::Esc if app.running_filter.is_some() => app.clear_running_filter(),
        _ => {}
    }
}

fn start_filter(app: &mut App) {
    app.running_filter = Some(String::new());
    app.running_filter_editing = true;
    app.running_selected = 0;
}

/// Keys while actively typing the quick-search box (`f` was pressed, not
/// yet locked): typing edits the filter text directly (so `r`/`k` — this
/// tab's own shortcuts — are unavailable while typing, since they're
/// needed as literal characters instead), Up/Down navigate the filtered
/// subset, Esc clears the filter entirely. Enter/Delete *lock* it instead
/// of killing — the filtered view stays exactly as it is, but every other
/// key (including Enter/Delete/k, next press) goes back to meaning what it
/// normally does, now scoped to the filtered subset (see `on_key`'s
/// `running_filter_editing` check).
fn filter_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => {
            app.running_filter = None;
            app.running_filter_editing = false;
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
        KeyCode::Enter | KeyCode::Delete => app.running_filter_editing = false,
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
    match running::terminate(&entry.launch_id) {
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
    // with a fake `launch_id` that matches no real process, `terminate`
    // just returns an `Err("nothing running against launch ...")` (see
    // `running.rs`) — no real process is ever touched. `refresh_running`
    // (called afterwards) only re-scans real state, which is read-only.

    fn entry(name: &str) -> RunningEntry {
        RunningEntry {
            pid: 0,
            name: name.to_string(),
            target_path: format!("/tmp/{name}.exe"),
            prefix_path: format!("/tmp/iprolaunch-test-nonexistent-prefix/{name}"),
            launch_id: format!("iprolaunch-test-nonexistent-launch-{name}"),
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
    fn enter_locks_the_filter_instead_of_killing() {
        let mut app = test_app_with_two_entries();
        start_filter(&mut app);
        for c in "guild".chars() {
            filter_key(&mut app, KeyCode::Char(c));
        }
        filter_key(&mut app, KeyCode::Enter);
        assert!(!app.running_filter_editing);
        // Still filtered — locking keeps the narrowed view, not just
        // resets it.
        assert_eq!(app.running_filter.as_deref(), Some("guild"));
        assert!(app.status.is_none(), "locking shouldn't kill anything");
    }

    #[test]
    fn delete_also_locks_the_filter_instead_of_killing() {
        let mut app = test_app_with_two_entries();
        start_filter(&mut app);
        filter_key(&mut app, KeyCode::Delete);
        assert!(!app.running_filter_editing);
        assert!(app.status.is_none());
    }

    #[test]
    fn a_second_enter_after_locking_kills_the_selected_entry() {
        let mut app = test_app_with_two_entries();
        start_filter(&mut app);
        for c in "guild".chars() {
            filter_key(&mut app, KeyCode::Char(c));
        }
        filter_key(&mut app, KeyCode::Enter); // locks
        on_key(&mut app, KeyCode::Enter); // now a normal key again, scoped to the filtered subset
        assert_eq!(
            app.status.as_deref(),
            Some(
                "Failed to kill Guildmaster: nothing running against launch iprolaunch-test-nonexistent-launch-Guildmaster"
            )
        );
    }

    #[test]
    fn once_locked_r_refreshes_normally_instead_of_editing_the_filter() {
        let mut app = test_app_with_two_entries();
        start_filter(&mut app);
        filter_key(&mut app, KeyCode::Char('r')); // still typing: 'r' is filter text, not refresh
        assert_eq!(app.running_filter.as_deref(), Some("r"));
        filter_key(&mut app, KeyCode::Enter); // locks
        on_key(&mut app, KeyCode::Char('r')); // now refresh again, not more filter text
        // refresh_running() only touches `status` on error — reaching here
        // at all (rather than 'r' silently becoming "rr" in the filter
        // text) is what actually matters.
        assert_eq!(app.running_filter.as_deref(), Some("r"));
        // The filter itself is still in place — only Esc/switching tabs
        // should clear it, not `r`.
        assert!(app.running_filter.is_some());
    }

    #[test]
    fn esc_clears_a_locked_filter_too() {
        let mut app = test_app_with_two_entries();
        start_filter(&mut app);
        filter_key(&mut app, KeyCode::Char('x'));
        filter_key(&mut app, KeyCode::Enter); // locks
        on_key(&mut app, KeyCode::Esc);
        assert_eq!(app.running_filter, None);
        assert!(!app.running_filter_editing);
    }
}
