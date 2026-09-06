use crossterm::event::KeyCode;

use super::app::{self, App, ConfigField, FieldKind, Mode, TextInputPurpose};
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adjust_u32_saturates_instead_of_underflowing() {
        assert_eq!(adjust_u32(0, -1), 0);
        assert_eq!(adjust_u32(3, -1), 2);
        assert_eq!(adjust_u32(3, 1), 4);
    }
}
