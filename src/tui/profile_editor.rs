use crossterm::event::KeyCode;

use super::app::{
    self, App, FieldKind, MapField, Mode, ProfileField, ProtonPickerTarget, TextInputPurpose,
};
use crate::proton;

/// Keys while the Library tab is showing one profile's editor
/// (`app.profile_editor = Some(slug)`), in `Mode::Normal`. Routed here from
/// `library::on_key` — Up/Down/Enter behave like the Config tab's field
/// list, Esc leaves the editor and goes back to the plain library list.
pub fn on_key(app: &mut App, code: KeyCode, slug: String) {
    match code {
        KeyCode::Up => {
            app.profile_field_selected =
                app::move_selection(app.profile_field_selected, ProfileField::ALL.len(), -1);
        }
        KeyCode::Down => {
            app.profile_field_selected =
                app::move_selection(app.profile_field_selected, ProfileField::ALL.len(), 1);
        }
        KeyCode::Esc => app.profile_editor = None,
        KeyCode::Enter => activate_selected(app, &slug),
        _ => {}
    }
}

fn activate_selected(app: &mut App, slug: &str) {
    let field = ProfileField::ALL[app.profile_field_selected];
    match field.kind() {
        FieldKind::Text => {
            let buffer = current_text_value(app, slug, field);
            app.mode = Mode::TextInput {
                purpose: TextInputPurpose::ProfileField(slug.to_string(), field),
                buffer,
            };
        }
        FieldKind::Cycle => cycle_field(app, slug, field),
        FieldKind::ProtonPicker => match proton::scan() {
            Ok(builds) => {
                let current = app.profile(slug).and_then(|p| p.defaults.proton.clone());
                let selected = match current.as_deref() {
                    None => 0,
                    Some("system") => 1,
                    Some(id) => builds.iter().position(|b| b.id == id).map_or(1, |i| i + 2),
                };
                app.mode = Mode::ProtonPicker {
                    builds,
                    selected,
                    target: ProtonPickerTarget::Profile(slug.to_string()),
                };
            }
            Err(err) => app.status = Some(format!("proton scan failed: {err:#}")),
        },
        FieldKind::MapEditor => {
            let map_field = match field {
                ProfileField::EnvTable => MapField::ProfileEnv(slug.to_string()),
                ProfileField::WineDllOverrideTable => {
                    MapField::ProfileWineDllOverride(slug.to_string())
                }
                _ => return,
            };
            app.mode = Mode::MapEditor {
                field: map_field,
                selected: 0,
            };
        }
        // No profile field uses these kinds.
        FieldKind::Toggle | FieldKind::Number => {}
    }
}

fn cycle_field(app: &mut App, slug: &str, field: ProfileField) {
    if let Some(profile) = app.profile_mut(slug) {
        match field {
            ProfileField::LogRecord => {
                profile.logging.record = app::next_profile_record_mode(profile.logging.record);
            }
            ProfileField::LogAutoOpen => {
                profile.logging.auto_open = app::next_profile_auto_open(profile.logging.auto_open);
            }
            _ => {}
        }
    }
    save_profile(app, slug);
}

fn current_text_value(app: &App, slug: &str, field: ProfileField) -> String {
    let Some(profile) = app.profile(slug) else {
        return String::new();
    };
    match field {
        ProfileField::TargetPath => profile.target_path.clone(),
        ProfileField::Title => profile.title.clone().unwrap_or_default(),
        ProfileField::Args => profile.args.join(" "),
        ProfileField::PrefixPath => profile.defaults.prefix_path.clone().unwrap_or_default(),
        ProfileField::WindowsVersion => {
            profile.defaults.windows_version.clone().unwrap_or_default()
        }
        ProfileField::LogKeep => profile
            .logging
            .keep
            .map_or(String::new(), |k| k.to_string()),
        _ => String::new(),
    }
}

/// Applies a confirmed text-input popup value to the profile field it was
/// opened for, then saves. Unlike `TextInputPurpose::ProfileTitle` (the
/// add-by-path follow-up prompt), a blank buffer here means "clear the
/// override back to inherit", not "skip" — consistent with every other
/// override field in this editor. `TargetPath` is the exception: it's
/// mandatory (never "inherit"-able) and gets checked against the real
/// filesystem before being accepted, so a typo or a moved/deleted exe can't
/// silently leave the profile pointing at nothing.
pub fn apply_text_field(app: &mut App, slug: &str, field: ProfileField, value: String) {
    let trimmed = value.trim().to_string();

    if field == ProfileField::LogKeep && !trimmed.is_empty() && trimmed.parse::<u32>().is_err() {
        app.status = Some(format!(
            "\"{trimmed}\" isn't a whole number — logging.keep override left unchanged."
        ));
        return;
    }
    if field == ProfileField::TargetPath {
        if trimmed.is_empty() {
            app.status = Some("target-path can't be blank — left unchanged.".to_string());
            return;
        }
        if !std::path::Path::new(&trimmed).is_file() {
            app.status = Some(format!(
                "\"{trimmed}\" doesn't exist — target-path left unchanged."
            ));
            return;
        }
    }

    let Some(profile) = app.profile_mut(slug) else {
        app.status = Some(format!("couldn't find profile \"{slug}\""));
        return;
    };
    match field {
        ProfileField::TargetPath => profile.target_path = trimmed.clone(),
        ProfileField::Title => profile.title = (!trimmed.is_empty()).then(|| trimmed.clone()),
        ProfileField::Args => {
            profile.args = trimmed.split_whitespace().map(str::to_string).collect();
        }
        ProfileField::PrefixPath => {
            profile.defaults.prefix_path = (!trimmed.is_empty()).then(|| trimmed.clone());
        }
        ProfileField::WindowsVersion => {
            profile.defaults.windows_version = (!trimmed.is_empty()).then(|| trimmed.clone());
        }
        ProfileField::LogKeep => profile.logging.keep = trimmed.parse::<u32>().ok(),
        // Never actually reached via a text popup — these fields open a
        // different mode (`ProtonPicker`/`MapEditor`/`Cycle`) instead.
        ProfileField::Proton
        | ProfileField::LogRecord
        | ProfileField::LogAutoOpen
        | ProfileField::EnvTable
        | ProfileField::WineDllOverrideTable => {}
    }
    save_profile(app, slug);
}

/// Saves the named profile (looked up fresh, since the caller may only hold
/// the slug) and reports the outcome the same way `config::save_config`
/// does for the global config.
pub fn save_profile(app: &mut App, slug: &str) {
    let Some(profile) = app.profile(slug) else {
        app.status = Some(format!("couldn't find profile \"{slug}\" to save"));
        return;
    };
    app.status = Some(match profile.save(slug) {
        Ok(()) => "Saved.".to_string(),
        Err(err) => format!("Failed to save profile: {err:#}"),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Profile};

    // Same rule as `tui::config`'s and `tui::library`'s tests: nothing here
    // may reach `save_profile`/`Profile::save` (a real write to
    // `~/.config/iprolaunch/profiles/<slug>/profile.toml`, with no
    // test-only override of `project_dirs()`). That rules out
    // `cycle_field`, the successful-parse path of `apply_text_field`, and
    // the `MapEntryInput` commit step — verified by hand instead.

    fn test_app_with_profile(slug: &str) -> App {
        let mut app = App::new(Config::default());
        app.profiles = vec![(
            slug.to_string(),
            Profile {
                name: "Game#1".to_string(),
                target_path: format!("/tmp/{slug}.exe"),
                title: Some("Real Title".to_string()),
                last_launched: None,
                args: vec!["--dx11".to_string()],
                defaults: Default::default(),
                logging: Default::default(),
                env: Default::default(),
                winedlloverride: Default::default(),
            },
        )];
        app
    }

    #[test]
    fn up_down_wrap_within_field_count() {
        let mut app = test_app_with_profile("game-1");
        app.profile_field_selected = 0;
        on_key(&mut app, KeyCode::Up, "game-1".to_string());
        assert_eq!(app.profile_field_selected, 0); // can't go above the first field

        for _ in 0..ProfileField::ALL.len() + 2 {
            on_key(&mut app, KeyCode::Down, "game-1".to_string());
        }
        assert_eq!(app.profile_field_selected, ProfileField::ALL.len() - 1);
    }

    #[test]
    fn esc_leaves_the_editor() {
        let mut app = test_app_with_profile("game-1");
        app.profile_editor = Some("game-1".to_string());
        on_key(&mut app, KeyCode::Esc, "game-1".to_string());
        assert_eq!(app.profile_editor, None);
    }

    #[test]
    fn enter_on_a_text_field_opens_prefilled_text_input() {
        let mut app = test_app_with_profile("game-1");
        app.profile_field_selected = ProfileField::ALL
            .iter()
            .position(|f| *f == ProfileField::Title)
            .unwrap();
        activate_selected(&mut app, "game-1");

        match &app.mode {
            Mode::TextInput { purpose, buffer } => {
                assert!(matches!(
                    purpose,
                    TextInputPurpose::ProfileField(slug, ProfileField::Title) if slug == "game-1"
                ));
                assert_eq!(buffer, "Real Title");
            }
            _ => panic!("expected TextInput"),
        }
    }

    #[test]
    fn enter_on_the_env_field_opens_the_profile_scoped_map_editor() {
        let mut app = test_app_with_profile("game-1");
        app.profile_field_selected = ProfileField::ALL
            .iter()
            .position(|f| *f == ProfileField::EnvTable)
            .unwrap();
        activate_selected(&mut app, "game-1");

        assert!(matches!(
            app.mode,
            Mode::MapEditor {
                field: MapField::ProfileEnv(ref slug),
                selected: 0,
            } if slug == "game-1"
        ));
    }

    #[test]
    fn apply_text_field_rejects_a_non_numeric_log_keep_without_touching_the_profile() {
        let mut app = test_app_with_profile("game-1");
        apply_text_field(
            &mut app,
            "game-1",
            ProfileField::LogKeep,
            "not-a-number".to_string(),
        );
        assert!(
            app.status
                .as_deref()
                .unwrap()
                .contains("isn't a whole number")
        );
        // Untouched — still `None` (the default), never reached `save_profile`.
        assert_eq!(app.profile("game-1").unwrap().logging.keep, None);
    }
}
