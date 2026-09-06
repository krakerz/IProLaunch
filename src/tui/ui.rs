use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Tabs};

use super::app::{App, ConfigField, Mode, Tab};

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

pub fn draw(frame: &mut Frame, app: &App) {
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
        Tab::Library => draw_library(frame, chunks[2], app),
        Tab::Config => draw_config(frame, chunks[2], app),
        Tab::Help => draw_help(frame, chunks[2]),
    }

    draw_status_bar(frame, chunks[3], app);

    match &app.mode {
        Mode::TextInput { buffer, .. } => draw_text_input_popup(frame, buffer),
        Mode::ProtonPicker { builds, selected } => {
            draw_proton_picker_popup(frame, builds, *selected)
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
    let items: Vec<ListItem> = app
        .profiles
        .iter()
        .map(|(slug, p)| {
            let last = p.last_launched.as_deref().unwrap_or("never");
            ListItem::new(format!("{}  [{slug}]  last: {last}", p.name))
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Library (Enter = launch, a = add)"),
        )
        .highlight_style(Style::default().bg(Color::DarkGray))
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, area, &mut list_state(app.library_selected));
}

fn draw_config(frame: &mut Frame, area: Rect, app: &App) {
    let d = &app.cfg.defaults;
    let l = &app.cfg.logging;
    let g = &app.cfg.gamedb;

    let items: Vec<ListItem> = ConfigField::ALL
        .iter()
        .map(|field| {
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
            };
            ListItem::new(format!("{:<28} {}", field.label(), value))
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
    frame.render_stateful_widget(list, area, &mut list_state(app.config_selected));
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
  Esc                      cancel while typing a path

Config:
  Enter                    edit (text fields), cycle (mode/record), or
                           toggle (auto_open); opens a picker for proton
  Left/Right               adjust a number field
  Esc                      cancel a text edit without saving
  Changes save to config.toml immediately.

Not editable here — edit profile.toml by hand instead:
  - a profile's env/winedlloverride overrides, or its windows-version/prefix_path override
  - a profile's `title` (used to match the umu-database for a GAMEID)
  - the global [env] and [winedlloverride] tables

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

fn draw_text_input_popup(frame: &mut Frame, buffer: &str) {
    let area = centered_rect(60, 20, frame.area());
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Edit (Enter = save, Esc = cancel)");
    frame.render_widget(Paragraph::new(format!("{buffer}_")).block(block), area);
}

fn draw_proton_picker_popup(
    frame: &mut Frame,
    builds: &[crate::proton::ProtonBuild],
    selected: usize,
) {
    let area = centered_rect(60, 60, frame.area());
    frame.render_widget(Clear, area);

    let mut items = vec![ListItem::new(
        "system  (let umu-run auto-manage UMU-Proton)",
    )];
    items.extend(
        builds
            .iter()
            .map(|b| ListItem::new(format!("{}  [{}]", b.display_name, b.id))),
    );
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Pick a Proton build (Enter, Esc = cancel)"),
        )
        .highlight_style(Style::default().bg(Color::DarkGray))
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, area, &mut list_state(selected));
}

fn list_state(selected: usize) -> ratatui::widgets::ListState {
    let mut state = ratatui::widgets::ListState::default();
    state.select(Some(selected));
    state
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

    fn rendered(app: &App, width: u16, height: u16) -> String {
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
    fn header_shows_the_wordmark_and_version() {
        // Regression check: the header block must be tall enough for the
        // logo *plus* the version line — sized for just the logo once and
        // silently clipped the version off until caught by manual testing.
        let app = test_app();
        let out = rendered(&app, 80, 24);
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
            let out = rendered(&app, 80, 24);
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
            rendered(&app, 20, 6);
        }
    }

    #[test]
    fn empty_running_tab_shows_placeholder_text() {
        let mut app = test_app();
        app.tab = Tab::Running;
        app.running.clear();
        assert!(rendered(&app, 80, 24).contains("Nothing running"));
    }

    #[test]
    fn empty_library_tab_shows_placeholder_text() {
        let mut app = test_app();
        app.tab = Tab::Library;
        app.profiles.clear();
        assert!(rendered(&app, 80, 24).contains("No games yet"));
    }

    #[test]
    fn config_tab_lists_every_field_label() {
        let mut app = test_app();
        app.tab = Tab::Config;
        let out = rendered(&app, 100, 30);
        for field in ConfigField::ALL {
            assert!(
                out.contains(field.label()),
                "missing label: {}",
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
        };
        assert!(rendered(&app, 80, 24).contains("hello-world"));
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
        };
        let out = rendered(&app, 80, 24);
        assert!(out.contains("system"));
        assert!(out.contains("GE-Proton10-34"));
    }
}
