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
            let cursor = buffer.chars().count();
            app.mode = Mode::TextInput {
                purpose: TextInputPurpose::ProfileField(slug.to_string(), field),
                buffer,
                cursor,
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
    if field == ProfileField::Slug {
        // Shown as-is (not stripped of any historical "-N" disambiguator)
        // — unlike `#` in `Name`, `-` legitimately appears in real slugs
        // (e.g. "elden-ring"), so there's no reliable way to tell "part of
        // the name" from "an auto-added suffix" just by looking at the
        // stored string. The user edits the full text; whatever they save
        // gets freshly disambiguated against other profiles if needed.
        return slug.to_string();
    }
    let Some(profile) = app.profile(slug) else {
        return String::new();
    };
    match field {
        ProfileField::TargetPath => profile.target_path.clone(),
        ProfileField::Name => name_base(&profile.name).to_string(),
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

/// The part of a display name before its last literal `#` — safe to split
/// on since `#` is this app's own disambiguator marker, never otherwise
/// used in a name (unlike `-` in a slug, which can't be split the same
/// way — see `current_text_value`'s `Slug` case).
fn name_base(name: &str) -> &str {
    name.rsplit_once('#').map_or(name, |(base, _)| base)
}

/// Smallest `n >= 1` such that no *other* profile is already named
/// `"{base}#{n}"` — mirrors `launch::ensure_profile`'s own disambiguation
/// loop (fills the lowest free slot rather than always growing past the
/// historical max, so a gap left by a deleted/renamed profile gets reused).
fn next_available_name(
    base: &str,
    profiles: &[(String, crate::config::Profile)],
    exclude_slug: &str,
) -> String {
    let mut n = 1;
    let mut candidate = format!("{base}#{n}");
    while profiles
        .iter()
        .any(|(s, p)| s != exclude_slug && p.name == candidate)
    {
        n += 1;
        candidate = format!("{base}#{n}");
    }
    candidate
}

/// Smallest available slug starting from `sanitized` itself (no suffix),
/// then `sanitized-2`, `sanitized-3`, ... — the same scheme
/// `launch::ensure_profile` uses when creating a brand-new profile.
fn next_available_slug(
    sanitized: &str,
    profiles: &[(String, crate::config::Profile)],
    exclude_slug: &str,
) -> String {
    let mut n = 1;
    let mut candidate = sanitized.to_string();
    while profiles
        .iter()
        .any(|(s, _)| s != exclude_slug && s == &candidate)
    {
        n += 1;
        candidate = format!("{sanitized}-{n}");
    }
    candidate
}

/// Renames a profile's folder on disk (`profiles/<slug>/` →
/// `profiles/<new-slug>/`) — safe to do independently of anything else:
/// `prefix::resolve` recomputes a per-exe prefix's own directory fresh
/// from the exe path every time, never from the profile's folder slug, so
/// this never touches (or orphans) an existing Wine prefix. Sanitizes and
/// disambiguates the requested text the same way a brand-new profile's
/// slug is derived (see `next_available_slug`), then moves the directory
/// and updates every place this session tracks the old slug (`app.profiles`,
/// and `app.profile_editor` if it's this very profile being edited) so the
/// editor keeps working on the same profile under its new name.
fn rename_slug(app: &mut App, slug: &str, requested: &str) {
    let sanitized = crate::prefix::sanitize(requested);
    if sanitized.is_empty() {
        app.status = Some("slug can't be blank — left unchanged.".to_string());
        return;
    }
    let candidate = next_available_slug(&sanitized, &app.profiles, slug);
    if candidate == slug {
        return; // nothing to rename
    }

    let dir = match crate::config::Profile::profiles_dir() {
        Ok(dir) => dir,
        Err(err) => {
            app.status = Some(format!("couldn't resolve profiles dir: {err:#}"));
            return;
        }
    };
    let old_dir = dir.join(slug);
    let new_dir = dir.join(&candidate);
    if new_dir.exists() {
        app.status = Some(format!(
            "\"{candidate}\" already exists on disk — left unchanged."
        ));
        return;
    }
    if let Err(err) = std::fs::rename(&old_dir, &new_dir) {
        app.status = Some(format!("couldn't rename profile folder: {err}"));
        return;
    }

    if let Some(entry) = app.profiles.iter_mut().find(|(s, _)| s == slug) {
        entry.0 = candidate.clone();
    }
    if app.profile_editor.as_deref() == Some(slug) {
        app.profile_editor = Some(candidate.clone());
    }
    app.status = Some(if candidate == sanitized {
        format!("Renamed to \"{candidate}\".")
    } else {
        format!("Renamed to \"{candidate}\" (\"{sanitized}\" was already taken).")
    });
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
    if field == ProfileField::Slug {
        rename_slug(app, slug, &trimmed);
        return;
    }
    // The auto-grown "#N" needs an immutable scan of every other profile,
    // so it's computed here, before `profile_mut`'s mutable borrow below.
    let computed_name = if field == ProfileField::Name {
        if trimmed.is_empty() {
            app.status = Some("name can't be blank — left unchanged.".to_string());
            return;
        }
        Some(next_available_name(&trimmed, &app.profiles, slug))
    } else {
        None
    };

    let Some(profile) = app.profile_mut(slug) else {
        app.status = Some(format!("couldn't find profile \"{slug}\""));
        return;
    };
    match field {
        ProfileField::TargetPath => profile.target_path = trimmed.clone(),
        ProfileField::Name => {
            profile.name = computed_name.expect("computed above for the Name field");
        }
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
        // `Slug` returns early above. The rest are never actually reached
        // via a text popup — they open a different mode
        // (`ProtonPicker`/`MapEditor`/`Cycle`) instead.
        ProfileField::Slug
        | ProfileField::Proton
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
            Mode::TextInput {
                purpose,
                buffer,
                cursor,
            } => {
                assert!(matches!(
                    purpose,
                    TextInputPurpose::ProfileField(slug, ProfileField::Title) if slug == "game-1"
                ));
                assert_eq!(buffer, "Real Title");
                assert_eq!(*cursor, buffer.chars().count(), "cursor starts at the end");
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

    fn profile_named(name: &str) -> Profile {
        Profile {
            name: name.to_string(),
            target_path: "/tmp/game.exe".to_string(),
            title: None,
            last_launched: None,
            args: Vec::new(),
            defaults: Default::default(),
            logging: Default::default(),
            env: Default::default(),
            winedlloverride: Default::default(),
        }
    }

    #[test]
    fn name_base_splits_on_the_last_hash() {
        assert_eq!(name_base("game#1"), "game");
        assert_eq!(name_base("no-hash-here"), "no-hash-here");
        assert_eq!(name_base("weird#name#2"), "weird#name"); // last '#' only
    }

    #[test]
    fn next_available_name_starts_at_1_when_nothing_collides() {
        let profiles = vec![("a".to_string(), profile_named("other#1"))];
        assert_eq!(next_available_name("game", &profiles, "a"), "game#1");
    }

    #[test]
    fn next_available_name_fills_the_lowest_free_slot_not_just_the_max() {
        // "game#1" was deleted/renamed away, only "game#2" remains — a
        // rename into "game" should reclaim "game#1", not jump to "game#3".
        let profiles = vec![("a".to_string(), profile_named("game#2"))];
        assert_eq!(next_available_name("game", &profiles, "b"), "game#1");
    }

    #[test]
    fn next_available_name_excludes_the_profile_being_renamed() {
        // Renaming a profile to the name it already has shouldn't collide
        // with itself.
        let profiles = vec![("a".to_string(), profile_named("game#1"))];
        assert_eq!(next_available_name("game", &profiles, "a"), "game#1");
    }

    #[test]
    fn next_available_slug_starts_bare_then_grows_with_a_hyphen() {
        let profiles = vec![("game".to_string(), profile_named("game#1"))];
        assert_eq!(next_available_slug("game", &profiles, "other"), "game-2");
        assert_eq!(
            next_available_slug("newname", &profiles, "other"),
            "newname"
        );
    }

    #[test]
    fn next_available_slug_excludes_the_profile_being_renamed() {
        let profiles = vec![("game".to_string(), profile_named("game#1"))];
        assert_eq!(next_available_slug("game", &profiles, "game"), "game");
    }
}
