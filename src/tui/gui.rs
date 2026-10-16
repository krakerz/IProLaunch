//! Game Mode window (`iprolaunch --gamemode`): the same TUI, drawn in our own
//! window instead of a terminal, so Steam Game Mode doesn't need a terminal
//! emulator (which inherits Steam's `LD_PRELOAD` and fights gamescope over
//! focus and font size).
//!
//! The TUI's event loop runs unchanged on a worker thread against
//! [`GuiBackend`]; the window (eframe, main thread) paints the cell grid it
//! publishes and sends key presses back as crossterm events. Blocking
//! actions (a game launch, winetricks) only block the worker, so the window
//! keeps drawing. While the TUI is suspended for one, the window shows our
//! captured stdout/stderr (and the child's) in place of the grid.

use std::io::{self, Read, Write};
use std::os::fd::FromRawFd;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use eframe::egui::{self, Color32, FontId, Pos2, Rect, Vec2};
use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::{Position, Size};
use ratatui::style::{Color, Modifier};
use unicode_width::UnicodeWidthStr;

const LOG_LINES: usize = 3000;
const DEFAULT_BG: Color32 = Color32::from_rgb(18, 18, 24);
const DEFAULT_FG: Color32 = Color32::from_rgb(220, 220, 220);

/// Shared between the window, the TUI worker and the output reader.
pub struct Link {
    screen: Mutex<Buffer>,
    size: Mutex<(u16, u16)>,
    log: Mutex<Log>,
    suspended: AtomicBool,
    quit: AtomicBool,
    /// Zoom steps requested by the gamepad (worker side), applied by the
    /// window on its next frame.
    zoom_steps: AtomicI32,
    events: Mutex<Receiver<Event>>,
    ctx: egui::Context,
    /// Why the TUI worker stopped, if it failed or panicked — reported on the
    /// real stderr once the window has closed.
    failure: Mutex<Option<String>>,
}

static LINK: OnceLock<Arc<Link>> = OnceLock::new();

/// Our stdout/stderr before `capture_output` redirected them into the pipe.
static ORIGINAL_FDS: OnceLock<(i32, i32)> = OnceLock::new();

/// The Game Mode window's link, when running in it.
pub fn link() -> Option<&'static Link> {
    LINK.get().map(|l| &**l)
}

impl Link {
    pub fn next_event(&self, timeout: Duration) -> Option<Event> {
        match self.events.lock().unwrap().recv_timeout(timeout) {
            Ok(event) => Some(event),
            Err(RecvTimeoutError::Timeout) => None,
            // Window gone: the process is exiting anyway.
            Err(RecvTimeoutError::Disconnected) => {
                std::thread::sleep(timeout);
                None
            }
        }
    }

    /// Shows captured output instead of the TUI, starting from a clean log.
    pub fn suspend(&self) {
        self.log.lock().unwrap().clear();
        self.suspended.store(true, Ordering::SeqCst);
        self.ctx.request_repaint();
    }

    /// Back to the TUI. Drops keys pressed meanwhile so they don't replay.
    pub fn resume(&self) {
        io::stdout().flush().ok();
        self.suspended.store(false, Ordering::SeqCst);
        while self.events.lock().unwrap().try_recv().is_ok() {}
        self.ctx.request_repaint();
    }
}

#[derive(Default)]
struct Log {
    lines: Vec<String>,
    current: String,
    /// Inside an escape sequence: 1 = after ESC, 2 = CSI, 3 = OSC.
    escape: u8,
    /// A `\r` was seen: clear the line on the next character unless it's the
    /// `\n` of a CRLF (Wine console output), which would otherwise blank it.
    pending_cr: bool,
}

impl Log {
    fn clear(&mut self) {
        self.lines.clear();
        self.current.clear();
    }

    fn push_str(&mut self, text: &str) {
        for c in text.chars() {
            match (self.escape, c) {
                (0, '\x1b') => self.escape = 1,
                (1, '[') => self.escape = 2,
                (1, ']') => self.escape = 3,
                (1, _) => self.escape = 0,
                (2, '\x40'..='\x7e') => self.escape = 0,
                (2, _) => {}
                (3, '\x07') => self.escape = 0,
                (3, '\x1b') => self.escape = 1,
                (3, _) => {}
                (_, '\n') => {
                    self.pending_cr = false;
                    self.lines.push(std::mem::take(&mut self.current));
                }
                // Progress bars redraw their line with a bare \r.
                (_, '\r') => self.pending_cr = true,
                (_, c) if c.is_control() && c != '\t' => {}
                (_, c) => {
                    if std::mem::take(&mut self.pending_cr) {
                        self.current.clear();
                    }
                    if c == '\t' {
                        self.current.push_str("    ");
                    } else {
                        self.current.push(c);
                    }
                }
            }
        }
        if self.lines.len() > LOG_LINES {
            self.lines.drain(..self.lines.len() - LOG_LINES);
        }
    }

    /// The last `rows` screen rows, long lines wrapped at `cols` columns so
    /// an error message isn't cut off at the window edge.
    fn tail(&self, rows: usize, cols: usize) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let all = self.lines.iter().chain(std::iter::once(&self.current));
        for line in all.rev() {
            if out.len() >= rows {
                break;
            }
            for row in wrap(line, cols).into_iter().rev() {
                out.push(row);
            }
        }
        out.truncate(rows);
        out.reverse();
        out
    }
}

/// Splits `line` into rows of at most `cols` display columns.
fn wrap(line: &str, cols: usize) -> Vec<String> {
    let cols = cols.max(2);
    let mut rows = vec![String::new()];
    let mut width = 0;
    for c in line.chars() {
        let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if width + w > cols {
            rows.push(String::new());
            width = 0;
        }
        rows.last_mut().expect("never empty").push(c);
        width += w;
    }
    rows
}

/// Where a trailing, still-incomplete UTF-8 sequence starts in `bytes` (so
/// a CJK character split across two pipe reads isn't decoded as U+FFFD), or
/// `bytes.len()` if it ends on a character boundary.
fn complete_utf8_len(bytes: &[u8]) -> usize {
    let len = bytes.len();
    for back in 1..=len.min(3) {
        let b = bytes[len - back];
        if b & 0xC0 == 0x80 {
            continue; // continuation byte: keep looking for the lead byte
        }
        let needed = match b {
            0xF0.. => 4,
            0xE0.. => 3,
            0xC0.. => 2,
            _ => 1,
        };
        return if back < needed { len - back } else { len };
    }
    len
}

/// ratatui backend that publishes each finished frame to the window.
pub struct GuiBackend {
    buf: Buffer,
    cursor: Position,
}

impl GuiBackend {
    pub fn new() -> Self {
        Self {
            buf: Buffer::empty(ratatui::layout::Rect::new(0, 0, 0, 0)),
            cursor: Position::default(),
        }
    }

    fn link() -> &'static Link {
        link().expect("GuiBackend used outside the Game Mode window")
    }
}

impl Backend for GuiBackend {
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let area = self.buf.area;
        for (x, y, cell) in content {
            if x < area.width && y < area.height {
                self.buf[(x, y)] = cell.clone();
            }
        }
        Ok(())
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        Ok(self.cursor)
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.cursor = position.into();
        Ok(())
    }

    /// Terminal::resize calls this before redrawing everything, so it's
    /// where the buffer picks up the window's new size.
    fn clear(&mut self) -> io::Result<()> {
        let size = self.size()?;
        self.buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, size.width, size.height));
        Ok(())
    }

    fn clear_region(&mut self, _clear_type: ClearType) -> io::Result<()> {
        self.clear()
    }

    fn size(&self) -> io::Result<Size> {
        let (width, height) = *Self::link().size.lock().unwrap();
        Ok(Size { width, height })
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        let size = self.size()?;
        Ok(WindowSize {
            columns_rows: size,
            pixels: size,
        })
    }

    /// Only repaints on a real change — ratatui flushes every 30ms tick, and
    /// an unconditional repaint kept the window redrawing while idle.
    fn flush(&mut self) -> io::Result<()> {
        let link = Self::link();
        let mut screen = link.screen.lock().unwrap();
        if *screen != self.buf {
            screen.clone_from(&self.buf);
            link.ctx.request_repaint();
        }
        Ok(())
    }
}

/// Whether the TUI is running in the Game Mode window.
pub fn active() -> bool {
    link().is_some()
}

/// Gamepad zoom (View + LT/RT); does nothing in a terminal.
pub fn zoom(step: i8) {
    if let Some(link) = link() {
        link.zoom_steps.fetch_add(step.into(), Ordering::SeqCst);
        link.ctx.request_repaint();
    }
}

/// Parses `--gamemode`'s optional `WIDTHxHEIGHT`.
pub fn parse_size(s: &str) -> Result<(u32, u32)> {
    let (w, h) = s
        .split_once(['x', 'X'])
        .context("expected WIDTHxHEIGHT, e.g. 1920x1200")?;
    let w: u32 = w.trim().parse().context("bad width")?;
    let h: u32 = h.trim().parse().context("bad height")?;
    if w < 320 || h < 240 {
        bail!("window size {w}x{h} is too small (minimum 320x240)");
    }
    Ok((w, h))
}

/// Whether we're inside Steam Game Mode's gamescope session.
fn in_gamescope() -> bool {
    std::env::var_os("GAMESCOPE_WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
        || std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|v| v.eq_ignore_ascii_case("gamescope"))
        || std::env::var_os("SteamGamepadUI").is_some()
}

/// Runs the TUI in its own window. `size` is the window's inner size;
/// without one it opens fullscreen.
pub fn run(
    size: Option<(u32, u32)>,
    tui: impl FnOnce() -> Result<()> + Send + 'static,
) -> Result<()> {
    if in_gamescope() {
        // gamescope's apps are X11 clients; its Wayland socket isn't for us.
        // SAFETY: called before any other thread is started.
        unsafe { std::env::remove_var("WAYLAND_DISPLAY") };
    }

    let (tx, rx) = mpsc::channel();
    let ctx = egui::Context::default();
    let link = Arc::new(Link {
        screen: Mutex::new(Buffer::empty(ratatui::layout::Rect::new(0, 0, 80, 24))),
        size: Mutex::new((80, 24)),
        log: Mutex::new(Log::default()),
        suspended: AtomicBool::new(false),
        quit: AtomicBool::new(false),
        zoom_steps: AtomicI32::new(0),
        events: Mutex::new(rx),
        ctx: ctx.clone(),
        failure: Mutex::new(None),
    });
    LINK.set(link.clone()).ok();
    capture_output(link.clone())?;

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("IProLaunch")
        .with_app_id("iprolaunch");
    viewport = match size {
        Some((w, h)) => viewport.with_inner_size([w as f32, h as f32]),
        None => viewport.with_fullscreen(true),
    };
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let worker_link = link.clone();
    let outer_link = link.clone();
    let mut tui = Some(tui);
    let result = eframe::run_native_ext(
        "iprolaunch",
        options,
        Some(ctx),
        Box::new(move |cc| {
            cc.egui_ctx.set_fonts(fonts());
            // Ctrl +/-/0 are handled in `forward_input`; egui's own keyboard
            // zoom would apply on top of that.
            cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
            let tui = tui.take().expect("app created once");
            std::thread::spawn(move || {
                // Caught so a panic still closes the window instead of
                // leaving it frozen on the last frame.
                let failure = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(tui)) {
                    Ok(Ok(())) => None,
                    Ok(Err(err)) => Some(format!("{err:#}")),
                    Err(payload) => Some(panic_message(payload.as_ref())),
                };
                *worker_link
                    .failure
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = failure;
                worker_link.quit.store(true, Ordering::SeqCst);
                worker_link.ctx.request_repaint();
            });
            Ok(Box::new(Window {
                link,
                events: tx,
                zoom: load_zoom(),
                saved_zoom: load_zoom(),
            }))
        }),
    );
    // The worker may still be blocked on a game; leaving now is the intent.
    io::stdout().flush().ok();
    // Errors below must reach the real stderr: the pipe's copy thread
    // wouldn't get to run before the process exits.
    restore_output();
    if let Err(err) = result {
        bail!("couldn't open the Game Mode window: {err}");
    }
    let failure = outer_link
        .failure
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    match failure {
        Some(msg) => bail!("{msg}"),
        None => std::process::exit(0),
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    let text = payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string());
    format!("the Game Mode window's TUI crashed: {text}")
}

/// Points stdout/stderr back at what they were before `capture_output`.
fn restore_output() {
    if let Some(&(out, err)) = ORIGINAL_FDS.get() {
        // SAFETY: dup2 onto the standard fds, from fds we saved and own.
        unsafe {
            if out >= 0 {
                libc::dup2(out, 1);
            }
            if err >= 0 {
                libc::dup2(err, 2);
            }
        }
    }
}

/// Points our stdout/stderr (and so every child's) at a pipe the window
/// shows, still copying everything to the original stderr for logs.
/// Stdin becomes /dev/null: there's no terminal to type into.
fn capture_output(link: Arc<Link>) -> Result<()> {
    let mut fds = [0; 2];
    // SAFETY: plain fd juggling on fds we own, before other threads exist.
    let original_stderr = unsafe {
        if libc::pipe(fds.as_mut_ptr()) != 0 {
            bail!("pipe: {}", io::Error::last_os_error());
        }
        let saved = libc::dup(2);
        ORIGINAL_FDS.set((libc::dup(1), libc::dup(2))).ok();
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY);
        if null >= 0 {
            libc::dup2(null, 0);
            libc::close(null);
        }
        libc::dup2(fds[1], 1);
        libc::dup2(fds[1], 2);
        libc::close(fds[1]);
        saved
    };
    // SAFETY: both fds were just created above and are owned here.
    let mut reader = unsafe { std::fs::File::from_raw_fd(fds[0]) };
    let mut copy =
        (original_stderr >= 0).then(|| unsafe { std::fs::File::from_raw_fd(original_stderr) });
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        let mut pending: Vec<u8> = Vec::new();
        loop {
            let n = match reader.read(&mut chunk) {
                Ok(0) | Err(_) => return,
                Ok(n) => n,
            };
            if let Some(copy) = copy.as_mut() {
                copy.write_all(&chunk[..n]).ok();
            }
            pending.extend_from_slice(&chunk[..n]);
            let complete = complete_utf8_len(&pending);
            link.log
                .lock()
                .unwrap()
                .push_str(&String::from_utf8_lossy(&pending[..complete]));
            pending.drain(..complete);
            link.ctx.request_repaint();
        }
    });
    Ok(())
}

/// egui's monospace font plus a system CJK font, so Japanese/Chinese/Korean
/// game names render instead of showing boxes.
fn fonts() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    let found = Command::new("fc-match")
        .args(["-f", "%{file}|%{index}", "sans-serif:lang=ja"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok());
    if let Some((file, index)) = found.as_deref().and_then(|s| s.split_once('|'))
        && let Ok(bytes) = std::fs::read(file)
    {
        let mut data = egui::FontData::from_owned(bytes);
        data.index = index.trim().parse().unwrap_or(0);
        fonts.font_data.insert("cjk".into(), Arc::new(data));
        for family in [egui::FontFamily::Monospace, egui::FontFamily::Proportional] {
            fonts.families.entry(family).or_default().push("cjk".into());
        }
    }
    fonts
}

struct Window {
    link: Arc<Link>,
    events: Sender<Event>,
    zoom: f32,
    saved_zoom: f32,
}

/// Zoom is remembered across launches, in the data dir next to the library.
fn zoom_file() -> Option<std::path::PathBuf> {
    crate::config::project_dirs()
        .ok()
        .map(|d| d.data_dir().join("gamemode-zoom"))
}

fn load_zoom() -> f32 {
    zoom_file()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| s.trim().parse::<f32>().ok())
        .filter(|z| (0.3..=4.0).contains(z))
        .unwrap_or(1.0)
}

fn save_zoom(zoom: f32) {
    if let Some(path) = zoom_file() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        std::fs::write(path, format!("{zoom}\n")).ok();
    }
}

impl Window {
    /// Forwards input as crossterm events; Ctrl +/-/0 zoom stays here.
    fn forward_input(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|i| i.events.clone());
        for event in events {
            let sent = match event {
                egui::Event::Text(text) => {
                    for c in text.chars() {
                        self.send_key(KeyCode::Char(c), KeyModifiers::NONE);
                    }
                    continue;
                }
                egui::Event::Paste(text) => Event::Paste(text),
                egui::Event::Key {
                    key: k,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    if modifiers.command {
                        match k {
                            egui::Key::Equals | egui::Key::Plus => {
                                self.zoom = (self.zoom * 1.1).min(4.0);
                                continue;
                            }
                            egui::Key::Minus => {
                                self.zoom = (self.zoom / 1.1).max(0.3);
                                continue;
                            }
                            egui::Key::Num0 => {
                                self.zoom = 1.0;
                                continue;
                            }
                            _ => {}
                        }
                    }
                    match translate_key(k, modifiers) {
                        Some(event) => event,
                        None => continue,
                    }
                }
                _ => continue,
            };
            self.events.send(sent).ok();
        }
    }

    fn send_key(&self, code: KeyCode, modifiers: KeyModifiers) {
        self.events.send(key(code, modifiers)).ok();
    }
}

fn key(code: KeyCode, modifiers: KeyModifiers) -> Event {
    Event::Key(KeyEvent::new(code, modifiers))
}

/// Named keys, plus Ctrl+V for paste. Plain typing arrives as `Text`; other
/// Ctrl/Alt chords are dropped, since the TUI ignores modifiers and would
/// otherwise run the bare letter's action (Ctrl+S opening Share).
fn translate_key(k: egui::Key, m: egui::Modifiers) -> Option<Event> {
    use egui::Key as K;
    let mut mods = KeyModifiers::NONE;
    if m.ctrl || m.command {
        mods |= KeyModifiers::CONTROL;
    }
    if m.alt {
        mods |= KeyModifiers::ALT;
    }
    if m.shift {
        mods |= KeyModifiers::SHIFT;
    }
    let code = match k {
        K::ArrowUp => KeyCode::Up,
        K::ArrowDown => KeyCode::Down,
        K::ArrowLeft => KeyCode::Left,
        K::ArrowRight => KeyCode::Right,
        K::Enter => KeyCode::Enter,
        K::Escape => KeyCode::Esc,
        K::Tab if m.shift => KeyCode::BackTab,
        K::Tab => KeyCode::Tab,
        K::Backspace => KeyCode::Backspace,
        K::Delete => KeyCode::Delete,
        K::Insert => KeyCode::Insert,
        K::Home => KeyCode::Home,
        K::End => KeyCode::End,
        K::PageUp => KeyCode::PageUp,
        K::PageDown => KeyCode::PageDown,
        K::F1 => KeyCode::F(1),
        K::F2 => KeyCode::F(2),
        K::F3 => KeyCode::F(3),
        K::F4 => KeyCode::F(4),
        K::F5 => KeyCode::F(5),
        K::F6 => KeyCode::F(6),
        K::F7 => KeyCode::F(7),
        K::F8 => KeyCode::F(8),
        K::F9 => KeyCode::F(9),
        K::F10 => KeyCode::F(10),
        K::F11 => KeyCode::F(11),
        K::F12 => KeyCode::F(12),
        K::V if mods.contains(KeyModifiers::CONTROL) => {
            mods.remove(KeyModifiers::SHIFT);
            KeyCode::Char('v')
        }
        _ => return None,
    };
    Some(key(code, mods))
}

impl eframe::App for Window {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.link.quit.load(Ordering::SeqCst) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        self.forward_input(&ctx);
        let steps = self.link.zoom_steps.swap(0, Ordering::SeqCst);
        if steps != 0 {
            self.zoom = (self.zoom * 1.1f32.powi(steps)).clamp(0.3, 4.0);
        }
        if self.zoom != self.saved_zoom {
            save_zoom(self.zoom);
            self.saved_zoom = self.zoom;
        }

        let full = ctx.content_rect();
        let painter = ui.painter().clone();
        painter.rect_filled(full, 0.0, DEFAULT_BG);

        // Auto size targets ~50 rows: terminal-like density that stays
        // readable on a Deck's 800px-tall screen.
        let px = ((full.height() / 50.0).clamp(12.0, 48.0) * self.zoom).round();
        let font = FontId::monospace(px);
        let (cell_w, cell_h) = ctx.fonts_mut(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));
        let cols = ((full.width() / cell_w).floor() as u16).max(20);
        let rows = ((full.height() / cell_h).floor() as u16).max(8);
        {
            let mut size = self.link.size.lock().unwrap();
            if *size != (cols, rows) {
                *size = (cols, rows);
                // Have the worker redraw at the new size right away.
                self.events.send(Event::Resize(cols, rows)).ok();
            }
        }
        let origin = full.min
            + Vec2::new(
                (full.width() - cols as f32 * cell_w) / 2.0,
                (full.height() - rows as f32 * cell_h) / 2.0,
            );
        let cell_rect = |x: u16, y: u16, w: u16| {
            Rect::from_min_size(
                origin + Vec2::new(x as f32 * cell_w, y as f32 * cell_h),
                Vec2::new(w as f32 * cell_w, cell_h),
            )
        };

        if self.link.suspended.load(Ordering::SeqCst) {
            let log = self.link.log.lock().unwrap();
            for (y, text) in log
                .tail(rows as usize, cols as usize)
                .into_iter()
                .enumerate()
            {
                painter.text(
                    cell_rect(0, y as u16, 1).min,
                    egui::Align2::LEFT_TOP,
                    text,
                    font.clone(),
                    DEFAULT_FG,
                );
            }
            return;
        }

        let screen = self.link.screen.lock().unwrap();
        let area = screen.area;
        let colors = |cell: &Cell| {
            let (mut fg, mut bg) = (to_color(cell.fg, DEFAULT_FG), to_color(cell.bg, DEFAULT_BG));
            if cell.modifier.contains(Modifier::REVERSED) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if cell.modifier.contains(Modifier::DIM) {
                fg = fg.lerp_to_gamma(bg, 0.4);
            }
            (fg, bg)
        };
        let (w, h) = (area.width.min(cols), area.height.min(rows));
        // Backgrounds first: a wide (CJK) glyph spills into the next cell,
        // whose background would otherwise paint over its right half.
        for y in 0..h {
            for x in 0..w {
                let cell = &screen[(x, y)];
                let (_, bg) = colors(cell);
                if bg != DEFAULT_BG {
                    // ratatui resets a wide glyph's hidden second cell, so
                    // the glyph's own cell carries the background for both.
                    let width = UnicodeWidthStr::width(cell.symbol()).clamp(1, 2) as u16;
                    painter.rect_filled(cell_rect(x, y, width), 0.0, bg);
                }
            }
        }
        for y in 0..h {
            for x in 0..w {
                let cell = &screen[(x, y)];
                let (fg, _) = colors(cell);
                let rect = cell_rect(x, y, 1);
                let symbol = cell.symbol();
                if !symbol.is_empty() && symbol != " " {
                    painter.text(rect.min, egui::Align2::LEFT_TOP, symbol, font.clone(), fg);
                }
                if cell.modifier.contains(Modifier::UNDERLINED) {
                    let y = rect.max.y - 1.0;
                    painter.line_segment(
                        [Pos2::new(rect.min.x, y), Pos2::new(rect.max.x, y)],
                        egui::Stroke::new(1.0, fg),
                    );
                }
            }
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::from(DEFAULT_BG).to_array()
    }
}

/// ratatui colors to the window's palette (xterm defaults for the 16 named
/// colors and the 256-color cube).
fn to_color(color: Color, default: Color32) -> Color32 {
    const ANSI: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 49, 49),
        (13, 188, 121),
        (229, 229, 16),
        (36, 114, 200),
        (188, 63, 188),
        (17, 168, 205),
        (204, 204, 204),
        (102, 102, 102),
        (241, 76, 76),
        (35, 209, 139),
        (245, 245, 67),
        (59, 142, 234),
        (214, 112, 214),
        (41, 184, 219),
        (242, 242, 242),
    ];
    let index = match color {
        Color::Reset => return default,
        Color::Rgb(r, g, b) => return Color32::from_rgb(r, g, b),
        Color::Black => 0,
        Color::Red => 1,
        Color::Green => 2,
        Color::Yellow => 3,
        Color::Blue => 4,
        Color::Magenta => 5,
        Color::Cyan => 6,
        Color::Gray => 7,
        Color::DarkGray => 8,
        Color::LightRed => 9,
        Color::LightGreen => 10,
        Color::LightYellow => 11,
        Color::LightBlue => 12,
        Color::LightMagenta => 13,
        Color::LightCyan => 14,
        Color::White => 15,
        Color::Indexed(i) => i,
    };
    match index {
        0..=15 => {
            let (r, g, b) = ANSI[index as usize];
            Color32::from_rgb(r, g, b)
        }
        16..=231 => {
            let i = index - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            Color32::from_rgb(level(i / 36), level((i / 6) % 6), level(i % 6))
        }
        _ => {
            let v = 8 + (index - 232) * 10;
            Color32::from_rgb(v, v, v)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_size_accepts_width_x_height() {
        assert_eq!(parse_size("1920x1200").unwrap(), (1920, 1200));
        assert_eq!(parse_size("1280X800").unwrap(), (1280, 800));
        assert!(parse_size("1920").is_err());
        assert!(parse_size("10x10").is_err());
    }

    #[test]
    fn log_strips_escapes_and_handles_carriage_returns() {
        let mut log = Log::default();
        log.push_str("\x1b[1;32mok\x1b[0m\nprogress 10%\rprogress 90%\n\x1b]0;title\x07done");
        assert_eq!(log.tail(10, 80), vec!["ok", "progress 90%", "done"]);
    }

    #[test]
    fn crlf_lines_keep_their_text() {
        let mut log = Log::default();
        log.push_str("wine line one\r\nline two\r");
        log.push_str("\nend");
        assert_eq!(log.tail(10, 80), vec!["wine line one", "line two", "end"]);
    }

    #[test]
    fn long_lines_wrap_and_tail_counts_screen_rows() {
        let mut log = Log::default();
        log.push_str("first\nabcdefghij");
        assert_eq!(log.tail(3, 4), vec!["abcd", "efgh", "ij"]);
        assert_eq!(wrap("日本語", 4), vec!["日本", "語"]);
    }

    #[test]
    fn incomplete_trailing_utf8_is_held_back() {
        let bytes = "a日".as_bytes();
        assert_eq!(complete_utf8_len(bytes), bytes.len());
        assert_eq!(complete_utf8_len(&bytes[..2]), 1);
        assert_eq!(complete_utf8_len(&bytes[..3]), 1);
        assert_eq!(complete_utf8_len(b"abc"), 3);
        assert_eq!(complete_utf8_len(b""), 0);
    }

    #[test]
    fn ctrl_letters_become_crossterm_ctrl_chars() {
        let ctrl = egui::Modifiers {
            ctrl: true,
            ..Default::default()
        };
        assert_eq!(
            translate_key(egui::Key::V, ctrl),
            Some(key(KeyCode::Char('v'), KeyModifiers::CONTROL))
        );
        assert_eq!(translate_key(egui::Key::V, egui::Modifiers::NONE), None);
        assert_eq!(translate_key(egui::Key::S, ctrl), None);
        let shift = egui::Modifiers {
            shift: true,
            ..Default::default()
        };
        assert_eq!(
            translate_key(egui::Key::Tab, shift),
            Some(key(KeyCode::BackTab, KeyModifiers::SHIFT))
        );
    }

    #[test]
    fn indexed_colors_follow_the_xterm_cube() {
        assert_eq!(
            to_color(Color::Indexed(16), DEFAULT_FG),
            Color32::from_rgb(0, 0, 0)
        );
        assert_eq!(
            to_color(Color::Indexed(231), DEFAULT_FG),
            Color32::from_rgb(255, 255, 255)
        );
        assert_eq!(to_color(Color::Reset, DEFAULT_FG), DEFAULT_FG);
    }
}
