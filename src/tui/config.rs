use crossterm::event::KeyCode;

use super::app::{
    self, App, ConfigField, FieldKind, IntegrateAction, IntegrateField, MapField, Mode,
    ProtonPickerTarget, TextInputPurpose,
};
use super::profile_editor;
use super::{Term, resume, suspend};
use crate::proton;

/// The Config tab's main field list and its "Desktop integration" table
/// share one continuous selection index (`app.config_selected`) so Up/Down
/// flows from one into the other with no separate focus-switch key — this
/// is the boundary between the two ranges.
fn total_rows() -> usize {
    ConfigField::ALL.len() + IntegrateField::ALL.len()
}

pub fn on_key(app: &mut App, code: KeyCode, terminal: &mut Term) {
    match code {
        KeyCode::Up => {
            app.config_selected = app::move_selection(app.config_selected, total_rows(), -1);
        }
        KeyCode::Down => {
            app.config_selected = app::move_selection(app.config_selected, total_rows(), 1);
        }
        KeyCode::Left => adjust_number(app, -1),
        KeyCode::Right => adjust_number(app, 1),
        KeyCode::Enter => activate_selected(app, terminal),
        _ => {}
    }
}

fn activate_selected(app: &mut App, terminal: &mut Term) {
    if app.config_selected >= ConfigField::ALL.len() {
        let field = IntegrateField::ALL[app.config_selected - ConfigField::ALL.len()];
        return activate_integrate_field(app, terminal, field);
    }

    let field = ConfigField::ALL[app.config_selected];
    match field.kind() {
        FieldKind::Cycle => cycle_field(app, field),
        FieldKind::Toggle => {
            app.cfg.logging.auto_open = !app.cfg.logging.auto_open;
            save_config(app);
        }
        FieldKind::Number => {} // Left/Right, not Enter
        FieldKind::Text => {
            let buffer = current_text_value(app, field);
            let cursor = buffer.chars().count();
            app.mode = Mode::TextInput {
                purpose: TextInputPurpose::ConfigField(field),
                buffer,
                cursor,
            };
        }
        FieldKind::ProtonPicker => match proton::scan() {
            Ok(builds) => {
                let selected = builds
                    .iter()
                    .position(|b| b.id == app.cfg.defaults.proton)
                    .map_or(0, |i| i + 1); // +1: index 0 is the "system" entry
                app.mode = Mode::ProtonPicker {
                    builds,
                    selected,
                    target: ProtonPickerTarget::Global,
                };
            }
            Err(err) => app.status = Some(format!("proton scan failed: {err:#}")),
        },
        FieldKind::MapEditor => {
            let map_field = match field {
                ConfigField::EnvTable => MapField::Env,
                ConfigField::WineDllOverrideTable => MapField::WineDllOverride,
                _ => return,
            };
            app.mode = Mode::MapEditor {
                field: map_field,
                selected: 0,
            };
        }
    }
}

fn activate_integrate_field(app: &mut App, terminal: &mut Term, field: IntegrateField) {
    match field {
        // Info rows — nothing to do on Enter.
        IntegrateField::Status | IntegrateField::BinaryPath => {}
        IntegrateField::Setup => run_integrate_action(app, terminal, IntegrateAction::Setup),
        IntegrateField::Reapply => run_integrate_action(app, terminal, IntegrateAction::Reapply),
        IntegrateField::Uninstall => {
            run_integrate_action(app, terminal, IntegrateAction::Uninstall)
        }
    }
}

/// Runs one desktop-integration action, guarding against the two
/// nonsensical combinations first (Setup when already installed, Reapply/
/// Uninstall when not installed yet) without even suspending the TUI for
/// those. `Setup` and `Reapply` both just call `integrate::install` — it's
/// already idempotent and backup-guarded (a backup is only captured if one
/// doesn't already exist), so re-running it is exactly "refresh the
/// `.desktop` file's binary path and re-apply the mimetype defaults,
/// leaving the original backup alone" — the two rows exist for clarity of
/// intent, not because the underlying action differs.
///
/// Suspends the TUI first since `integrate::install`/`uninstall` print
/// their own informational output via `println!` (which mimetypes were
/// touched, or "nothing to uninstall"), which would otherwise be invisible
/// or corrupt the alternate screen if called while the TUI is drawing.
/// Mirrors `library::launch_path`'s suspend/run/wait-for-enter/resume shape.
/// Whether `action` is nonsensical given the current `installed` state
/// (Setup when already installed, Reapply/Uninstall when not installed
/// yet), and if so, the status message to show instead of running it. Kept
/// pure (installed state passed in, not queried here) so it's testable
/// without needing a real `.desktop` file or a `Term`.
fn integrate_guard_message(action: IntegrateAction, installed: bool) -> Option<&'static str> {
    match (action, installed) {
        (IntegrateAction::Setup, true) => {
            Some("Already installed — use Reapply to refresh the binary path.")
        }
        (IntegrateAction::Reapply, false) | (IntegrateAction::Uninstall, false) => {
            Some("Not installed yet — use Setup first.")
        }
        _ => None,
    }
}

fn run_integrate_action(app: &mut App, terminal: &mut Term, action: IntegrateAction) {
    if let Some(message) = integrate_guard_message(action, crate::integrate::is_installed()) {
        app.status = Some(message.to_string());
        return;
    }

    if suspend(terminal).is_err() {
        app.status = Some("Failed to suspend the TUI.".to_string());
        return;
    }

    let result = match action {
        IntegrateAction::Setup | IntegrateAction::Reapply => crate::integrate::install(),
        IntegrateAction::Uninstall => crate::integrate::uninstall(),
    };
    if let Err(err) = &result {
        println!("Error: {err:#}");
    }

    use std::io::Write;
    print!("\nPress Enter to return to iprolaunch. ");
    std::io::stdout().flush().ok();
    let mut discard = String::new();
    std::io::stdin().read_line(&mut discard).ok();

    if resume(terminal).is_err() {
        app.status = Some("Failed to restore the TUI.".to_string());
        return;
    }

    app.status = Some(match (action, &result) {
        (_, Err(err)) => format!("Desktop integration failed: {err:#}"),
        (IntegrateAction::Setup, Ok(())) => "Set up as default handler.".to_string(),
        (IntegrateAction::Reapply, Ok(())) => "Reapplied — binary path refreshed.".to_string(),
        (IntegrateAction::Uninstall, Ok(())) => "Removed as default handler.".to_string(),
    });
}

fn cycle_field(app: &mut App, field: ConfigField) {
    match field {
        ConfigField::PrefixMode => {
            app.cfg.defaults.prefix_mode = app::next_prefix_mode(app.cfg.defaults.prefix_mode);
        }
        ConfigField::LogMode => {
            app.cfg.logging.mode = app::next_log_mode(app.cfg.logging.mode);
        }
        ConfigField::LogRecord => {
            app.cfg.logging.record = app::next_record_mode(app.cfg.logging.record);
        }
        ConfigField::Gamescope => {
            app.cfg.defaults.gamescope = app::next_gamescope_setting(app.cfg.defaults.gamescope);
        }
        ConfigField::GamescopeFilter => {
            app.cfg.defaults.gamescope_settings.filter =
                app::next_gamescope_filter(app.cfg.defaults.gamescope_settings.filter);
        }
        ConfigField::GamescopeScaler => {
            app.cfg.defaults.gamescope_settings.scaler =
                app::next_gamescope_scaler(app.cfg.defaults.gamescope_settings.scaler);
        }
        ConfigField::GamescopeBorderless => {
            app.cfg.defaults.gamescope_settings.borderless =
                app::next_optional_bool(app.cfg.defaults.gamescope_settings.borderless);
        }
        ConfigField::GamescopeGrabCursor => {
            app.cfg.defaults.gamescope_settings.grab_cursor =
                app::next_optional_bool(app.cfg.defaults.gamescope_settings.grab_cursor);
        }
        ConfigField::GamescopeAdaptiveSync => {
            app.cfg.defaults.gamescope_settings.adaptive_sync =
                app::next_optional_bool(app.cfg.defaults.gamescope_settings.adaptive_sync);
        }
        _ => {}
    }
    save_config(app);
}

fn current_text_value(app: &App, field: ConfigField) -> String {
    let gs = &app.cfg.defaults.gamescope_settings;
    match field {
        ConfigField::PrefixPath => app.cfg.defaults.prefix_path.clone(),
        ConfigField::PrefixesRoot => app.cfg.defaults.prefixes_root.clone(),
        ConfigField::WindowsVersion => app.cfg.defaults.windows_version.clone(),
        ConfigField::LogPath => app.cfg.logging.path.clone(),
        ConfigField::GamescopeOutputWidth => {
            gs.output_width.map_or(String::new(), |v| v.to_string())
        }
        ConfigField::GamescopeOutputHeight => {
            gs.output_height.map_or(String::new(), |v| v.to_string())
        }
        ConfigField::GamescopeRefresh => gs.refresh.map_or(String::new(), |v| v.to_string()),
        ConfigField::GamescopeNestedWidth => {
            gs.nested_width.map_or(String::new(), |v| v.to_string())
        }
        ConfigField::GamescopeNestedHeight => {
            gs.nested_height.map_or(String::new(), |v| v.to_string())
        }
        ConfigField::LaunchWrapper => app.cfg.defaults.launch_wrapper.clone().unwrap_or_default(),
        _ => String::new(),
    }
}

/// Applies a confirmed text-input popup value to the field it was opened
/// for, then saves. Called from the shared `Mode::TextInput` handler in
/// `tui::mod`, since that mode is also used by the Library tab's "add by
/// path" and the profile editor — this only handles the `ConfigField`
/// purpose. The 5 numeric gamescope fields are validated the same way the
/// profile editor's equivalents are (see `profile_editor::apply_text_field`)
/// — blank clears back to "unset" (don't pass that flag to gamescope at
/// all), a non-numeric value is rejected with a status message instead of
/// silently discarded.
pub fn apply_text_field(app: &mut App, field: ConfigField, value: String) {
    let trimmed = value.trim().to_string();
    let is_numeric_field = matches!(
        field,
        ConfigField::GamescopeOutputWidth
            | ConfigField::GamescopeOutputHeight
            | ConfigField::GamescopeRefresh
            | ConfigField::GamescopeNestedWidth
            | ConfigField::GamescopeNestedHeight
    );
    if is_numeric_field && !trimmed.is_empty() && trimmed.parse::<u32>().is_err() {
        app.status = Some(format!(
            "\"{trimmed}\" isn't a whole number — {} left unchanged.",
            field.label()
        ));
        return;
    }

    match field {
        ConfigField::PrefixPath => app.cfg.defaults.prefix_path = value,
        ConfigField::PrefixesRoot => app.cfg.defaults.prefixes_root = value,
        ConfigField::WindowsVersion => app.cfg.defaults.windows_version = value,
        ConfigField::LogPath => app.cfg.logging.path = value,
        ConfigField::GamescopeOutputWidth => {
            app.cfg.defaults.gamescope_settings.output_width = trimmed.parse::<u32>().ok();
        }
        ConfigField::GamescopeOutputHeight => {
            app.cfg.defaults.gamescope_settings.output_height = trimmed.parse::<u32>().ok();
        }
        ConfigField::GamescopeRefresh => {
            app.cfg.defaults.gamescope_settings.refresh = trimmed.parse::<u32>().ok();
        }
        ConfigField::GamescopeNestedWidth => {
            app.cfg.defaults.gamescope_settings.nested_width = trimmed.parse::<u32>().ok();
        }
        ConfigField::GamescopeNestedHeight => {
            app.cfg.defaults.gamescope_settings.nested_height = trimmed.parse::<u32>().ok();
        }
        ConfigField::LaunchWrapper => {
            app.cfg.defaults.launch_wrapper = (!trimmed.is_empty()).then_some(trimmed.clone());
        }
        _ => {}
    }
    save_config(app);
}

/// Applies a confirmed proton-picker selection to whichever config `target`
/// says: the global default (`0` = "system", else `builds[selected - 1]`),
/// or one profile's override (`0` = inherit/`None`, `1` = "system", else
/// `builds[selected - 2]`), then saves the right file.
pub fn apply_proton_choice(
    app: &mut App,
    builds: &[proton::ProtonBuild],
    selected: usize,
    target: &ProtonPickerTarget,
) {
    match target {
        ProtonPickerTarget::Global => {
            app.cfg.defaults.proton = if selected == 0 {
                "system".to_string()
            } else {
                builds
                    .get(selected - 1)
                    .map_or_else(|| "system".to_string(), |b| b.id.clone())
            };
            save_config(app);
        }
        ProtonPickerTarget::Profile(slug) => {
            let value = if selected == 0 {
                None
            } else if selected == 1 {
                Some("system".to_string())
            } else {
                builds.get(selected - 2).map(|b| b.id.clone())
            };
            if let Some(profile) = app.profile_mut(slug) {
                profile.defaults.proton = value;
            }
            profile_editor::save_profile(app, slug);
        }
    }
}

fn adjust_number(app: &mut App, delta: i64) {
    if app.config_selected >= ConfigField::ALL.len() {
        return; // no Number-kind field in the integrate table
    }
    let field = ConfigField::ALL[app.config_selected];
    match field {
        ConfigField::LogKeep => {
            app.cfg.logging.keep = adjust_u32(app.cfg.logging.keep, delta);
            save_config(app);
        }
        ConfigField::GamedbInterval => {
            app.cfg.gamedb.update_interval_days =
                adjust_u32(app.cfg.gamedb.update_interval_days, delta);
            save_config(app);
        }
        _ => {}
    }
}

fn adjust_u32(v: u32, delta: i64) -> u32 {
    if delta < 0 {
        v.saturating_sub(delta.unsigned_abs() as u32)
    } else {
        v.saturating_add(delta as u32)
    }
}

fn save_config(app: &mut App) {
    match app.cfg.save() {
        Ok(()) => app.status = Some("Saved.".to_string()),
        Err(err) => app.status = Some(format!("Failed to save config: {err:#}")),
    }
}

/// Saves whichever config a `MapField` belongs to — the global config for
/// `Env`/`WineDllOverride`, or the named profile for a `Profile*` variant.
fn save_map_owner(app: &mut App, field: &MapField) {
    match field {
        MapField::Env | MapField::WineDllOverride => save_config(app),
        MapField::ProfileEnv(slug) | MapField::ProfileWineDllOverride(slug) => {
            profile_editor::save_profile(app, &slug.clone());
        }
    }
}

/// Keys while browsing one map's entries (`Mode::MapEditor`): `a` add,
/// `e` edit the selected entry, `d` delete it, Esc back (to the Config tab,
/// or to the profile editor if `app.profile_editor` is set — both just
/// return to `Mode::Normal`, and whichever screen that resolves to is
/// decided entirely by `app.profile_editor`, not by anything in `Mode`).
pub fn map_editor_key(app: &mut App, code: KeyCode) {
    let Mode::MapEditor { field, selected } = &app.mode else {
        return;
    };
    let field = field.clone();
    let selected = *selected;
    let entries = app.map_entries(&field);

    match code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Up => {
            let selected = app::move_selection(selected, entries.len(), -1);
            app.mode = Mode::MapEditor { field, selected };
        }
        KeyCode::Down => {
            let selected = app::move_selection(selected, entries.len(), 1);
            app.mode = Mode::MapEditor { field, selected };
        }
        KeyCode::Char('a') => {
            app.mode = Mode::MapEntryInput {
                field,
                original_key: None,
                step: app::MapEntryStep::Key,
                key: String::new(),
                value: String::new(),
            };
        }
        KeyCode::Char('e') => {
            if let Some((k, v)) = entries.get(selected) {
                app.mode = Mode::MapEntryInput {
                    field,
                    original_key: Some(k.clone()),
                    step: app::MapEntryStep::Key,
                    key: k.clone(),
                    value: v.clone(),
                };
            }
        }
        KeyCode::Char('d') => {
            if let Some((k, _)) = entries.get(selected)
                && let Some(map) = app.map_mut(&field)
            {
                map.remove(k);
                save_map_owner(app, &field);
            }
            let new_len = app.map_entries(&field).len();
            let selected = selected.min(new_len.saturating_sub(1));
            app.mode = Mode::MapEditor { field, selected };
        }
        _ => {}
    }
}

/// Keys while adding/editing one map entry (`Mode::MapEntryInput`): plain
/// typing/Backspace edit whichever of `key`/`value` is the active step;
/// Enter advances Key → Value, then commits on Value; Esc cancels entirely
/// (never partially applies a half-finished edit).
pub fn map_entry_input_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => {
            if let Mode::MapEntryInput { field, .. } = &app.mode {
                let field = field.clone();
                app.mode = Mode::MapEditor { field, selected: 0 };
            }
            return;
        }
        KeyCode::Backspace => {
            match &mut app.mode {
                Mode::MapEntryInput {
                    step: app::MapEntryStep::Key,
                    key,
                    ..
                } => {
                    key.pop();
                }
                Mode::MapEntryInput {
                    step: app::MapEntryStep::Value,
                    value,
                    ..
                } => {
                    value.pop();
                }
                _ => {}
            }
            return;
        }
        KeyCode::Char(c) => {
            match &mut app.mode {
                Mode::MapEntryInput {
                    step: app::MapEntryStep::Key,
                    key,
                    ..
                } => {
                    key.push(c);
                }
                Mode::MapEntryInput {
                    step: app::MapEntryStep::Value,
                    value,
                    ..
                } => {
                    value.push(c);
                }
                _ => {}
            }
            return;
        }
        KeyCode::Enter => {}
        _ => return,
    }

    let Mode::MapEntryInput {
        field,
        original_key,
        step,
        key,
        value,
    } = std::mem::replace(&mut app.mode, Mode::Normal)
    else {
        return;
    };

    match step {
        app::MapEntryStep::Key => {
            app.mode = Mode::MapEntryInput {
                field,
                original_key,
                step: app::MapEntryStep::Value,
                key,
                value,
            };
        }
        app::MapEntryStep::Value => {
            let key = key.trim().to_string();
            if !key.is_empty()
                && let Some(map) = app.map_mut(&field)
            {
                if let Some(old_key) = &original_key
                    && *old_key != key
                {
                    map.remove(old_key);
                }
                map.insert(key, value);
                save_map_owner(app, &field);
            }
            app.mode = Mode::MapEditor { field, selected: 0 };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use app::MapField;

    #[test]
    fn adjust_u32_saturates_instead_of_underflowing() {
        assert_eq!(adjust_u32(0, -1), 0);
        assert_eq!(adjust_u32(3, -1), 2);
        assert_eq!(adjust_u32(3, 1), 4);
    }

    #[test]
    fn total_rows_spans_both_config_and_integrate_tables() {
        assert_eq!(
            total_rows(),
            ConfigField::ALL.len() + IntegrateField::ALL.len()
        );
    }

    #[test]
    fn integrate_guard_blocks_setup_when_already_installed() {
        assert!(integrate_guard_message(IntegrateAction::Setup, true).is_some());
        assert!(integrate_guard_message(IntegrateAction::Setup, false).is_none());
    }

    #[test]
    fn integrate_guard_blocks_reapply_and_uninstall_when_not_installed() {
        assert!(integrate_guard_message(IntegrateAction::Reapply, false).is_some());
        assert!(integrate_guard_message(IntegrateAction::Uninstall, false).is_some());
        assert!(integrate_guard_message(IntegrateAction::Reapply, true).is_none());
        assert!(integrate_guard_message(IntegrateAction::Uninstall, true).is_none());
    }

    // The tests below only exercise paths that don't call `save_config`/
    // `save_map_owner` (and so never touch `Config::save()` or
    // `Profile::save()`, which write to the *real* `~/.config/iprolaunch/`
    // tree — there is no test-only override of `project_dirs()`). Add
    // ('a'), edit ('e'), delete ('d'), and the final confirm-on-Value step
    // all call one of those and are deliberately NOT covered here; verify
    // those by hand instead. Never add a test that reaches `app.cfg.save()`
    // or `Profile::save()` without a way to redirect it away from the real
    // files first.

    fn test_app() -> App {
        App::new(crate::config::Config::default())
    }

    #[test]
    fn map_editor_navigation_wraps_within_entry_count() {
        let mut app = test_app();
        app.cfg.env.insert("A".into(), "1".into());
        app.cfg.env.insert("B".into(), "2".into());
        app.mode = Mode::MapEditor {
            field: MapField::Env,
            selected: 0,
        };

        map_editor_key(&mut app, KeyCode::Down);
        assert!(matches!(app.mode, Mode::MapEditor { selected: 1, .. }));
        map_editor_key(&mut app, KeyCode::Down); // already at the last entry
        assert!(matches!(app.mode, Mode::MapEditor { selected: 1, .. }));
        map_editor_key(&mut app, KeyCode::Up);
        assert!(matches!(app.mode, Mode::MapEditor { selected: 0, .. }));
    }

    #[test]
    fn map_editor_esc_returns_to_normal() {
        let mut app = test_app();
        app.mode = Mode::MapEditor {
            field: MapField::Env,
            selected: 0,
        };
        map_editor_key(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn add_opens_entry_input_with_empty_buffers() {
        let mut app = test_app();
        app.mode = Mode::MapEditor {
            field: MapField::WineDllOverride,
            selected: 0,
        };
        map_editor_key(&mut app, KeyCode::Char('a'));

        match &app.mode {
            Mode::MapEntryInput {
                field,
                original_key,
                step,
                key,
                value,
            } => {
                assert_eq!(*field, MapField::WineDllOverride);
                assert_eq!(*original_key, None);
                assert_eq!(*step, app::MapEntryStep::Key);
                assert_eq!(key, "");
                assert_eq!(value, "");
            }
            _ => panic!("expected MapEntryInput"),
        }
    }

    #[test]
    fn edit_opens_entry_input_prefilled_with_the_selected_entry() {
        let mut app = test_app();
        app.cfg
            .winedlloverride
            .insert("winhttp".into(), "n,b".into());
        app.mode = Mode::MapEditor {
            field: MapField::WineDllOverride,
            selected: 0,
        };
        map_editor_key(&mut app, KeyCode::Char('e'));

        match &app.mode {
            Mode::MapEntryInput {
                original_key,
                key,
                value,
                ..
            } => {
                assert_eq!(original_key.as_deref(), Some("winhttp"));
                assert_eq!(key, "winhttp");
                assert_eq!(value, "n,b");
            }
            _ => panic!("expected MapEntryInput"),
        }
    }

    #[test]
    fn typing_and_backspace_edit_the_active_steps_buffer() {
        let mut app = test_app();
        app.mode = Mode::MapEntryInput {
            field: MapField::Env,
            original_key: None,
            step: app::MapEntryStep::Key,
            key: String::new(),
            value: String::new(),
        };
        for c in "FOO".chars() {
            map_entry_input_key(&mut app, KeyCode::Char(c));
        }
        map_entry_input_key(&mut app, KeyCode::Backspace);

        let Mode::MapEntryInput { key, .. } = &app.mode else {
            panic!("expected MapEntryInput")
        };
        assert_eq!(key, "FO");
    }

    #[test]
    fn enter_on_key_step_advances_to_value_step_keeping_the_key() {
        let mut app = test_app();
        app.mode = Mode::MapEntryInput {
            field: MapField::Env,
            original_key: None,
            step: app::MapEntryStep::Key,
            key: "FOO".to_string(),
            value: String::new(),
        };
        map_entry_input_key(&mut app, KeyCode::Enter);

        match &app.mode {
            Mode::MapEntryInput { step, key, .. } => {
                assert_eq!(*step, app::MapEntryStep::Value);
                assert_eq!(key, "FOO");
            }
            _ => panic!("expected MapEntryInput still, on the Value step"),
        }
    }

    #[test]
    fn esc_during_entry_input_cancels_back_to_the_map_editor() {
        let mut app = test_app();
        app.mode = Mode::MapEntryInput {
            field: MapField::Env,
            original_key: None,
            step: app::MapEntryStep::Value,
            key: "FOO".to_string(),
            value: "bar".to_string(),
        };
        map_entry_input_key(&mut app, KeyCode::Esc);

        assert!(matches!(
            app.mode,
            Mode::MapEditor {
                field: MapField::Env,
                ..
            }
        ));
        // And critically: cancelling must not have written anything.
        assert!(app.cfg.env.is_empty());
    }
}
