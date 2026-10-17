mod app;
mod config;
mod gamepad;
mod gui;
mod library;
mod profile_editor;
mod running;
mod ui;

use std::io::{self, IsTerminal, Stdout, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use gilrs::{EventType as GilrsEventType, Gilrs};
use ratatui::Terminal;
use ratatui::backend::{Backend, ClearType, CrosstermBackend, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};

use crate::config::Config;
use crate::gamedb;
use app::{App, InputKind, Mode, TextInputPurpose};
use gamepad::GamepadSource;

pub type Term = Terminal<AppBackend>;

pub use gui::parse_size as parse_gamemode_size;

/// The terminal, or the Game Mode window (see `gui`).
pub enum AppBackend {
    Terminal(CrosstermBackend<Stdout>),
    Window(gui::GuiBackend),
}

macro_rules! each_backend {
    ($self:ident, $b:ident => $e:expr) => {
        match $self {
            AppBackend::Terminal($b) => $e,
            AppBackend::Window($b) => $e,
        }
    };
}

impl Backend for AppBackend {
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        each_backend!(self, b => b.draw(content))
    }
    fn hide_cursor(&mut self) -> io::Result<()> {
        each_backend!(self, b => b.hide_cursor())
    }
    fn show_cursor(&mut self) -> io::Result<()> {
        each_backend!(self, b => b.show_cursor())
    }
    fn get_cursor_position(&mut self) -> io::Result<Position> {
        each_backend!(self, b => b.get_cursor_position())
    }
    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        each_backend!(self, b => b.set_cursor_position(position))
    }
    fn clear(&mut self) -> io::Result<()> {
        each_backend!(self, b => b.clear())
    }
    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        each_backend!(self, b => b.clear_region(clear_type))
    }
    fn size(&self) -> io::Result<Size> {
        each_backend!(self, b => b.size())
    }
    fn window_size(&mut self) -> io::Result<WindowSize> {
        each_backend!(self, b => b.window_size())
    }
    fn flush(&mut self) -> io::Result<()> {
        each_backend!(self, b => Backend::flush(b))
    }
}

/// `iprolaunch --gamemode [WxH]`: the TUI in its own window for Steam Game
/// Mode, no terminal needed.
pub fn run_gamemode(cfg: Config, size: Option<(u32, u32)>) -> Result<()> {
    gui::run(size, move || {
        let mut app = App::new(cfg);
        let mut terminal = Terminal::new(AppBackend::Window(gui::GuiBackend::new()))?;
        // Sizes the backend's buffer: if the window published its real size
        // before `Terminal::new`, autoresize never fires and every cell
        // would be dropped into the initial 0x0 buffer (a blank window).
        terminal.clear()?;
        let mut gamepad = GamepadSource::new();
        event_loop(&mut terminal, &mut app, &mut gamepad)
    })
}

/// Next input event from the terminal or the Game Mode window.
fn next_event(timeout: Duration) -> io::Result<Option<Event>> {
    if let Some(link) = gui::link() {
        return Ok(link.next_event(timeout));
    }
    if event::poll(timeout)? {
        Ok(Some(event::read()?))
    } else {
        Ok(None)
    }
}

pub fn run(cfg: Config) -> Result<()> {
    // Bare `iprolaunch` with no controlling terminal (confirmed for real:
    // Steam Game Mode launches a non-Steam-game shortcut this way) makes
    // `enable_raw_mode()` below fail with a bare, cryptic OS error
    // ("No such device or address (os error 6)") that's invisible there
    // anyway (no console shown) — this at least prints something
    // actionable if stderr *is* visible (e.g. run from a script), and the
    // exit is the same either way. Checked up front rather than after
    // already touching the real terminal, so there's nothing to undo.
    if !io::stdout().is_terminal() {
        bail!(
            "no terminal available to run the TUI in (this happens when launched without a \
             console, e.g. a Steam Game Mode/gamescope shortcut) — use `iprolaunch -gm` \
             for the TUI in its own window, or `iprolaunch <name-or-slug>` for one game"
        );
    }

    let mut app = App::new(cfg);

    // Restore the terminal even on panic, so a bug doesn't leave the user's
    // shell stuck in raw mode / the alternate screen.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));

    enable_raw_mode()?;
    // From here on every exit path (including a failed setup step) restores.
    let _restore = RestoreOnDrop;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    let backend = AppBackend::Terminal(CrosstermBackend::new(stdout));
    let mut terminal = Terminal::new(backend)?;

    let mut gamepad = GamepadSource::new();
    event_loop(&mut terminal, &mut app, &mut gamepad)
}

fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(
        io::stdout(),
        DisableBracketedPaste,
        LeaveAlternateScreen,
        crossterm::cursor::Show
    );
}

struct RestoreOnDrop;

impl Drop for RestoreOnDrop {
    fn drop(&mut self) {
        restore_terminal();
    }
}

/// Bumped by every `suspend`, so the event loop can tell an input it just
/// handled ran something external (a launch) and stop applying the rest of
/// an input batch collected before it.
static SUSPENDS: AtomicU64 = AtomicU64::new(0);

/// How often the Running tab rescans `/proc` while idle — a full environ
/// scan per entry is far too heavy for every 30ms tick (battery on a Deck).
const RUNNING_REFRESH_INTERVAL: Duration = Duration::from_secs(1);

fn event_loop(terminal: &mut Term, app: &mut App, gamepad: &mut GamepadSource) -> Result<()> {
    let mut last_tab = app.tab;
    let mut last_running_refresh = Instant::now();
    loop {
        // systemctl is too slow to query every frame; refresh on entering the tab.
        if app.tab == app::Tab::Sunshine && last_tab != app::Tab::Sunshine {
            app.sunshine_service_status = crate::vdesktop::service_status();
        }
        last_tab = app.tab;
        terminal.draw(|f| ui::draw(f, app))?;

        let mut handled = false;
        // 30ms matches gilrs's own `Repeat` filter interval (`gamepad::poll`'s
        // held-D-pad auto-repeat) — that's the fastest a new repeat step can
        // actually appear, so polling faster just burns cycles finding
        // nothing new, and polling slower starts batching two repeat steps
        // into one redraw (a visible double-row jump instead of a smooth
        // one-row-per-frame scroll).
        if let Some(event) = next_event(Duration::from_millis(30))? {
            match event {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    app.input_kind = InputKind::Keyboard;
                    let ctrl_v = key.modifiers.contains(KeyModifiers::CONTROL)
                        && matches!(key.code, KeyCode::Char('v' | 'V'));
                    if ctrl_v && is_text_entry(app) {
                        match crate::quick_launch_cmd::read_clipboard() {
                            Ok(text) => paste(app, &text, terminal),
                            Err(err) => app.status = Some(format!("Couldn't paste: {err:#}")),
                        }
                    } else {
                        on_key(app, key.code, terminal);
                    }
                    handled = true;
                }
                Event::Paste(text) if is_text_entry(app) => {
                    paste(app, &text, terminal);
                    handled = true;
                }
                _ => {}
            }
        }
        // Checked every tick regardless of whether the keyboard poll above
        // timed out or returned an unrelated event (resize, etc.) — gilrs
        // queues its own events independently of crossterm's.
        let suspends = SUSPENDS.load(Ordering::SeqCst);
        for input in gamepad.poll() {
            if SUSPENDS.load(Ordering::SeqCst) != suspends {
                break;
            }
            app.input_kind = InputKind::Gamepad;
            match input {
                // Buttons map to letter shortcuts (Select = `f`, Start = `q`,
                // ...); in a text field they'd type those letters instead.
                gamepad::PadInput::Key(KeyCode::Char(_)) if is_text_entry(app) => {}
                gamepad::PadInput::Key(code) => on_key(app, code, terminal),
                gamepad::PadInput::Zoom(step) => gui::zoom(step),
            }
            handled = true;
        }
        if !handled
            && matches!(app.tab, app::Tab::Running)
            && last_running_refresh.elapsed() >= RUNNING_REFRESH_INTERVAL
        {
            app.refresh_running();
            last_running_refresh = Instant::now();
        }

        if app.should_quit {
            return Ok(());
        }
    }
}

/// Whether keystrokes are currently going into a free-text field.
fn is_text_entry(app: &App) -> bool {
    matches!(
        app.mode,
        Mode::TextInput { .. } | Mode::MapEntryInput { .. } | Mode::GameIdPicker { .. }
    ) || (matches!(app.mode, Mode::Normal)
        && ((app.tab == app::Tab::Running && app.running_filter_editing)
            || (app.tab == app::Tab::Library && app.library_filter_editing)))
}

/// Types pasted text into the active field. Line breaks become spaces since
/// every field is single-line (Enter would otherwise submit mid-paste).
fn paste(app: &mut App, text: &str, terminal: &mut Term) {
    let text = text.trim_end_matches(['\r', '\n']);
    for c in text.chars() {
        let c = if c == '\n' || c == '\r' || c == '\t' {
            ' '
        } else {
            c
        };
        if !c.is_control() {
            on_key(app, KeyCode::Char(c), terminal);
        }
    }
}

fn on_key(app: &mut App, code: KeyCode, terminal: &mut Term) {
    match &app.mode {
        Mode::TextInput { .. } => return handle_text_input(app, code, terminal),
        Mode::ProtonPicker { .. } => return handle_proton_picker(app, code),
        Mode::GameIdPicker { .. } => return handle_gameid_picker(app, code),
        Mode::MapEditor { .. } => return config::map_editor_key(app, code),
        Mode::MapEntryInput { .. } => return config::map_entry_input_key(app, code),
        Mode::ConfirmDeleteProfile { .. } => return library::confirm_delete_key(app, code),
        Mode::ConfirmRenameSlug { .. } => {
            return profile_editor::confirm_rename_slug_key(app, code);
        }
        Mode::Help => return help_popup_key(app, code),
        Mode::ConfirmWinetricks { .. } => {
            return library::confirm_winetricks_key(app, code, terminal);
        }
        Mode::ShareTo { .. } => return library::share_to_key(app, code, terminal),
        Mode::ConfirmUpdate { .. } => return config::confirm_update_key(app, code, terminal),
        Mode::Normal => {}
    }

    // While actively typing a Running/Library quick-search (not yet
    // locked by Enter), every key goes straight to it — including digits/
    // q/?/Tab, which the global shortcuts below would otherwise swallow
    // before the filter ever saw them (e.g. searching a game named
    // "Dark Souls 3"). Once locked (Enter pressed, or no filter at all),
    // these fall through to the normal global-key handling below, same as
    // ever.
    if app.tab == app::Tab::Running && app.running_filter_editing {
        return running::on_key(app, code, terminal);
    }
    if app.tab == app::Tab::Library && app.library_filter_editing {
        return library::on_key(app, code, terminal);
    }

    match code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char('1') => {
            app.clear_filters();
            app.tab = app::Tab::Running;
        }
        KeyCode::Char('2') => {
            app.clear_filters();
            app.tab = app::Tab::Library;
        }
        KeyCode::Char('3') => {
            app.clear_filters();
            app.tab = app::Tab::Config;
        }
        KeyCode::Char('4') => {
            app.clear_filters();
            app.tab = app::Tab::Sunshine;
        }
        KeyCode::Char('5') => {
            app.clear_filters();
            app.tab = app::Tab::Help;
        }
        KeyCode::Char('?') => app.mode = Mode::Help,
        KeyCode::Tab => app.next_tab(),
        KeyCode::BackTab => app.prev_tab(),
        _ => match app.tab {
            app::Tab::Running => running::on_key(app, code, terminal),
            app::Tab::Library => library::on_key(app, code, terminal),
            app::Tab::Config | app::Tab::Sunshine => config::on_key(app, code, terminal),
            app::Tab::Help => help_scroll_key(app, code),
        },
    }
}

/// Esc closes the `?` popup (back to `Mode::Normal`); everything else is
/// just scrolling, shared with the Help tab itself.
fn help_popup_key(app: &mut App, code: KeyCode) {
    if code == KeyCode::Esc {
        app.mode = Mode::Normal;
        return;
    }
    help_scroll_key(app, code);
}

/// Up/Down by one line, PageUp/PageDown by a full page, Home/End to jump to
/// either end — used both by the Help tab (`Mode::Normal`) and the `?`
/// popup (`Mode::Help`). Soft-clamped against the *total* line count here;
/// `ui::draw_help`/`draw_help_popup` additionally clamp against the actual
/// visible height at render time, so an exact bound here isn't needed.
fn help_scroll_key(app: &mut App, code: KeyCode) {
    let total = ui::help_text_line_count();
    match code {
        KeyCode::Up => app.help_scroll = app.help_scroll.saturating_sub(1),
        KeyCode::Down => app.help_scroll = (app.help_scroll + 1).min(total),
        KeyCode::PageUp => app.help_scroll = app.help_scroll.saturating_sub(10),
        KeyCode::PageDown => app.help_scroll = (app.help_scroll + 10).min(total),
        KeyCode::Home => app.help_scroll = 0,
        KeyCode::End => app.help_scroll = total,
        _ => {}
    }
}

/// Cursor movement, insert, and delete for `Mode::TextInput` (and the GAMEID
/// picker's query) — everything
/// that never needs `terminal`, kept separate from `handle_text_input` so
/// it's unit-testable without needing a real `Term`. Operates on
/// `buffer.chars()` (char index, not byte offset) throughout. Returns
/// `true` if `code` was handled here (so the caller shouldn't fall through
/// to Enter/Esc handling).
fn edit_text_buffer(app: &mut App, code: KeyCode) -> bool {
    match code {
        KeyCode::Left => {
            if let Some((_, cursor)) = text_fields(&mut app.mode) {
                *cursor = cursor.saturating_sub(1);
            }
            true
        }
        KeyCode::Right => {
            if let Some((buffer, cursor)) = text_fields(&mut app.mode) {
                *cursor = (*cursor + 1).min(buffer.chars().count());
            }
            true
        }
        KeyCode::Backspace => {
            if let Some((buffer, cursor)) = text_fields(&mut app.mode)
                && *cursor > 0
            {
                let mut chars: Vec<char> = buffer.chars().collect();
                *cursor -= 1;
                chars.remove(*cursor);
                *buffer = chars.into_iter().collect();
            }
            true
        }
        KeyCode::Char(c) => {
            if let Some((buffer, cursor)) = text_fields(&mut app.mode) {
                let mut chars: Vec<char> = buffer.chars().collect();
                chars.insert((*cursor).min(chars.len()), c);
                *buffer = chars.into_iter().collect();
                *cursor += 1;
            }
            true
        }
        _ => false,
    }
}

/// The editable buffer and char cursor of whichever mode owns one, so the
/// GAMEID picker's query edits exactly like a `TextInput`.
fn text_fields(mode: &mut Mode) -> Option<(&mut String, &mut usize)> {
    match mode {
        Mode::TextInput { buffer, cursor, .. } => Some((buffer, cursor)),
        Mode::GameIdPicker { query, cursor, .. } => Some((query, cursor)),
        _ => None,
    }
}

fn handle_text_input(app: &mut App, code: KeyCode, terminal: &mut Term) {
    if code == KeyCode::Esc {
        app.mode = Mode::Normal;
        return;
    }
    if edit_text_buffer(app, code) {
        return;
    }
    if code != KeyCode::Enter {
        return;
    }

    let Mode::TextInput {
        purpose, buffer, ..
    } = std::mem::replace(&mut app.mode, Mode::Normal)
    else {
        return;
    };
    match purpose {
        TextInputPurpose::AddLibraryPath => {
            let label = buffer.clone();
            library::launch_path(app, terminal, &buffer, &label, None);
        }
        TextInputPurpose::ConfigField(field) => config::apply_text_field(app, field, buffer),
        TextInputPurpose::ProfileTitle(slug) => library::apply_profile_title(app, &slug, buffer),
        TextInputPurpose::ProfileField(slug, field) => {
            profile_editor::apply_text_field(app, &slug, field, buffer)
        }
        TextInputPurpose::SunshineUsername { slug, name } => {
            library::submit_sunshine_username(app, &slug, &name, buffer)
        }
        TextInputPurpose::SunshinePassword {
            slug,
            name,
            username,
        } => library::submit_sunshine_password(app, &slug, &name, username, buffer),
    }
}

/// `builds.len()` plus 1 for the always-present "system" entry, plus one
/// more for "inherit" when picking a profile override (see
/// `ui::draw_proton_picker_popup`, which renders the same extra row).
fn proton_picker_len(
    builds: &[crate::proton::ProtonBuild],
    target: &app::ProtonPickerTarget,
) -> usize {
    builds.len()
        + if matches!(target, app::ProtonPickerTarget::Profile(_)) {
            2
        } else {
            1
        }
}

fn handle_proton_picker(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Up => {
            if let Mode::ProtonPicker {
                builds,
                selected,
                target,
            } = &mut app.mode
            {
                let len = proton_picker_len(builds, target);
                *selected = app::move_selection(*selected, len, -1);
            }
        }
        KeyCode::Down => {
            if let Mode::ProtonPicker {
                builds,
                selected,
                target,
            } = &mut app.mode
            {
                let len = proton_picker_len(builds, target);
                *selected = app::move_selection(*selected, len, 1);
            }
        }
        KeyCode::Enter => {
            let Mode::ProtonPicker {
                builds,
                selected,
                target,
            } = std::mem::replace(&mut app.mode, Mode::Normal)
            else {
                return;
            };
            config::apply_proton_choice(app, &builds, selected, &target);
        }
        _ => {}
    }
}

fn handle_gameid_picker(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Up | KeyCode::Down => {
            if let Mode::GameIdPicker {
                results, selected, ..
            } = &mut app.mode
            {
                let delta = if code == KeyCode::Up { -1 } else { 1 };
                *selected = app::move_selection(*selected, app::gameid_picker_len(results), delta);
            }
        }
        KeyCode::Enter => {
            let Mode::GameIdPicker {
                slug,
                results,
                selected,
                ..
            } = std::mem::replace(&mut app.mode, Mode::Normal)
            else {
                return;
            };
            profile_editor::apply_gameid_choice(app, &slug, selected, &results);
        }
        _ => {
            let before = match &app.mode {
                Mode::GameIdPicker { query, .. } => query.clone(),
                _ => return,
            };
            edit_text_buffer(app, code);
            if let Mode::GameIdPicker {
                query,
                results,
                selected,
                ..
            } = &mut app.mode
                && *query != before
            {
                *results = gamedb::search(query);
                *selected = app::gameid_row_after_search(results);
            }
        }
    }
}

/// Leaves the alternate screen and raw mode so a subprocess's own stdout
/// (e.g. `umu-run`'s launch messages) prints normally instead of into a
/// buffer the user can't see.
///
/// On failure the TUI is put back as it was, since callers bail out
/// without calling `resume`.
pub fn suspend(terminal: &mut Term) -> Result<()> {
    SUSPENDS.fetch_add(1, Ordering::SeqCst);
    if let Some(link) = gui::link() {
        link.suspend();
        return Ok(());
    }
    let result = (|| -> Result<()> {
        disable_raw_mode()?;
        execute!(io::stdout(), DisableBracketedPaste, LeaveAlternateScreen)?;
        terminal.show_cursor()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = resume(terminal);
    }
    result
}

pub fn resume(terminal: &mut Term) -> Result<()> {
    gamepad::discard_pending();
    if let Some(link) = gui::link() {
        link.resume();
        terminal.clear()?;
        return Ok(());
    }
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
    terminal.hide_cursor()?;
    terminal.clear()?;
    Ok(())
}

/// Blocks until the user acknowledges a suspended external process's result
/// (a launch, winetricks, a self-update, desktop integration) — Enter on a
/// keyboard, or B on a gamepad, so this doesn't strand someone playing
/// entirely on a controller (e.g. Steam Deck Game Mode) with no easy way to
/// dismiss it short of summoning the on-screen keyboard.
///
/// Deliberately B (`gamepad::translate`'s `Esc`), not "any button" (a real
/// reported bug with that first version): this uses its own freshly opened
/// `Gilrs` handle, a separate reader from the main loop's own persistent
/// `GamepadSource`, and evdev delivers a copy of the same physical
/// button-press to every open reader — so whatever button dismisses this
/// also lands, buffered, in `GamepadSource`'s own queue and gets replayed as
/// a real keypress the moment the main loop resumes polling it. A on
/// Library's list is Enter/confirm, so pressing A here to return promptly
/// re-launched whatever was selected. Esc is a safe no-op to replay (there's
/// nothing open yet to cancel), so B sidesteps the whole class of bug
/// without needing to plumb `GamepadSource` down through every caller just
/// to drain the *same* reader instead of a second one.
///
/// Briefly re-enables raw mode of its own accord so a keypress doesn't need
/// a trailing newline to register, and disables it again before returning —
/// paired, so the caller's own subsequent `resume` (which does its own
/// `enable_raw_mode`) sees the same disabled-raw-mode state it always has.
/// Falls back to the old plain blocking line read, keyboard-only, if raw
/// mode can't be re-enabled here for some reason, rather than hanging.
pub fn wait_for_return() {
    print!("\nPress Enter (or B on a gamepad) to return to iprolaunch. ");
    io::stdout().flush().ok();

    let window = gui::link().is_some();
    if !window && enable_raw_mode().is_err() {
        let mut discard = String::new();
        io::stdin().read_line(&mut discard).ok();
        println!();
        return;
    }

    let mut gilrs = Gilrs::new().ok();
    loop {
        if let Ok(Some(Event::Key(key))) = next_event(Duration::from_millis(50))
            && key.kind == KeyEventKind::Press
            && key.code == KeyCode::Enter
        {
            break;
        }
        let pressed_b = gilrs.as_mut().is_some_and(|g| {
            let mut pressed = false;
            while let Some(gilrs::Event { event, .. }) = g.next_event() {
                if let GilrsEventType::ButtonPressed(button, _) = event
                    && gamepad::translate(button) == Some(KeyCode::Esc)
                {
                    pressed = true;
                }
            }
            pressed
        });
        if pressed_b {
            break;
        }
    }

    if !window {
        disable_raw_mode().ok();
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;
    use app::TextInputPurpose;

    #[test]
    fn paste_only_targets_free_text_fields() {
        assert!(is_text_entry(&text_input_app("", 0)));

        let mut app = App::new(crate::config::Config::default());
        assert!(!is_text_entry(&app));
        app.tab = app::Tab::Library;
        app.library_filter_editing = true;
        assert!(is_text_entry(&app));
        app.tab = app::Tab::Running;
        assert!(!is_text_entry(&app));
    }

    fn text_input_app(buffer: &str, cursor: usize) -> App {
        let mut app = App::new(crate::config::Config::default());
        app.mode = Mode::TextInput {
            purpose: TextInputPurpose::AddLibraryPath,
            buffer: buffer.to_string(),
            cursor,
        };
        app
    }

    fn gameid_picker_app(query: &str, results: Vec<gamedb::GameDbEntry>) -> App {
        let mut app = App::new(crate::config::Config::default());
        app.mode = Mode::GameIdPicker {
            slug: "g".to_string(),
            query: query.to_string(),
            cursor: query.chars().count(),
            results,
            selected: 0,
        };
        app
    }

    #[test]
    fn gameid_picker_counts_as_text_entry_and_edits_its_query() {
        let mut app = gameid_picker_app("ab", Vec::new());
        assert!(is_text_entry(&app));
        handle_gameid_picker(&mut app, KeyCode::Backspace);
        let Mode::GameIdPicker { query, cursor, .. } = &app.mode else {
            panic!("expected GameIdPicker")
        };
        assert_eq!((query.as_str(), *cursor), ("a", 1));
    }

    #[test]
    fn gameid_picker_up_down_stay_within_the_fixed_rows_plus_results() {
        let entry = gamedb::GameDbEntry {
            title: "A".to_string(),
            umu_id: "umu-1".to_string(),
            stores: Vec::new(),
        };
        let mut app = gameid_picker_app("", vec![entry]);
        for _ in 0..5 {
            handle_gameid_picker(&mut app, KeyCode::Down);
        }
        assert!(matches!(app.mode, Mode::GameIdPicker { selected: 2, .. }));
        handle_gameid_picker(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Normal));
    }

    fn cursor_and_buffer(app: &App) -> (usize, &str) {
        let Mode::TextInput { buffer, cursor, .. } = &app.mode else {
            panic!("expected TextInput")
        };
        (*cursor, buffer)
    }

    #[test]
    fn left_right_move_the_cursor_and_clamp_at_both_ends() {
        let mut app = text_input_app("abc", 1);
        edit_text_buffer(&mut app, KeyCode::Left);
        assert_eq!(cursor_and_buffer(&app).0, 0);
        edit_text_buffer(&mut app, KeyCode::Left); // already at 0
        assert_eq!(cursor_and_buffer(&app).0, 0);

        edit_text_buffer(&mut app, KeyCode::Right);
        edit_text_buffer(&mut app, KeyCode::Right);
        edit_text_buffer(&mut app, KeyCode::Right);
        assert_eq!(cursor_and_buffer(&app).0, 3); // clamped to buffer length
        edit_text_buffer(&mut app, KeyCode::Right);
        assert_eq!(cursor_and_buffer(&app).0, 3);
    }

    #[test]
    fn typing_inserts_at_the_cursor_not_just_at_the_end() {
        // The scenario the user asked for: fix one segment of a path
        // without retyping the whole thing.
        let mut app = text_input_app("a\\c.exe", 2); // cursor right after "a\"
        edit_text_buffer(&mut app, KeyCode::Char('b'));
        let (cursor, buffer) = cursor_and_buffer(&app);
        assert_eq!(buffer, "a\\bc.exe");
        assert_eq!(cursor, 3);
    }

    #[test]
    fn backspace_deletes_before_the_cursor_and_moves_it_back() {
        let mut app = text_input_app("abc", 2); // cursor between 'b' and 'c'
        edit_text_buffer(&mut app, KeyCode::Backspace);
        let (cursor, buffer) = cursor_and_buffer(&app);
        assert_eq!(buffer, "ac");
        assert_eq!(cursor, 1);
    }

    #[test]
    fn backspace_at_the_start_does_nothing() {
        let mut app = text_input_app("abc", 0);
        edit_text_buffer(&mut app, KeyCode::Backspace);
        let (cursor, buffer) = cursor_and_buffer(&app);
        assert_eq!(buffer, "abc");
        assert_eq!(cursor, 0);
    }

    #[test]
    fn esc_closes_the_help_popup() {
        let mut app = App::new(crate::config::Config::default());
        app.mode = Mode::Help;
        help_popup_key(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn down_and_up_move_help_scroll_by_one_line() {
        let mut app = App::new(crate::config::Config::default());
        help_scroll_key(&mut app, KeyCode::Down);
        help_scroll_key(&mut app, KeyCode::Down);
        assert_eq!(app.help_scroll, 2);
        help_scroll_key(&mut app, KeyCode::Up);
        assert_eq!(app.help_scroll, 1);
    }

    #[test]
    fn up_at_the_top_does_not_go_negative() {
        let mut app = App::new(crate::config::Config::default());
        help_scroll_key(&mut app, KeyCode::Up);
        assert_eq!(app.help_scroll, 0);
    }

    #[test]
    fn page_down_and_page_up_move_by_ten_lines() {
        let mut app = App::new(crate::config::Config::default());
        help_scroll_key(&mut app, KeyCode::PageDown);
        assert_eq!(app.help_scroll, 10);
        help_scroll_key(&mut app, KeyCode::PageUp);
        assert_eq!(app.help_scroll, 0);
    }

    #[test]
    fn end_jumps_to_the_bottom_and_home_back_to_the_top() {
        let mut app = App::new(crate::config::Config::default());
        help_scroll_key(&mut app, KeyCode::End);
        assert_eq!(app.help_scroll, ui::help_text_line_count());
        help_scroll_key(&mut app, KeyCode::Home);
        assert_eq!(app.help_scroll, 0);
    }

    #[test]
    fn down_does_not_scroll_past_the_total_line_count() {
        let mut app = App::new(crate::config::Config::default());
        app.help_scroll = ui::help_text_line_count();
        help_scroll_key(&mut app, KeyCode::Down);
        assert_eq!(app.help_scroll, ui::help_text_line_count());
    }
}
