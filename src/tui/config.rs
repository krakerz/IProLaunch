use crossterm::event::KeyCode;

use super::app::{self, App, ConfigField, FieldKind, MapField, Mode, TextInputPurpose};
use crate::proton;

pub fn on_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Up => {
            app.config_selected =
                app::move_selection(app.config_selected, ConfigField::ALL.len(), -1);
        }
        KeyCode::Down => {
            app.config_selected =
                app::move_selection(app.config_selected, ConfigField::ALL.len(), 1);
        }
        KeyCode::Left => adjust_number(app, -1),
        KeyCode::Right => adjust_number(app, 1),
        KeyCode::Enter => activate_selected(app),
        _ => {}
    }
}

fn activate_selected(app: &mut App) {
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
            app.mode = Mode::TextInput {
                purpose: TextInputPurpose::ConfigField(field),
                buffer,
            };
        }
        FieldKind::ProtonPicker => match proton::scan() {
            Ok(builds) => {
                let selected = builds
                    .iter()
                    .position(|b| b.id == app.cfg.defaults.proton)
                    .map_or(0, |i| i + 1); // +1: index 0 is the "system" entry
                app.mode = Mode::ProtonPicker { builds, selected };
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
        _ => {}
    }
    save_config(app);
}

fn current_text_value(app: &App, field: ConfigField) -> String {
    match field {
        ConfigField::PrefixPath => app.cfg.defaults.prefix_path.clone(),
        ConfigField::PrefixesRoot => app.cfg.defaults.prefixes_root.clone(),
        ConfigField::WindowsVersion => app.cfg.defaults.windows_version.clone(),
        ConfigField::LogPath => app.cfg.logging.path.clone(),
        _ => String::new(),
    }
}

/// Applies a confirmed text-input popup value to the field it was opened
/// for, then saves. Called from the shared `Mode::TextInput` handler in
/// `tui::mod`, since that mode is also used by the Library tab's "add by
/// path" — this only handles the `ConfigField` purpose.
pub fn apply_text_field(app: &mut App, field: ConfigField, value: String) {
    match field {
        ConfigField::PrefixPath => app.cfg.defaults.prefix_path = value,
        ConfigField::PrefixesRoot => app.cfg.defaults.prefixes_root = value,
        ConfigField::WindowsVersion => app.cfg.defaults.windows_version = value,
        ConfigField::LogPath => app.cfg.logging.path = value,
        _ => {}
    }
    save_config(app);
}

/// Applies a confirmed proton-picker selection (`0` = "system", else
/// `builds[selected - 1]`) to `defaults.proton`, then saves.
pub fn apply_proton_choice(app: &mut App, builds: &[proton::ProtonBuild], selected: usize) {
    app.cfg.defaults.proton = if selected == 0 {
        "system".to_string()
    } else {
        builds
            .get(selected - 1)
            .map_or_else(|| "system".to_string(), |b| b.id.clone())
    };
    save_config(app);
}

fn adjust_number(app: &mut App, delta: i64) {
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

/// Keys while browsing one map's entries (`Mode::MapEditor`): `a` add,
/// `e` edit the selected entry, `d` delete it, Esc back to the Config tab.
pub fn map_editor_key(app: &mut App, code: KeyCode) {
    let (field, selected) = match app.mode {
        Mode::MapEditor { field, selected } => (field, selected),
        _ => return,
    };
    let entries = app.map_entries(field);

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
            if let Some((k, _)) = entries.get(selected) {
                let k = k.clone();
                app.map_mut(field).remove(&k);
                save_config(app);
            }
            let new_len = app.map_entries(field).len();
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
            if let Mode::MapEntryInput { field, .. } = app.mode {
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
            if !key.is_empty() {
                if let Some(old_key) = &original_key
                    && *old_key != key
                {
                    app.map_mut(field).remove(old_key);
                }
                app.map_mut(field).insert(key, value);
                save_config(app);
            }
            app.mode = Mode::MapEditor { field, selected: 0 };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adjust_u32_saturates_instead_of_underflowing() {
        assert_eq!(adjust_u32(0, -1), 0);
        assert_eq!(adjust_u32(3, -1), 2);
        assert_eq!(adjust_u32(3, 1), 4);
    }

    // The tests below only exercise paths that don't call `save_config` (and
    // so never touch `Config::save()`, which writes to the *real*
    // `~/.config/iprolaunch/config.toml` — there is no test-only override of
    // `project_dirs()`). Add ('a'), edit ('e'), delete ('d'), and the final
    // confirm-on-Value step all call it and are deliberately NOT covered
    // here; verify those by hand instead. Never add a test that reaches
    // `save_config`/`app.cfg.save()` without a way to redirect it away from
    // the real config file first.

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
