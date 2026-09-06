use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Tabs};

use super::app::{
    App, ConfigField, IntegrateField, MapEntryStep, MapField, Mode, ProfileField,
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
        Tab::Help => draw_help(frame, chunks[2]),
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
        Mode::ConfirmDeleteProfile { name, .. } => draw_confirm_delete_popup(frame, name),
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
    let items: Vec<ListItem> = app
        .running
        .iter()
        .map(|e| ListItem::new(format!("{}  [pid {}]  {}", e.name, e.pid, e.target_path)))
        .collect();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Running (Enter/k = kill, r = refresh)"),
        )
        .highlight_style(Style::default().bg(Color::DarkGray))
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, area, &mut list_state(app.running_selected));
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

    let tick = app.marquee_tick();
    let items: Vec<ListItem> = app
        .profiles
        .iter()
        .enumerate()
        .map(|(i, (slug, p))| {
            let last = p.last_launched.as_deref().unwrap_or("never");
            let left = format!(
                "{}  [{slug}]  [{}]",
                p.name,
                shortened_parent_hint(&p.target_path)
            );
            let right = format!("last launched: {last}");
            let pad = inner_width
                .saturating_sub(left.chars().count() + right.chars().count())
                .max(1);
            let combined = format!("{left}{:pad$}{right}", "");
            let text = if i == app.library_selected && combined.chars().count() > inner_width {
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
                .title("Library (Enter = launch, a = add, r = refresh, e = edit, d = delete)"),
        )
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

fn draw_confirm_delete_popup(frame: &mut Frame, name: &str) {
    let area = centered_rect(60, 20, frame.area());
    frame.render_widget(Clear, area);
    let text = format!(
        "Delete \"{name}\"?\n\
Removes its profile.toml (settings/history) — not the exe itself.\n\n\
y = confirm, any other key = cancel"
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
                ConfigField::LogMode => format!("{:?}", l.mode),
                ConfigField::LogPath => l.path.clone(),
                ConfigField::LogKeep => l.keep.to_string(),
                ConfigField::LogRecord => format!("{:?}", l.record),
                ConfigField::LogAutoOpen => l.auto_open.to_string(),
                ConfigField::GamedbInterval => g.update_interval_days.to_string(),
                ConfigField::EnvTable => entry_count(&app.cfg.env),
                ConfigField::WineDllOverrideTable => entry_count(&app.cfg.winedlloverride),
            };
            let combined = format!("{:<28} {}", field.label(), value);
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
                IntegrateField::Setup => {
                    "Enter = register iprolaunch as the default .exe/.bat/.cmd/.msi handler"
                        .to_string()
                }
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

fn draw_help(frame: &mut Frame, area: Rect) {
    let text = "\
iprolaunch — TUI help

Global:
  1/2/3/4, Tab/Shift-Tab   switch screen
  q                        quit

Running:
  Enter or k               kill the selected launch
  r                        refresh now (also refreshes automatically)

Library:
  Enter                    launch the selected game
  a                        add a game by typing its exe path
  r                        refresh the list from disk
  e                        edit the selected game's profile overrides
  d                        delete the selected game's profile (confirms first)
  Esc                      cancel while typing a path

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
    - name is what the Library list shows — edit just the base text (its
      \"#N\" is stripped for editing and never shown in the box); saving
      auto-fills the lowest \"#N\" not already used by another profile's
      same base, reusing a gap left by a deleted/renamed one rather than
      always growing past the historical max.

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
  setup                      register iprolaunch as the default .exe/.bat/.cmd/.msi handler
  reapply                    re-point the registration at this binary's
                             current path, without touching the saved
                             backup of what the default was before setup
  uninstall                  remove the registration and restore that backup

env / winedlloverride entry list (global or per-profile):
  a                        add an entry (prompts for name, then value)
  e                        edit the selected entry
  d                        delete the selected entry
  Esc                      back to whichever screen opened it

Config file: ~/.config/iprolaunch/config.toml
Profiles:    ~/.config/iprolaunch/profiles/<slug>/profile.toml
";
    frame.render_widget(
        Paragraph::new(text).block(Block::default().borders(Borders::ALL).title("Help")),
        area,
    );
}

fn draw_status_bar(frame: &mut Frame, area: Rect, app: &App) {
    let text = match &app.status {
        Some(s) => Span::styled(s.clone(), Style::default().fg(Color::Yellow)),
        None => Span::raw("Ready."),
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
    };
    format!(
        "{:?}|{}|{}|{}|{:?}|{mode_part}",
        app.tab,
        app.library_selected,
        app.config_selected,
        app.profile_field_selected,
        app.profile_editor,
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
            idx - row_start > 60,
            "expected last launched to be pushed toward the right edge, gap was {}",
            idx - row_start
        );
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
        let out = rendered(&mut app, 100, 40);
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
        let out = rendered(&mut app, 100, 30);
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
