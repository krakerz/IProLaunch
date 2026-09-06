use crate::config::{Config, LogMode, PrefixMode, Profile, RecordMode};
use crate::proton::ProtonBuild;
use crate::running::{self, RunningEntry};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Running,
    Library,
    Config,
    Help,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Running, Tab::Library, Tab::Config, Tab::Help];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Running => "Running",
            Tab::Library => "Library",
            Tab::Config => "Config",
            Tab::Help => "Help",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    pub fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    pub fn prev(self) -> Self {
        let n = Self::ALL.len();
        Self::ALL[(self.index() + n - 1) % n]
    }
}

/// Which global config field a config-tab row edits. Order here is the
/// order rows render in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigField {
    Proton,
    PrefixMode,
    PrefixPath,
    PrefixesRoot,
    WindowsVersion,
    LogMode,
    LogPath,
    LogKeep,
    LogRecord,
    LogAutoOpen,
    GamedbInterval,
}

impl ConfigField {
    pub const ALL: [ConfigField; 11] = [
        ConfigField::Proton,
        ConfigField::PrefixMode,
        ConfigField::PrefixPath,
        ConfigField::PrefixesRoot,
        ConfigField::WindowsVersion,
        ConfigField::LogMode,
        ConfigField::LogPath,
        ConfigField::LogKeep,
        ConfigField::LogRecord,
        ConfigField::LogAutoOpen,
        ConfigField::GamedbInterval,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ConfigField::Proton => "defaults.proton",
            ConfigField::PrefixMode => "defaults.prefix_mode",
            ConfigField::PrefixPath => "defaults.prefix_path",
            ConfigField::PrefixesRoot => "defaults.prefixes_root",
            ConfigField::WindowsVersion => "defaults.windows-version",
            ConfigField::LogMode => "logging.mode",
            ConfigField::LogPath => "logging.path (blank = default)",
            ConfigField::LogKeep => "logging.keep",
            ConfigField::LogRecord => "logging.record",
            ConfigField::LogAutoOpen => "logging.auto_open",
            ConfigField::GamedbInterval => "gamedb.update_interval_days",
        }
    }

    /// How this field reacts to Enter: cycle/toggle in place, open a text
    /// popup, or (proton only) open the build picker. `env` isn't listed
    /// here at all — not editable from the TUI in this pass, see Help tab.
    pub fn kind(self) -> FieldKind {
        match self {
            ConfigField::Proton => FieldKind::ProtonPicker,
            ConfigField::PrefixMode | ConfigField::LogMode | ConfigField::LogRecord => {
                FieldKind::Cycle
            }
            ConfigField::LogAutoOpen => FieldKind::Toggle,
            ConfigField::LogKeep | ConfigField::GamedbInterval => FieldKind::Number,
            ConfigField::PrefixPath
            | ConfigField::PrefixesRoot
            | ConfigField::WindowsVersion
            | ConfigField::LogPath => FieldKind::Text,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Cycle,
    Toggle,
    Number,
    Text,
    ProtonPicker,
}

pub enum Mode {
    Normal,
    /// Free-text edit of a config field or a new library path.
    TextInput {
        purpose: TextInputPurpose,
        buffer: String,
    },
    /// Picking a Proton build for `defaults.proton`.
    ProtonPicker {
        builds: Vec<ProtonBuild>,
        selected: usize,
    },
}

pub enum TextInputPurpose {
    ConfigField(ConfigField),
    AddLibraryPath,
}

pub struct App {
    pub cfg: Config,
    pub tab: Tab,
    pub mode: Mode,
    pub should_quit: bool,
    pub status: Option<String>,

    pub running: Vec<RunningEntry>,
    pub running_selected: usize,

    pub profiles: Vec<(String, Profile)>,
    pub library_selected: usize,

    pub config_selected: usize,
}

impl App {
    pub fn new(cfg: Config) -> Self {
        let mut app = Self {
            cfg,
            tab: Tab::Running,
            mode: Mode::Normal,
            should_quit: false,
            status: None,
            running: Vec::new(),
            running_selected: 0,
            profiles: Vec::new(),
            library_selected: 0,
            config_selected: 0,
        };
        app.refresh_running();
        app.refresh_profiles();
        app
    }

    pub fn refresh_running(&mut self) {
        match running::list_live() {
            Ok(entries) => self.running = entries,
            Err(err) => self.status = Some(format!("couldn't list running games: {err:#}")),
        }
        if self.running_selected >= self.running.len() {
            self.running_selected = self.running.len().saturating_sub(1);
        }
    }

    pub fn refresh_profiles(&mut self) {
        match Profile::load_all() {
            Ok(mut profiles) => {
                profiles.sort_by_key(|p| p.1.name.to_lowercase());
                self.profiles = profiles;
            }
            Err(err) => self.status = Some(format!("couldn't list library: {err:#}")),
        }
        if self.library_selected >= self.profiles.len() {
            self.library_selected = self.profiles.len().saturating_sub(1);
        }
    }

    pub fn next_tab(&mut self) {
        self.tab = self.tab.next();
    }

    pub fn prev_tab(&mut self) {
        self.tab = self.tab.prev();
    }
}

/// Moves a list selection up (`delta < 0`) or down (`delta > 0`), clamped to
/// a valid index for a list of `len` items (`0` if `len == 0`). Shared by
/// every tab's Up/Down handling instead of each re-deriving the same
/// saturating-add/sub clamp.
pub fn move_selection(selected: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    let selected = selected.min(len - 1);
    if delta < 0 {
        selected.saturating_sub(delta.unsigned_abs())
    } else {
        (selected + delta as usize).min(len - 1)
    }
}

/// Cycles an enum value forward, for `FieldKind::Cycle` fields.
pub fn next_prefix_mode(m: PrefixMode) -> PrefixMode {
    match m {
        PrefixMode::Single => PrefixMode::PerExe,
        PrefixMode::PerExe => PrefixMode::Single,
    }
}

pub fn next_log_mode(m: LogMode) -> LogMode {
    match m {
        LogMode::Single => LogMode::Each,
        LogMode::Each => LogMode::Single,
    }
}

pub fn next_record_mode(m: RecordMode) -> RecordMode {
    match m {
        RecordMode::All => RecordMode::Errors,
        RecordMode::Errors => RecordMode::Off,
        RecordMode::Off => RecordMode::All,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_cycling_wraps_both_ways() {
        assert_eq!(Tab::Running.next(), Tab::Library);
        assert_eq!(Tab::Help.next(), Tab::Running);
        assert_eq!(Tab::Running.prev(), Tab::Help);
    }

    #[test]
    fn enum_cycles_cover_every_variant_and_return_to_start() {
        let mut m = PrefixMode::Single;
        m = next_prefix_mode(m);
        assert_eq!(m, PrefixMode::PerExe);
        m = next_prefix_mode(m);
        assert_eq!(m, PrefixMode::Single);

        let mut r = RecordMode::All;
        r = next_record_mode(r);
        assert_eq!(r, RecordMode::Errors);
        r = next_record_mode(r);
        assert_eq!(r, RecordMode::Off);
        r = next_record_mode(r);
        assert_eq!(r, RecordMode::All);
    }

    #[test]
    fn move_selection_clamps_at_both_ends() {
        assert_eq!(move_selection(0, 3, -1), 0); // can't go below 0
        assert_eq!(move_selection(2, 3, 1), 2); // can't go past the last index
        assert_eq!(move_selection(1, 3, 1), 2);
        assert_eq!(move_selection(1, 3, -1), 0);
    }

    #[test]
    fn move_selection_on_empty_list_is_always_zero() {
        assert_eq!(move_selection(5, 0, 1), 0);
        assert_eq!(move_selection(5, 0, -1), 0);
    }

    #[test]
    fn move_selection_clamps_an_out_of_range_start_first() {
        // Defensive: if `len` shrank since `selected` was set (e.g. a
        // profile got deleted from disk), don't panic or go further out of
        // range — snap into range before applying delta.
        assert_eq!(move_selection(10, 3, 1), 2);
        assert_eq!(move_selection(10, 3, -1), 1);
    }
}
