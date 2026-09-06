use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Tabs};

use super::app::{
    App, ConfigField, InputKind, IntegrateField, MapEntryStep, MapField, Mode, ProfileField,
    ProtonPickerTarget, Tab, TextInputPurpose,
};

/// figlet, font "slant". Kept as literal art rather than generated at
/// runtime — it's decoration, not something that needs to adapt to the
/// actual terminal width (see `draw_header`, which just left-aligns it and
/// lets it clip on a narrower terminal like any other over-wide content).
const LOGO: [&str; 5] = [
    "    ________             __                           __  ",
    "   /  _/ __ \\_________  / /   ____ ___  ______  _____/ /_ ",
    "   / // /_/ / ___/ __ \\/ /   / __ `/ / / / __ \\/ ___/ __ \\",
    " _/ // ____/ /  / /_/ / /___/ /_/ / /_/ / / / / /__/ / / /",
    "/___/_/   /_/   \\____/_____/\\__,_/\\__,_/_/ /_/\\___/_/ /_/ ",
];

pub fn draw(frame: &mut Frame, app: &mut App) {
    let signature = marquee_signature(app);
    app.sync_marquee(signature);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8),
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(frame.area());

    draw_header(frame, chunks[0]);
    draw_tabs(frame, chunks[1], app.tab);

    match app.tab {
        Tab::Running => draw_running(frame, chunks[2], app),
        Tab::Library => match &app.profile_editor {
            Some(slug) => draw_profile_editor(frame, chunks[2], app, slug),
            None => draw_library(frame, chunks[2], app),
        },
        Tab::Config => draw_config(frame, chunks[2], app),
        Tab::Help => draw_help(frame, chunks[2], app),
    }

    draw_status_bar(frame, chunks[3], app);

    let tick = app.marquee_tick();
    match &app.mode {
        Mode::TextInput {
            purpose,
            buffer,
            cursor,
        } => draw_text_input_popup(frame, purpose, buffer, *cursor, tick),
        Mode::ProtonPicker {
            builds,
            selected,
            target,
        } => draw_proton_picker_popup(frame, builds, *selected, target, tick),
        Mode::MapEditor { field, selected } => {
            draw_map_editor_popup(frame, app, field, *selected, tick)
        }
        Mode::MapEntryInput {
            field,
            step,
            key,
            value,
            ..
        } => draw_map_entry_input_popup(frame, field, *step, key, value, tick),
        Mode::ConfirmDeleteProfile { name, .. } => {
            draw_confirm_delete_popup(frame, name, app.input_kind)
        }
        Mode::ConfirmRenameSlug {
            old_prefix_dir,
            new_prefix_dir,
            ..
        } => draw_confirm_rename_slug_popup(frame, old_prefix_dir, new_prefix_dir, app.input_kind),
        Mode::Help => draw_help_popup(frame, app),
        Mode::ConfirmWinetricks { name, .. } => {
            draw_confirm_winetricks_popup(frame, name, app.input_kind)
        }
        Mode::Normal => {}
    }
}

/// ASCII-art wordmark, left-aligned, with the version tucked into the
/// bottom-right of the same block (on the logo's own trailing blank row —
/// `Line::alignment` overrides the paragraph's default per-line, so this one
/// row can be right-aligned while the art above stays left-aligned).
fn draw_header(frame: &mut Frame, area: Rect) {
    let mut lines: Vec<Line> = LOGO.iter().map(|s| Line::from(*s)).collect();
    lines.push(Line::from(format!("v{}", env!("CARGO_PKG_VERSION"))).alignment(Alignment::Right));
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn draw_tabs(frame: &mut Frame, area: Rect, active: Tab) {
    let titles: Vec<Line> = Tab::ALL
        .iter()
        .enumerate()
        .map(|(i, t)| Line::from(format!("[{}] {}", i + 1, t.title())))
        .collect();
    let index = Tab::ALL.iter().position(|t| *t == active).unwrap_or(0);
    let tabs = Tabs::new(titles)
        .block(Block::default().borders(Borders::ALL))
        .select(index)
        .highlight_style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Yellow),
        );
    frame.render_widget(tabs, area);
}

fn draw_running(frame: &mut Frame, area: Rect, app: &App) {
    if app.running.is_empty() {
        frame.render_widget(
            Paragraph::new("Nothing running.").block(Block::default().borders(Borders::ALL)),
            area,
        );
        return;
    }
    let indices = app.filtered_running_indices();
    let normal_title = match app.input_kind {
        InputKind::Keyboard => "Running (Enter/k = kill, r = refresh, f = search)",
        InputKind::Gamepad => "Running (A/X = kill, L3 = refresh, Select = search)",
    };
    let title = filter_title(
        &app.running_filter,
        app.running_filter_editing,
        normal_title,
    );
    if indices.is_empty() {
        frame.render_widget(
            Paragraph::new("No match.").block(Block::default().borders(Borders::ALL).title(title)),
            area,
        );
        return;
    }
    let items: Vec<ListItem> = indices
        .iter()
        .map(|&i| {
            let e = &app.running[i];
            ListItem::new(format!("{}  [pid {}]  {}", e.name, e.pid, e.target_path))
        })
        .collect();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().bg(Color::DarkGray))
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, area, &mut list_state(app.running_selected));
}

/// Shared by `draw_running`/`draw_library`: while actively typing a
/// quick-search filter, the block's own title becomes the filter box
/// itself (trailing `_` cursor, same convention as the standalone
/// text-input popups) instead of the tab's normal key hint, since typing
/// is all that works then. Once locked (Enter pressed), every one of the
/// tab's normal shortcuts works again — so `normal_title` (which already
/// lists all of them) is kept alongside the filter status instead of being
/// replaced by it, otherwise there'd be nothing on screen reminding you
/// `a`/`e`/`d`/`c`/etc. are usable again.
fn filter_title(filter: &Option<String>, editing: bool, normal_title: &str) -> String {
    match (filter, editing) {
        (Some(text), true) => format!("Filter: {text}_  (Enter = lock, Esc = clear)"),
        (Some(text), false) => format!("{normal_title} — Filter: {text} (locked, Esc = clear)"),
        (None, _) => normal_title.to_string(),
    }
}

fn draw_library(frame: &mut Frame, area: Rect, app: &App) {
    if app.profiles.is_empty() {
        frame.render_widget(
            Paragraph::new("No games yet. Press 'a' to add one by exe path.")
                .block(Block::default().borders(Borders::ALL)),
            area,
        );
        return;
    }
    // 2 (border) + 2 ("> "/blank highlight_symbol column, reserved on every
    // row whether or not it's selected) — the space actually available for
    // list-item text inside the block.
    let inner_width = area.width.saturating_sub(4) as usize;
    let normal_title = match app.input_kind {
        InputKind::Keyboard => {
            "Library (Enter = launch, a = add, r = refresh, e = edit, d = delete, c = copy cmd, p = winetricks, f = search)"
        }
        InputKind::Gamepad => {
            "Library (A = launch, X = delete, L3 = refresh, R3 = winetricks, Select = search — add/edit/copy cmd need a keyboard)"
        }
    };
    let title = filter_title(
        &app.library_filter,
        app.library_filter_editing,
        normal_title,
    );

    let indices = app.filtered_profile_indices();
    if indices.is_empty() {
        frame.render_widget(
            Paragraph::new("No match.").block(Block::default().borders(Borders::ALL).title(title)),
            area,
        );
        return;
    }

    let tick = app.marquee_tick();
    let items: Vec<ListItem> = indices
        .iter()
        .enumerate()
        .map(|(display_index, &real_index)| {
            let (slug, p) = &app.profiles[real_index];
            let left = format!(
                "{}  [{slug}]  [{}]",
                p.name,
                shortened_parent_hint(&p.target_path)
            );
            // Hidden entirely (not "last launched: never") when it's never
            // actually been launched — nothing to report yet, so nothing
            // to show.
            let combined = match &p.last_launched {
                Some(last) => {
                    let right = format!("last launched: {last}");
                    let pad = inner_width
                        .saturating_sub(left.chars().count() + right.chars().count())
                        .max(1);
                    format!("{left}{:pad$}{right}", "")
                }
                None => left,
            };
            let text = if display_index == app.library_selected
                && combined.chars().count() > inner_width
            {
                marquee(&combined, inner_width, tick)
            } else {
                combined
            };
            ListItem::new(text)
        })
        .collect();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().bg(Color::DarkGray))
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, area, &mut list_state(app.library_selected));
}

/// The exe's last 2 parent directory names, backslash-joined and prefixed
/// with `..\` — enough context to tell apart two profiles that happen to
/// share an exe filename (e.g. `a\game.exe` vs `b\game.exe`) without having
/// to open the profile editor, deliberately truncated rather than showing
/// the full path (which would usually be too long for one list row). Only
/// real directory names count — a bare `/` (or a Windows drive prefix) at
/// the root isn't a meaningful "parent dir" the way `Downloads`/`Programs`
/// are, so it's filtered out rather than showing up as a literal `/`.
fn shortened_parent_hint(target_path: &str) -> String {
    let components: Vec<&str> = std::path::Path::new(target_path)
        .parent()
        .into_iter()
        .flat_map(|p| p.components())
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();
    let tail: Vec<&str> = components.iter().rev().take(2).rev().copied().collect();
    format!("..\\{}", tail.join("\\"))
}

fn draw_profile_editor(frame: &mut Frame, area: Rect, app: &App, slug: &str) {
    let Some(profile) = app.profile(slug) else {
        frame.render_widget(
            Paragraph::new(format!("Profile \"{slug}\" is gone (Esc to go back)."))
                .block(Block::default().borders(Borders::ALL)),
            area,
        );
        return;
    };

    let inner_width = area.width.saturating_sub(4) as usize;
    let tick = app.marquee_tick();
    let items: Vec<ListItem> = ProfileField::ALL
        .iter()
        .enumerate()
        .map(|(i, field)| {
            let value = match field {
                ProfileField::TargetPath => profile.target_path.clone(),
                ProfileField::Slug => slug.to_string(),
                ProfileField::Name => profile.name.clone(),
                ProfileField::Title => profile
                    .title
                    .clone()
                    .unwrap_or_else(|| "(inherit: exe filename)".to_string()),
                ProfileField::Args => {
                    if profile.args.is_empty() {
                        "(none)".to_string()
                    } else {
                        profile.args.join(" ")
                    }
                }
                ProfileField::Proton => profile
                    .defaults
                    .proton
                    .clone()
                    .unwrap_or_else(|| "(inherit)".to_string()),
                ProfileField::PrefixPath => profile
                    .defaults
                    .prefix_path
                    .clone()
                    .unwrap_or_else(|| "(inherit)".to_string()),
                ProfileField::WindowsVersion => profile
                    .defaults
                    .windows_version
                    .clone()
                    .unwrap_or_else(|| "(inherit)".to_string()),
                ProfileField::Gamescope => profile
                    .defaults
                    .gamescope
                    .map_or_else(|| "(inherit)".to_string(), |g| format!("{g:?}")),
                ProfileField::GamescopeOutputWidth => profile
                    .defaults
                    .gamescope_settings
                    .output_width
                    .map_or_else(|| "(inherit)".to_string(), |v| v.to_string()),
                ProfileField::GamescopeOutputHeight => profile
                    .defaults
                    .gamescope_settings
                    .output_height
                    .map_or_else(|| "(inherit)".to_string(), |v| v.to_string()),
                ProfileField::GamescopeRefresh => profile
                    .defaults
                    .gamescope_settings
                    .refresh
                    .map_or_else(|| "(inherit)".to_string(), |v| v.to_string()),
                ProfileField::GamescopeNestedWidth => profile
                    .defaults
                    .gamescope_settings
                    .nested_width
                    .map_or_else(|| "(inherit)".to_string(), |v| v.to_string()),
                ProfileField::GamescopeNestedHeight => profile
                    .defaults
                    .gamescope_settings
                    .nested_height
                    .map_or_else(|| "(inherit)".to_string(), |v| v.to_string()),
                ProfileField::GamescopeFilter => profile
                    .defaults
                    .gamescope_settings
                    .filter
                    .map_or_else(|| "(inherit)".to_string(), |f| format!("{f:?}")),
                ProfileField::GamescopeScaler => profile
                    .defaults
                    .gamescope_settings
                    .scaler
                    .map_or_else(|| "(inherit)".to_string(), |s| format!("{s:?}")),
                ProfileField::GamescopeBorderless => profile
                    .defaults
                    .gamescope_settings
                    .borderless
                    .map_or_else(|| "(inherit)".to_string(), |b| b.to_string()),
                ProfileField::GamescopeGrabCursor => profile
                    .defaults
                    .gamescope_settings
                    .grab_cursor
                    .map_or_else(|| "(inherit)".to_string(), |b| b.to_string()),
                ProfileField::GamescopeAdaptiveSync => profile
                    .defaults
                    .gamescope_settings
                    .adaptive_sync
                    .map_or_else(|| "(inherit)".to_string(), |b| b.to_string()),
                ProfileField::LogKeep => profile
                    .logging
                    .keep
                    .map_or_else(|| "(inherit)".to_string(), |k| k.to_string()),
                ProfileField::LogRecord => profile
                    .logging
                    .record
                    .map_or_else(|| "(inherit)".to_string(), |r| format!("{r:?}")),
                ProfileField::LogAutoOpen => profile
                    .logging
                    .auto_open
                    .map_or_else(|| "(inherit)".to_string(), |b| b.to_string()),
                ProfileField::EnvTable => entry_count(&profile.env),
                ProfileField::WineDllOverrideTable => entry_count(&profile.winedlloverride),
            };
            let combined = format!("{:<34} {}", field.label(), value);
            let text = if i == app.profile_field_selected && combined.chars().count() > inner_width
            {
                marquee(&combined, inner_width, tick)
            } else {
                combined
            };
            ListItem::new(text)
        })
        .collect();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(format!(
            "Editing {} [{slug}] (Enter = edit, blank = inherit, Esc = back to Library)",
            profile.name
        )))
        .highlight_style(Style::default().bg(Color::DarkGray))
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, area, &mut list_state(app.profile_field_selected));
}

/// The `'y'`/`'Y'` these destructive/rare confirm prompts key off is
/// produced by a literal `y` keypress, or (see `gamepad::translate`) RT —
/// deliberately not A/South, which every other popup already treats as
/// "confirm"; that's what keeps a stray button press from ever being enough
/// to delete/rename/run something here, matching keyboard Enter's identical
/// non-effect on these same prompts.
fn confirm_hint(input_kind: InputKind) -> &'static str {
    match input_kind {
        InputKind::Keyboard => "y = confirm, any other key = cancel",
        InputKind::Gamepad => "RT = confirm, any other button = cancel",
    }
}

fn draw_confirm_delete_popup(frame: &mut Frame, name: &str, input_kind: InputKind) {
    let area = centered_rect(60, 20, frame.area());
    frame.render_widget(Clear, area);
    let text = format!(
        "Delete \"{name}\"?\n\
Removes its profile.toml (settings/history) — not the exe itself.\n\n\
{}",
        confirm_hint(input_kind)
    );
    frame.render_widget(
        Paragraph::new(text).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Confirm delete"),
        ),
        area,
    );
}

fn draw_confirm_winetricks_popup(frame: &mut Frame, name: &str, input_kind: InputKind) {
    let area = centered_rect(60, 20, frame.area());
    frame.render_widget(Clear, area);
    let text = format!(
        "Launch winetricks for \"{name}\"?\n\
Runs against the exact same prefix a normal launch of this game would use.\n\n\
{}",
        confirm_hint(input_kind)
    );
    frame.render_widget(
        Paragraph::new(text).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Confirm winetricks"),
        ),
        area,
    );
}

fn draw_confirm_rename_slug_popup(
    frame: &mut Frame,
    old_prefix_dir: &std::path::Path,
    new_prefix_dir: &std::path::Path,
    input_kind: InputKind,
) {
    let area = centered_rect(76, 30, frame.area());
    frame.render_widget(Clear, area);
    let hint = match input_kind {
        InputKind::Keyboard => "y = confirm (renames both), any other key = cancel",
        InputKind::Gamepad => "RT = confirm (renames both), any other button = cancel",
    };
    let text = format!(
        "Renaming this slug also renames its prefix directory:\n\n  {}\n  → {}\n\n\
{hint}",
        old_prefix_dir.display(),
        new_prefix_dir.display()
    );
    frame.render_widget(
        Paragraph::new(text).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Confirm prefix rename"),
        ),
        area,
    );
}

fn draw_config(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(IntegrateField::ALL.len() as u16 + 2), // +2 for the block's own borders
        ])
        .split(area);

    draw_config_fields(frame, chunks[0], app);
    draw_integrate_table(frame, chunks[1], app);
}

fn draw_config_fields(frame: &mut Frame, area: Rect, app: &App) {
    let d = &app.cfg.defaults;
    let l = &app.cfg.logging;
    let g = &app.cfg.gamedb;
    let inner_width = area.width.saturating_sub(4) as usize;
    let tick = app.marquee_tick();

    let items: Vec<ListItem> = ConfigField::ALL
        .iter()
        .enumerate()
        .map(|(i, field)| {
            let value = match field {
                ConfigField::Proton => d.proton.clone(),
                ConfigField::PrefixMode => format!("{:?}", d.prefix_mode),
                ConfigField::PrefixPath => d.prefix_path.clone(),
                ConfigField::PrefixesRoot => d.prefixes_root.clone(),
                ConfigField::WindowsVersion => d.windows_version.clone(),
                ConfigField::Gamescope => format!("{:?}", d.gamescope),
                ConfigField::GamescopeOutputWidth => d
                    .gamescope_settings
                    .output_width
                    .map_or_else(|| "(unset)".to_string(), |v| v.to_string()),
                ConfigField::GamescopeOutputHeight => d
                    .gamescope_settings
                    .output_height
                    .map_or_else(|| "(unset)".to_string(), |v| v.to_string()),
                ConfigField::GamescopeRefresh => d
                    .gamescope_settings
                    .refresh
                    .map_or_else(|| "(unset)".to_string(), |v| v.to_string()),
                ConfigField::GamescopeNestedWidth => d
                    .gamescope_settings
                    .nested_width
                    .map_or_else(|| "(unset)".to_string(), |v| v.to_string()),
                ConfigField::GamescopeNestedHeight => d
                    .gamescope_settings
                    .nested_height
                    .map_or_else(|| "(unset)".to_string(), |v| v.to_string()),
                ConfigField::GamescopeFilter => d
                    .gamescope_settings
                    .filter
                    .map_or_else(|| "(unset)".to_string(), |f| format!("{f:?}")),
                ConfigField::GamescopeScaler => d
                    .gamescope_settings
                    .scaler
                    .map_or_else(|| "(unset)".to_string(), |s| format!("{s:?}")),
                ConfigField::GamescopeBorderless => d
                    .gamescope_settings
                    .borderless
                    .map_or_else(|| "(unset)".to_string(), |b| b.to_string()),
                ConfigField::GamescopeGrabCursor => d
                    .gamescope_settings
                    .grab_cursor
                    .map_or_else(|| "(unset)".to_string(), |b| b.to_string()),
                ConfigField::GamescopeAdaptiveSync => d
                    .gamescope_settings
                    .adaptive_sync
                    .map_or_else(|| "(unset)".to_string(), |b| b.to_string()),
                ConfigField::LogMode => format!("{:?}", l.mode),
                ConfigField::LogPath => l.path.clone(),
                ConfigField::LogKeep => l.keep.to_string(),
                ConfigField::LogRecord => format!("{:?}", l.record),
                ConfigField::LogAutoOpen => l.auto_open.to_string(),
                ConfigField::GamedbInterval => g.update_interval_days.to_string(),
                ConfigField::EnvTable => entry_count(&app.cfg.env),
                ConfigField::WineDllOverrideTable => entry_count(&app.cfg.winedlloverride),
            };
            let combined = format!("{:<34} {}", field.label(), value);
            let text = if i == app.config_selected && combined.chars().count() > inner_width {
                marquee(&combined, inner_width, tick)
            } else {
                combined
            };
            ListItem::new(text)
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Config (Enter = edit/cycle, Left/Right = adjust number)"),
        )
        .highlight_style(Style::default().bg(Color::DarkGray))
        .highlight_symbol("> ");
    frame.render_stateful_widget(
        list,
        area,
        &mut config_row_state(app, 0, ConfigField::ALL.len()),
    );
}

fn draw_integrate_table(frame: &mut Frame, area: Rect, app: &App) {
    let inner_width = area.width.saturating_sub(4) as usize;
    let tick = app.marquee_tick();
    let selected_local = app.config_selected.checked_sub(ConfigField::ALL.len());

    let items: Vec<ListItem> = IntegrateField::ALL
        .iter()
        .enumerate()
        .map(|(i, field)| {
            let value = match field {
                IntegrateField::Status => integration_status_text(),
                IntegrateField::BinaryPath => crate::integrate::registered_binary_path()
                    .unwrap_or_else(|| "(not installed)".to_string()),
                IntegrateField::Setup => "Enter = register as default handler, add to app menu, install icon, add right-click \"Add to Library\"".to_string(),
                IntegrateField::Reapply => {
                    "Enter = re-point the registration at this binary's current path".to_string()
                }
                IntegrateField::Uninstall => {
                    "Enter = remove the registration and restore the prior default".to_string()
                }
            };
            let combined = format!("{:<34} {}", field.label(), value);
            let text = if selected_local == Some(i) && combined.chars().count() > inner_width {
                marquee(&combined, inner_width, tick)
            } else {
                combined
            };
            ListItem::new(text)
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Desktop integration"),
        )
        .highlight_style(Style::default().bg(Color::DarkGray))
        .highlight_symbol("> ");
    frame.render_stateful_widget(
        list,
        area,
        &mut config_row_state(app, ConfigField::ALL.len(), IntegrateField::ALL.len()),
    );
}

/// `app.config_selected` spans both of the Config tab's tables as one
/// continuous index — this picks out which (if either) row within a table
/// starting at `offset` (with `len` rows) is currently selected, so only
/// one of the two tables ever shows a highlighted row at a time.
fn config_row_state(app: &App, offset: usize, len: usize) -> ratatui::widgets::ListState {
    let mut state = ratatui::widgets::ListState::default();
    if app.config_selected >= offset && app.config_selected < offset + len {
        state.select(Some(app.config_selected - offset));
    }
    state
}

const HELP_TEXT: &str = "\
iprolaunch — TUI help

Global:
  1/2/3/4, Tab/Shift-Tab   switch screen (also clears an active Running/
                           Library quick-search, locked or still typing)
  ?                        open this help as a popup from any tab (Esc closes it)
  Up/Down/PageUp/PageDown/Home/End   scroll (Help tab and the ? popup)
  q                        quit

Running:
  Enter or k               kill the selected launch
  r                        refresh now (also refreshes automatically)
  f                        quick-search — filters by name as you type;
                           while typing, only Enter/Esc/Up/Down work (r/k
                           become literal search characters instead).
                           Enter *locks* the search instead of killing —
                           the narrowed list stays, but r/k/Enter/Up/Down
                           all go back to normal, now scoped to it. Esc
                           clears it entirely, whether still typing or locked.

Library:
  Enter                    launch the selected game
  a                        add a game by typing its exe path
  r                        refresh the list from disk
  e                        edit the selected game's profile overrides
  d                        delete the selected game's profile (confirms first)
  c                        copy a quick-launch command to the clipboard —
                           \"<this binary's path>\" <slug> — for pasting into
                           a Steam non-Steam-game shortcut's Target field
  p                        run winetricks against this game's own prefix
                           (confirms first) — uses the profile's proton
                           override if in defaults.prefix_mode = per-slug,
                           otherwise the single shared prefix/Proton pair,
                           exactly like a real launch of it would
  f                        quick-search — filters by name as you type;
                           while typing, only Enter/Esc/Up/Down work
                           (a/r/e/d/c/p become literal search characters
                           instead). Enter *locks* the search instead of
                           launching — the narrowed list stays, but
                           a/r/e/d/c/p/Enter/Up/Down all go back to normal,
                           now scoped to it. Esc clears it entirely,
                           whether still typing or locked.
  Esc                      cancel while typing a path, or clear an active
                           quick-search (typing or locked)

Profile editor (Library, after 'e'):
  Enter                    edit (text fields), cycle (record/auto_open),
                           opens a picker for the proton override, or opens
                           the entry list for env/winedlloverride
  Esc                      back to the Library list
  A blank text field / the picker's \"inherit\" choice clears that override
  back to the global default. Changes save to that profile.toml immediately.
  target-path/slug/name are exceptions, not overrides:
    - target-path is mandatory, checked against the real filesystem on
      save, and rejected (left unchanged) if the exe isn't there.
    - slug is the profile's folder name — edit just the base text; renames
      the folder on disk, auto-appending \"-N\" only if that exact text is
      already taken by another profile (never something you type yourself).
      In defaults.prefix_mode = per-slug, if a prefix directory already
      exists under the old slug, you're asked to confirm first — renaming
      moves that directory too, since prefixes are keyed by slug.
    - name is what the Library list shows — edit just the base text (its
      \"#N\" is stripped for editing and never shown in the box); saving
      auto-fills the lowest \"#N\" not already used by another profile's
      same base, reusing a gap left by a deleted/renamed one rather than
      always growing past the historical max.
    - the proton override only has any effect in defaults.prefix_mode =
      per-slug — in single-prefix mode it's ignored (every profile shares
      one prefix, so a mismatched Proton version there risks corrupting it).
    - gamescope override cycles inherit -> none -> fullscreen -> maximize ->
      inherit — same as -f/-w on the command line, remembered per game so
      \"iprolaunch <slug>\" doesn't need retyping it (an explicit -f/-w still
      wins if passed). -b/borderless is independent of this (not mutually
      exclusive with fullscreen/maximize) — see gamescope_settings.borderless.
    - gamescope_settings.* overrides (output_width/output_height/refresh/
      nested_width/nested_height/filter/scaler/borderless/grab_cursor/
      adaptive_sync) only matter once gamescope is actually wrapping the
      launch (-f/-w/-b, or the gamescope override above) — blank means
      \"don't pass that flag, let gamescope pick.\" output_* is gamescope's
      real output size (-W/-H) — gamescope only auto-detects this when it
      owns the display directly, not nested inside an existing desktop
      session, so this is the fix for -w producing a small window on a
      normal desktop. nested_* (-w/-h, gamescope's own flags — not to be
      confused with iprolaunch's own -w/--maximize) is the game's own
      internal render resolution; filter (-F, linear/nearest/fsr/nis/pixel)
      and scaler (-S, auto/integer/fit/fill/stretch) together control the
      upscale used when nested/output differ; borderless (-b) merges with
      the CLI -b flag — either one turns it on for that launch; grab_cursor
      (--force-grab-cursor, relative mouse mode) and adaptive_sync
      (--adaptive-sync, VRR) are config-only, no CLI flag.

Config:
  Enter                    edit (text fields), cycle (mode/record), or
                           toggle (auto_open); opens a picker for proton;
                           opens the entry list for env/winedlloverride
  Left/Right               adjust a number field
  Esc                      cancel a text edit without saving
  Up/Down flow from the field list straight into the separate
  \"Desktop integration\" table below it — one shared cursor, two blocks.
  Changes save to config.toml immediately.

Desktop integration (Config tab, bottom table):
  status / binary location  info only, not editable
  setup                      register iprolaunch as the default .exe/.bat/.cmd/.msi
                             handler, add it to the app/start menu, install its icon,
                             and add a file-manager right-click \"Add to IProLaunch
                             Library\" action (KDE/GNOME/Cinnamon/MATE/XFCE, whichever
                             are actually present — see `iprolaunch context-menu`
                             for installing/removing just one of those on their own)
  reapply                    re-point all of the above at this binary's
                             current path, without touching the saved
                             backup of what the default was before setup
  uninstall                  remove all of the above and restore that backup

env / winedlloverride entry list (global or per-profile):
  a                        add an entry (prompts for name, then value)
  e                        edit the selected entry
  d                        delete the selected entry
  Esc                      back to whichever screen opened it

Gamepad (Steam Deck Game Mode, or any plain controller):
  D-pad                    Up/Down/Left/Right
  A                        confirm (same as Enter)
  B                        cancel (same as Esc)
  X                        the destructive/contextual action — kill
                           (Running) or delete a profile, with the same
                           confirm prompt (Library)
  Y                        help (opens this popup from anywhere)
  LB / RB                  previous / next tab
  L3 (left stick click)    refresh
  R3 (right stick click)   winetricks (Library)
  RT                       the literal \"y\" a delete/rename/winetricks
                           confirm prompt needs — deliberately not A, so
                           mashing confirm can never delete anything by
                           accident (same as Enter alone not confirming
                           these on a keyboard either)
  Select                   quick-search
  Start                    quit
  Every title/status bar shows the matching set of captions once a gamepad
  button is used, and switches back the moment a real key is pressed.
  Adding a game by path, editing a profile's text fields, and copying the
  quick-launch command still need a keyboard (typing isn't something a
  gamepad can do) — bind a spare button through Steam Input's own remapper
  straight to the matching letter key if you want those one-button too;
  iprolaunch doesn't need to know the difference.

Config file: ~/.config/iprolaunch/config.toml
Profiles:    ~/.config/iprolaunch/profiles/<slug>/profile.toml
";

pub fn help_text_line_count() -> u16 {
    HELP_TEXT.lines().count() as u16
}

/// Clamps a requested scroll offset against the actual visible height of a
/// bordered block — `help_scroll` itself is only soft-clamped against the
/// total line count (see `mod::help_scroll_key`), so this is what stops it
/// from scrolling past the real end into blank space.
fn help_max_scroll(area_height: u16) -> u16 {
    help_text_line_count().saturating_sub(area_height.saturating_sub(2))
}

fn draw_help(frame: &mut Frame, area: Rect, app: &App) {
    let scroll = app.help_scroll.min(help_max_scroll(area.height));
    frame.render_widget(
        Paragraph::new(HELP_TEXT)
            .block(Block::default().borders(Borders::ALL).title("Help"))
            .scroll((scroll, 0)),
        area,
    );
}

/// The `?` popup — same content as the Help tab, in a large centered
/// overlay so it's usable from any tab without switching away from it.
fn draw_help_popup(frame: &mut Frame, app: &App) {
    let area = centered_rect(90, 90, frame.area());
    frame.render_widget(Clear, area);
    let scroll = app.help_scroll.min(help_max_scroll(area.height));
    frame.render_widget(
        Paragraph::new(HELP_TEXT)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Help (Esc to close)"),
            )
            .scroll((scroll, 0)),
        area,
    );
}

/// Global keys that work from (almost) anywhere — shown in the status bar
/// whenever there's no real status message to display, so they're always
/// visible without needing to check the Help tab/popup for them
/// specifically.
const GLOBAL_KEY_HINTS: &str = "q = quit   ? = help   1-4 / Tab / Shift-Tab = switch tabs";
const GLOBAL_KEY_HINTS_GAMEPAD: &str = "Start = quit   Y = help   LB/RB = switch tabs";

fn draw_status_bar(frame: &mut Frame, area: Rect, app: &App) {
    let hints = match app.input_kind {
        InputKind::Keyboard => GLOBAL_KEY_HINTS,
        InputKind::Gamepad => GLOBAL_KEY_HINTS_GAMEPAD,
    };
    let text = match &app.status {
        Some(s) => Span::styled(s.clone(), Style::default().fg(Color::Yellow)),
        None => Span::styled(hints, Style::default().fg(Color::DarkGray)),
    };
    frame.render_widget(Paragraph::new(Line::from(text)), area);
}

fn draw_text_input_popup(
    frame: &mut Frame,
    purpose: &TextInputPurpose,
    buffer: &str,
    cursor: usize,
    tick: usize,
) {
    let area = centered_rect(60, 20, frame.area());
    frame.render_widget(Clear, area);
    let title = match purpose {
        TextInputPurpose::ConfigField(field) => {
            format!("Edit {} (Enter = save, Esc = cancel)", field.label())
        }
        TextInputPurpose::AddLibraryPath => {
            "Path to .exe (Enter = add & launch, Esc = cancel)".to_string()
        }
        TextInputPurpose::ProfileTitle(_) => {
            "Game's real title, for GAMEID matching (Enter = save, blank = skip, Esc = skip)"
                .to_string()
        }
        TextInputPurpose::ProfileField(_, ProfileField::TargetPath) => {
            "Edit target-path (Enter = save, checked against disk — Esc = cancel)".to_string()
        }
        TextInputPurpose::ProfileField(_, ProfileField::Slug) => {
            "Edit slug — folder name (Enter = save & rename on disk, auto-disambiguated if taken, Esc = cancel)"
                .to_string()
        }
        TextInputPurpose::ProfileField(_, ProfileField::Name) => {
            "Edit name — \"#N\" is auto-managed, don't type it (Enter = save, Esc = cancel)"
                .to_string()
        }
        TextInputPurpose::ProfileField(_, field) => {
            format!(
                "Edit {} (Enter = save, blank = inherit, Esc = cancel)",
                field.label()
            )
        }
    };
    let title = marquee_title(title, area.width, tick);
    let block = Block::default().borders(Borders::ALL).title(title);
    frame.render_widget(
        Paragraph::new(cursor_line(buffer, cursor)).block(block),
        area,
    );
}

/// Renders `buffer` with a reverse-video block over the character at
/// `cursor` (or a trailing reversed space when the cursor sits past the
/// last character) — a text-editor-style block cursor, so Left/Right
/// movement (see `mod::handle_text_input`) has something to actually show
/// where it landed, not just always-append-at-the-end like before.
fn cursor_line(buffer: &str, cursor: usize) -> Line<'static> {
    let chars: Vec<char> = buffer.chars().collect();
    let cursor = cursor.min(chars.len());
    let before: String = chars[..cursor].iter().collect();
    let (at, after): (String, String) = if cursor < chars.len() {
        (
            chars[cursor].to_string(),
            chars[cursor + 1..].iter().collect(),
        )
    } else {
        (" ".to_string(), String::new())
    };
    Line::from(vec![
        Span::raw(before),
        Span::styled(at, Style::default().add_modifier(Modifier::REVERSED)),
        Span::raw(after),
    ])
}

fn draw_proton_picker_popup(
    frame: &mut Frame,
    builds: &[crate::proton::ProtonBuild],
    selected: usize,
    target: &ProtonPickerTarget,
    tick: usize,
) {
    let area = centered_rect(60, 60, frame.area());
    frame.render_widget(Clear, area);

    let mut items = Vec::new();
    let title = match target {
        ProtonPickerTarget::Global => {
            items.push(ListItem::new(
                "system  (let umu-run auto-manage UMU-Proton)",
            ));
            "Pick a Proton build (Enter, Esc = cancel)".to_string()
        }
        ProtonPickerTarget::Profile(slug) => {
            items.push(ListItem::new("(inherit — use the global default)"));
            items.push(ListItem::new(
                "system  (let umu-run auto-manage UMU-Proton)",
            ));
            format!("Pick a Proton override for {slug} (Enter, Esc = cancel)")
        }
    };
    let title = marquee_title(title, area.width, tick);
    items.extend(
        builds
            .iter()
            .map(|b| ListItem::new(format!("{}  [{}]", b.display_name, b.id))),
    );
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().bg(Color::DarkGray))
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, area, &mut list_state(selected));
}

fn entry_count(map: &std::collections::BTreeMap<String, String>) -> String {
    match map.len() {
        0 => "(empty)".to_string(),
        1 => "1 entry".to_string(),
        n => format!("{n} entries"),
    }
}

fn integration_status_text() -> String {
    if crate::integrate::is_installed() {
        "installed".to_string()
    } else {
        "not installed".to_string()
    }
}

fn draw_map_editor_popup(
    frame: &mut Frame,
    app: &App,
    field: &MapField,
    selected: usize,
    tick: usize,
) {
    let area = centered_rect(70, 60, frame.area());
    frame.render_widget(Clear, area);

    let entries = app.map_entries(field);
    let items: Vec<ListItem> = if entries.is_empty() {
        vec![ListItem::new("(empty — press 'a' to add an entry)")]
    } else {
        entries
            .iter()
            .map(|(k, v)| ListItem::new(format!("{k}={v}")))
            .collect()
    };
    let title = marquee_title(
        format!("{} — a add, e edit, d delete, Esc back", field.label()),
        area.width,
        tick,
    );
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().bg(Color::DarkGray))
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, area, &mut list_state(selected));
}

fn draw_map_entry_input_popup(
    frame: &mut Frame,
    field: &MapField,
    step: MapEntryStep,
    key: &str,
    value: &str,
    tick: usize,
) {
    let area = centered_rect(60, 20, frame.area());
    frame.render_widget(Clear, area);

    let (title, text) = match step {
        MapEntryStep::Key => (
            format!(
                "{} — variable name (Enter = next, Esc = cancel)",
                field.label()
            ),
            format!("{key}_"),
        ),
        MapEntryStep::Value => (
            format!(
                "{} — value for \"{key}\" (Enter = save, Esc = cancel)",
                field.label()
            ),
            format!("{value}_"),
        ),
    };
    let title = marquee_title(title, area.width, tick);
    frame.render_widget(
        Paragraph::new(text).block(Block::default().borders(Borders::ALL).title(title)),
        area,
    );
}

fn list_state(selected: usize) -> ratatui::widgets::ListState {
    let mut state = ratatui::widgets::ListState::default();
    state.select(Some(selected));
    state
}

/// A cheap identifier for "whatever might currently be marquee-scrolling":
/// changes exactly when the user has moved to a different row or a
/// different popup/field (tab switched, a list selection moved, a popup
/// opened or its own internal selection/field/step changed, the profile
/// editor opened a different profile) — never on something that doesn't
/// affect what's selected, like a `TextInput`'s `buffer` changing as the
/// user types, or a `ProtonPicker`'s `builds` list (which is why this
/// matches `Mode` by hand instead of just using its `Debug` output).
/// `App::sync_marquee` resets the scroll-delay timer whenever this changes.
fn marquee_signature(app: &App) -> String {
    let mode_part = match &app.mode {
        Mode::Normal => "normal".to_string(),
        Mode::TextInput { purpose, .. } => format!("text:{purpose:?}"),
        Mode::ProtonPicker {
            selected, target, ..
        } => format!("proton:{selected}:{target:?}"),
        Mode::MapEditor { field, selected } => format!("map:{field:?}:{selected}"),
        Mode::MapEntryInput { field, step, .. } => format!("mapentry:{field:?}:{step:?}"),
        Mode::ConfirmDeleteProfile { slug, .. } => format!("confirmdelete:{slug}"),
        Mode::ConfirmRenameSlug {
            slug, candidate, ..
        } => {
            format!("confirmrenameslug:{slug}:{candidate}")
        }
        Mode::Help => "help".to_string(),
        Mode::ConfirmWinetricks { slug, .. } => format!("confirmwinetricks:{slug}"),
    };
    format!(
        "{:?}|{}|{}|{}|{:?}|{:?}|{:?}|{mode_part}",
        app.tab,
        app.library_selected,
        app.config_selected,
        app.profile_field_selected,
        app.profile_editor,
        app.library_filter,
        app.running_filter,
    )
}

/// Scrolls `text` to fit within `width` characters when it's too long to
/// otherwise, animated by `tick` (see `App::marquee_tick`) — used for a
/// selected row or a popup title too long for a small terminal, so the
/// full text is still readable over a couple of seconds instead of being
/// silently clipped. Text that already fits is returned unchanged (no
/// pointless scrolling of something that's already fully visible).
fn marquee(text: &str, width: usize, tick: usize) -> String {
    let width = width.max(1);
    if text.chars().count() <= width {
        return text.to_string();
    }
    // A gap between one lap and the next, so wrap-around reads as
    // "...end    start..." instead of "...endstart..." running together.
    const GAP: &str = "    ";
    let padded: Vec<char> = text.chars().chain(GAP.chars()).collect();
    let start = tick % padded.len();
    padded.iter().cycle().skip(start).take(width).collect()
}

/// `marquee`, specialized for a popup's own `Block` title — takes the
/// popup's full `area.width` and accounts for its 2 border columns itself,
/// so every popup title call site doesn't have to repeat that subtraction.
fn marquee_title(title: String, area_width: u16, tick: usize) -> String {
    let width = area_width.saturating_sub(2) as usize;
    if title.chars().count() > width {
        marquee(&title, width, tick)
    } else {
        title
    }
}

/// Standard ratatui recipe for a centered floating popup.
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::config::Config;
    use crate::proton::ProtonBuild;

    fn rendered(app: &mut App, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    fn test_app() -> App {
        App::new(Config::default())
    }

    #[test]
    fn marquee_signature_differs_when_the_selected_row_changes() {
        let mut app = test_app();
        app.tab = Tab::Config;
        app.config_selected = 0;
        let a = marquee_signature(&app);
        app.config_selected = 1;
        let b = marquee_signature(&app);
        assert_ne!(a, b);
    }

    #[test]
    fn marquee_signature_ignores_a_text_inputs_buffer_contents() {
        // Typing shouldn't reset the marquee delay on the popup's own
        // title — only *which* field is open should matter.
        let mut app = test_app();
        app.mode = Mode::TextInput {
            purpose: super::super::app::TextInputPurpose::AddLibraryPath,
            buffer: "a".to_string(),
            cursor: 1,
        };
        let a = marquee_signature(&app);
        app.mode = Mode::TextInput {
            purpose: super::super::app::TextInputPurpose::AddLibraryPath,
            buffer: "a longer buffer now".to_string(),
            cursor: 19,
        };
        let b = marquee_signature(&app);
        assert_eq!(a, b);
    }

    #[test]
    fn marquee_leaves_short_text_unchanged() {
        assert_eq!(marquee("short", 20, 0), "short");
        assert_eq!(marquee("short", 20, 500), "short"); // tick doesn't matter either
    }

    #[test]
    fn marquee_scrolls_long_text_and_advances_with_tick() {
        let text = "this text is definitely longer than the width";
        let width = 10;
        let at_0 = marquee(text, width, 0);
        let at_1 = marquee(text, width, 1);
        assert_eq!(at_0.chars().count(), width);
        assert_eq!(at_1.chars().count(), width);
        assert_ne!(at_0, at_1, "advancing tick should shift the visible window");
        assert!(text.starts_with(&at_0));
    }

    #[test]
    fn marquee_wraps_around_via_the_gap_back_to_the_start() {
        let text = "abcdef";
        let width = 3;
        // 6 chars + 4-char gap = 10-char cycle; tick == cycle length should
        // land back exactly where tick == 0 did.
        assert_eq!(marquee(text, width, 0), marquee(text, width, 10));
    }

    #[test]
    fn marquee_title_accounts_for_the_2_border_columns() {
        let long = "a very long popup title that will not fit";
        // area.width - 2 (borders) == 10, so this should scroll, not just
        // pass the whole (over-)long title through untouched.
        let scrolled = marquee_title(long.to_string(), 12, 0);
        assert_eq!(scrolled.chars().count(), 10);
        // Comfortably wide: title fits once borders are subtracted, so it's
        // returned untouched.
        assert_eq!(marquee_title("short".to_string(), 20, 0), "short");
    }

    #[test]
    fn header_shows_the_wordmark_and_version() {
        // Regression check: the header block must be tall enough for the
        // logo *plus* the version line — sized for just the logo once and
        // silently clipped the version off until caught by manual testing.
        let mut app = test_app();
        let out = rendered(&mut app, 80, 24);
        assert!(
            out.contains(LOGO[0].trim()),
            "ASCII wordmark's first line should render"
        );
        assert!(
            out.contains(env!("CARGO_PKG_VERSION")),
            "version should render, got:\n{out}"
        );
    }

    #[test]
    fn every_tab_renders_without_panicking_at_a_normal_size() {
        let mut app = test_app();
        for tab in Tab::ALL {
            app.tab = tab;
            let out = rendered(&mut app, 80, 24);
            assert!(out.contains(tab.title()), "tab bar should show {:?}", tab);
        }
    }

    #[test]
    fn every_tab_renders_without_panicking_at_a_small_size() {
        // Guards against a layout/percentage split panicking on a
        // cramped terminal (a real risk with Ratatui's Layout constraints).
        let mut app = test_app();
        for tab in Tab::ALL {
            app.tab = tab;
            rendered(&mut app, 20, 6);
        }
    }

    #[test]
    fn empty_running_tab_shows_placeholder_text() {
        let mut app = test_app();
        app.tab = Tab::Running;
        app.running.clear();
        assert!(rendered(&mut app, 80, 24).contains("Nothing running"));
    }

    #[test]
    fn empty_library_tab_shows_placeholder_text() {
        let mut app = test_app();
        app.tab = Tab::Library;
        app.profiles.clear();
        assert!(rendered(&mut app, 80, 24).contains("No games yet"));
    }

    #[test]
    fn library_row_shows_the_shortened_parent_hint_and_right_aligns_last_launched() {
        let mut app = test_app();
        app.tab = Tab::Library;
        let mut profile = test_profile("ktsysview#1");
        profile.target_path = "/media/media/Downloads/Programs/KTSYSVIEW.exe".to_string();
        profile.last_launched = Some("06-Sep-2026, 13:04:45".to_string());
        app.profiles = vec![("ktsysview".to_string(), profile)];
        let out = rendered(&mut app, 100, 24);
        assert!(out.contains("[..\\Downloads\\Programs]"));
        // Right-aligned: "last launched:" should land near the row's right
        // edge, not immediately after the rest of the row's content.
        let idx = out
            .find("last launched:")
            .expect("last launched should render");
        let row_start = out[..idx].rfind("ktsysview#1").unwrap();
        assert!(
            idx - row_start >= 60,
            "expected last launched to be pushed toward the right edge, gap was {}",
            idx - row_start
        );
    }

    #[test]
    fn library_row_hides_last_launched_entirely_when_never_launched() {
        let mut app = test_app();
        app.tab = Tab::Library;
        // test_profile()'s last_launched is already None — the case that
        // matters here.
        app.profiles = vec![("ktsysview".to_string(), test_profile("ktsysview#1"))];
        let out = rendered(&mut app, 100, 24);
        assert!(out.contains("ktsysview#1"));
        assert!(!out.contains("last launched"));
    }

    #[test]
    fn config_tab_lists_every_field_label() {
        let mut app = test_app();
        app.tab = Tab::Config;
        // Tall enough for both the main field list and the separate
        // "Desktop integration" table below it (see `draw_config`) — a
        // shorter terminal will legitimately clip content, same as any
        // other list-heavy screen; that's covered by
        // `every_tab_renders_without_panicking_at_a_small_size` instead.
        let out = rendered(&mut app, 100, 46);
        for field in ConfigField::ALL {
            assert!(
                out.contains(field.label()),
                "missing label: {}",
                field.label()
            );
        }
        for field in IntegrateField::ALL {
            assert!(
                out.contains(field.label()),
                "missing integrate label: {}",
                field.label()
            );
        }
    }

    #[test]
    fn text_input_popup_renders_current_buffer() {
        let mut app = test_app();
        app.mode = Mode::TextInput {
            purpose: super::super::app::TextInputPurpose::AddLibraryPath,
            buffer: "hello-world".to_string(),
            cursor: 11,
        };
        assert!(rendered(&mut app, 80, 24).contains("hello-world"));
    }

    #[test]
    fn profile_title_popup_explains_what_its_for_not_just_generic_edit() {
        let mut app = test_app();
        app.mode = Mode::TextInput {
            purpose: super::super::app::TextInputPurpose::ProfileTitle("some-slug".to_string()),
            buffer: String::new(),
            cursor: 0,
        };
        let out = rendered(&mut app, 80, 24);
        assert!(out.contains("real title"));
        assert!(out.contains("GAMEID"));
    }

    fn test_profile(name: &str) -> crate::config::Profile {
        crate::config::Profile {
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
    fn profile_editor_lists_every_field_label() {
        let mut app = test_app();
        app.tab = Tab::Library;
        app.profiles = vec![("game-1".to_string(), test_profile("Game#1"))];
        app.profile_editor = Some("game-1".to_string());
        // Tall enough to fit every field row without clipping — see
        // `config_tab_lists_every_field_label`'s identical reasoning.
        let out = rendered(&mut app, 100, 40);
        assert!(out.contains("Game#1"));
        for field in ProfileField::ALL {
            assert!(
                out.contains(field.label()),
                "missing label: {}",
                field.label()
            );
        }
    }

    #[test]
    fn confirm_delete_popup_names_the_profile() {
        let mut app = test_app();
        app.mode = Mode::ConfirmDeleteProfile {
            slug: "game-1".to_string(),
            name: "Game#1".to_string(),
        };
        let out = rendered(&mut app, 80, 24);
        assert!(out.contains("Game#1"));
    }

    #[test]
    fn proton_picker_popup_lists_system_and_every_build() {
        let mut app = test_app();
        app.mode = Mode::ProtonPicker {
            builds: vec![ProtonBuild {
                id: "GE-Proton10-34".to_string(),
                display_name: "GE-Proton10-34".to_string(),
            }],
            selected: 0,
            target: super::super::app::ProtonPickerTarget::Global,
        };
        let out = rendered(&mut app, 80, 24);
        assert!(out.contains("system"));
        assert!(out.contains("GE-Proton10-34"));
    }
}
