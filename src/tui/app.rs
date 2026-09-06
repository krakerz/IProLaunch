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
    EnvTable,
    WineDllOverrideTable,
    /// Not really a config *value* — a toggle action ("install"/"uninstall")
    /// rendered in its own section below the rest of the config fields, per
    /// the user's request. Deliberately last in `ALL` for that reason.
    Integrate,
}

impl ConfigField {
    pub const ALL: [ConfigField; 14] = [
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
        ConfigField::EnvTable,
        ConfigField::WineDllOverrideTable,
        ConfigField::Integrate,
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
            ConfigField::EnvTable => "env (global)",
            ConfigField::WineDllOverrideTable => "winedlloverride (global)",
            ConfigField::Integrate => "desktop integration",
        }
    }

    /// How this field reacts to Enter: cycle/toggle in place, open a text
    /// popup, open the proton build picker, open the multi-line map editor,
    /// or (install/uninstall) run the integrate action directly. Per-profile
    /// overrides for any of these still aren't editable from the TUI — see
    /// Help tab.
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
            ConfigField::EnvTable | ConfigField::WineDllOverrideTable => FieldKind::MapEditor,
            ConfigField::Integrate => FieldKind::IntegrationToggle,
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
    MapEditor,
    IntegrationToggle,
}

/// Which per-game override a Library "edit profile" row edits. Every one of
/// these is an override — blank/`Enter` on the "inherit" choice clears it
/// back to the global default rather than deleting the profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileField {
    Title,
    Args,
    Proton,
    PrefixPath,
    WindowsVersion,
    LogKeep,
    LogRecord,
    LogAutoOpen,
    EnvTable,
    WineDllOverrideTable,
}

impl ProfileField {
    pub const ALL: [ProfileField; 10] = [
        ProfileField::Title,
        ProfileField::Args,
        ProfileField::Proton,
        ProfileField::PrefixPath,
        ProfileField::WindowsVersion,
        ProfileField::LogKeep,
        ProfileField::LogRecord,
        ProfileField::LogAutoOpen,
        ProfileField::EnvTable,
        ProfileField::WineDllOverrideTable,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ProfileField::Title => "title (for GAMEID matching)",
            ProfileField::Args => "args (space-separated)",
            ProfileField::Proton => "defaults.proton override",
            ProfileField::PrefixPath => "defaults.prefix_path override",
            ProfileField::WindowsVersion => "defaults.windows-version override",
            ProfileField::LogKeep => "logging.keep override",
            ProfileField::LogRecord => "logging.record override",
            ProfileField::LogAutoOpen => "logging.auto_open override",
            ProfileField::EnvTable => "env override",
            ProfileField::WineDllOverrideTable => "winedlloverride override",
        }
    }

    /// Same shape as `ConfigField::kind`, except every non-table field here
    /// is an `Option` in the underlying `Profile` — blank text / a dedicated
    /// "inherit" choice means "no override", not "empty string"/"zero".
    /// `LogKeep` is a `Text` field (not `Number`) specifically so blank can
    /// mean "inherit" — a plain number field has no clean way to represent
    /// that.
    pub fn kind(self) -> FieldKind {
        match self {
            ProfileField::Proton => FieldKind::ProtonPicker,
            ProfileField::LogRecord | ProfileField::LogAutoOpen => FieldKind::Cycle,
            ProfileField::Title
            | ProfileField::Args
            | ProfileField::PrefixPath
            | ProfileField::WindowsVersion
            | ProfileField::LogKeep => FieldKind::Text,
            ProfileField::EnvTable | ProfileField::WineDllOverrideTable => FieldKind::MapEditor,
        }
    }
}

pub enum Mode {
    Normal,
    /// Free-text edit of a config field or a new library path.
    TextInput {
        purpose: TextInputPurpose,
        buffer: String,
    },
    /// Picking a Proton build for `defaults.proton` (global or a profile
    /// override, per `target`).
    ProtonPicker {
        builds: Vec<ProtonBuild>,
        selected: usize,
        target: ProtonPickerTarget,
    },
    /// Browsing one map's entries (`env` or `winedlloverride`): `a` add,
    /// `e` edit the selected entry, `d` delete it, Esc back to Config.
    MapEditor {
        field: MapField,
        selected: usize,
    },
    /// Add/edit flow for one entry of a map — key then value, each a plain
    /// single-line input. We join them into `KEY=VALUE` ourselves, so the
    /// user only ever types one bare value at a time, never that syntax.
    MapEntryInput {
        field: MapField,
        /// `Some(key)` when editing an existing entry (so we know which key
        /// to remove if the name itself gets changed); `None` when adding.
        original_key: Option<String>,
        step: MapEntryStep,
        key: String,
        value: String,
    },
    /// Confirming a destructive Library `d` (delete profile) before it
    /// happens — separate from every edit-in-place mode above since this is
    /// the only one that can't be undone by just pressing Esc partway
    /// through.
    ConfirmDeleteProfile {
        slug: String,
        name: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapEntryStep {
    Key,
    Value,
}

/// Which config a `Mode::ProtonPicker` session is setting: the global
/// default, or one profile's override (which also gets an "inherit" choice
/// the global picker doesn't need).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtonPickerTarget {
    Global,
    Profile(String),
}

pub enum TextInputPurpose {
    ConfigField(ConfigField),
    AddLibraryPath,
    /// Follow-up prompt shown right after a fresh add-by-path succeeds, so a
    /// newly-added profile isn't stuck with only the weak exe-stem fallback
    /// for GAMEID matching. Holds the new profile's slug; an empty buffer
    /// just skips (leaves `title` unset, same as before this existed).
    ProfileTitle(String),
    /// A profile-editor text field (slug + which field) — unlike
    /// `ProfileTitle` above, an empty buffer here means "clear the
    /// override" (revert to inherit), consistent with every other override
    /// field in the profile editor.
    ProfileField(String, ProfileField),
}

/// Which map a `Mode::MapEditor`/`MapEntryInput` session is editing: the
/// global `env`/`winedlloverride`, or one profile's override table (slug).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapField {
    Env,
    WineDllOverride,
    ProfileEnv(String),
    ProfileWineDllOverride(String),
}

impl MapField {
    pub fn label(&self) -> String {
        match self {
            MapField::Env => "env".to_string(),
            MapField::WineDllOverride => "winedlloverride".to_string(),
            MapField::ProfileEnv(slug) => format!("{slug}'s env override"),
            MapField::ProfileWineDllOverride(slug) => format!("{slug}'s winedlloverride override"),
        }
    }
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

    /// `Some(slug)` while the Library tab is showing that profile's editor
    /// instead of the plain list — set by `e`, cleared by Esc. Kept at the
    /// `App` level (like `config_selected`) rather than inside `Mode`, so
    /// `Mode::Normal` still means "no popup is open" even while the editor
    /// is showing; every popup mode (`TextInput`, `MapEditor`, ...) opened
    /// from the editor still returns here correctly since they only ever
    /// reset `mode`, never this field.
    pub profile_editor: Option<String>,
    pub profile_field_selected: usize,
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
            profile_editor: None,
            profile_field_selected: 0,
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

    /// The map a `MapField` refers to, as a sorted `(key, value)` list —
    /// sorted because it's a `BTreeMap`, so this is stable across calls
    /// (important since `Mode::MapEditor.selected` indexes into it). Empty
    /// for a profile variant whose slug no longer exists (e.g. deleted from
    /// under the editor) rather than panicking.
    pub fn map_entries(&self, field: &MapField) -> Vec<(String, String)> {
        self.map(field)
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default()
    }

    pub fn map(&self, field: &MapField) -> Option<&std::collections::BTreeMap<String, String>> {
        match field {
            MapField::Env => Some(&self.cfg.env),
            MapField::WineDllOverride => Some(&self.cfg.winedlloverride),
            MapField::ProfileEnv(slug) => self.profile(slug).map(|p| &p.env),
            MapField::ProfileWineDllOverride(slug) => {
                self.profile(slug).map(|p| &p.winedlloverride)
            }
        }
    }

    pub fn map_mut(
        &mut self,
        field: &MapField,
    ) -> Option<&mut std::collections::BTreeMap<String, String>> {
        match field {
            MapField::Env => Some(&mut self.cfg.env),
            MapField::WineDllOverride => Some(&mut self.cfg.winedlloverride),
            MapField::ProfileEnv(slug) => self.profile_mut(slug).map(|p| &mut p.env),
            MapField::ProfileWineDllOverride(slug) => {
                self.profile_mut(slug).map(|p| &mut p.winedlloverride)
            }
        }
    }

    pub fn profile(&self, slug: &str) -> Option<&Profile> {
        self.profiles
            .iter()
            .find(|(s, _)| s == slug)
            .map(|(_, p)| p)
    }

    pub fn profile_mut(&mut self, slug: &str) -> Option<&mut Profile> {
        self.profiles
            .iter_mut()
            .find(|(s, _)| s == slug)
            .map(|(_, p)| p)
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

/// Same as `next_record_mode`, but for a profile override, which also has
/// an "inherit the global setting" state (`None`) in the cycle.
pub fn next_profile_record_mode(m: Option<RecordMode>) -> Option<RecordMode> {
    match m {
        None => Some(RecordMode::All),
        Some(RecordMode::All) => Some(RecordMode::Errors),
        Some(RecordMode::Errors) => Some(RecordMode::Off),
        Some(RecordMode::Off) => None,
    }
}

/// Cycles a profile's `logging.auto_open` override: inherit → on → off → inherit.
pub fn next_profile_auto_open(v: Option<bool>) -> Option<bool> {
    match v {
        None => Some(true),
        Some(true) => Some(false),
        Some(false) => None,
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
    fn profile_record_mode_cycle_includes_inherit_and_returns_to_it() {
        let mut r = None;
        r = next_profile_record_mode(r);
        assert_eq!(r, Some(RecordMode::All));
        r = next_profile_record_mode(r);
        assert_eq!(r, Some(RecordMode::Errors));
        r = next_profile_record_mode(r);
        assert_eq!(r, Some(RecordMode::Off));
        r = next_profile_record_mode(r);
        assert_eq!(r, None);
    }

    #[test]
    fn profile_auto_open_cycle_includes_inherit_and_returns_to_it() {
        let mut v = None;
        v = next_profile_auto_open(v);
        assert_eq!(v, Some(true));
        v = next_profile_auto_open(v);
        assert_eq!(v, Some(false));
        v = next_profile_auto_open(v);
        assert_eq!(v, None);
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
