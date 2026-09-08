use std::collections::HashSet;
use std::path::Path;

use crate::config::{
    Config, GamescopeFilter, GamescopeScaler, GamescopeSetting, LogMode, PrefixMode, Profile,
    RecordMode,
};
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
/// order rows render in. Desktop integration is deliberately *not* one of
/// these — it's a separate table (`IntegrateField`) rendered in its own
/// block below this list, per the user's request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigField {
    Proton,
    PrefixMode,
    PrefixPath,
    PrefixesRoot,
    WindowsVersion,
    Gamescope,
    GamescopeOutputWidth,
    GamescopeOutputHeight,
    GamescopeRefresh,
    GamescopeNestedWidth,
    GamescopeNestedHeight,
    GamescopeFilter,
    GamescopeScaler,
    GamescopeBorderless,
    GamescopeGrabCursor,
    GamescopeAdaptiveSync,
    LaunchWrapper,
    LogMode,
    LogPath,
    LogKeep,
    LogRecord,
    LogAutoOpen,
    GamedbInterval,
    EnvTable,
    WineDllOverrideTable,
}

impl ConfigField {
    pub const ALL: [ConfigField; 25] = [
        ConfigField::Proton,
        ConfigField::PrefixMode,
        ConfigField::PrefixPath,
        ConfigField::PrefixesRoot,
        ConfigField::WindowsVersion,
        ConfigField::Gamescope,
        ConfigField::GamescopeOutputWidth,
        ConfigField::GamescopeOutputHeight,
        ConfigField::GamescopeRefresh,
        ConfigField::GamescopeNestedWidth,
        ConfigField::GamescopeNestedHeight,
        ConfigField::GamescopeFilter,
        ConfigField::GamescopeScaler,
        ConfigField::GamescopeBorderless,
        ConfigField::GamescopeGrabCursor,
        ConfigField::GamescopeAdaptiveSync,
        ConfigField::LaunchWrapper,
        ConfigField::LogMode,
        ConfigField::LogPath,
        ConfigField::LogKeep,
        ConfigField::LogRecord,
        ConfigField::LogAutoOpen,
        ConfigField::GamedbInterval,
        ConfigField::EnvTable,
        ConfigField::WineDllOverrideTable,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ConfigField::Proton => "defaults.proton",
            ConfigField::PrefixMode => "defaults.prefix_mode",
            ConfigField::PrefixPath => "defaults.prefix_path",
            ConfigField::PrefixesRoot => "defaults.prefixes_root",
            ConfigField::WindowsVersion => "defaults.windows-version",
            ConfigField::Gamescope => "defaults.gamescope",
            ConfigField::GamescopeOutputWidth => "gamescope_settings.output_width",
            ConfigField::GamescopeOutputHeight => "gamescope_settings.output_height",
            ConfigField::GamescopeRefresh => "gamescope_settings.refresh",
            ConfigField::GamescopeNestedWidth => "gamescope_settings.nested_width",
            ConfigField::GamescopeNestedHeight => "gamescope_settings.nested_height",
            ConfigField::GamescopeFilter => "gamescope_settings.filter",
            ConfigField::GamescopeScaler => "gamescope_settings.scaler",
            ConfigField::GamescopeBorderless => "gamescope_settings.borderless",
            ConfigField::GamescopeGrabCursor => "gamescope_settings.grab_cursor",
            ConfigField::GamescopeAdaptiveSync => "gamescope_settings.adaptive_sync",
            ConfigField::LaunchWrapper => "defaults.launch_wrapper (blank = none)",
            ConfigField::LogMode => "logging.mode",
            ConfigField::LogPath => "logging.path (blank = default)",
            ConfigField::LogKeep => "logging.keep",
            ConfigField::LogRecord => "logging.record",
            ConfigField::LogAutoOpen => "logging.auto_open",
            ConfigField::GamedbInterval => "gamedb.update_interval_days",
            ConfigField::EnvTable => "env (global)",
            ConfigField::WineDllOverrideTable => "winedlloverride (global)",
        }
    }

    /// How this field reacts to Enter: cycle/toggle in place, open a text
    /// popup, open the proton build picker, or open the multi-line map
    /// editor. Per-profile overrides for any of these are edited from the
    /// Library tab's profile editor instead — see Help tab.
    pub fn kind(self) -> FieldKind {
        match self {
            ConfigField::Proton => FieldKind::ProtonPicker,
            ConfigField::PrefixMode
            | ConfigField::LogMode
            | ConfigField::LogRecord
            | ConfigField::Gamescope
            | ConfigField::GamescopeFilter
            | ConfigField::GamescopeScaler
            | ConfigField::GamescopeBorderless
            | ConfigField::GamescopeGrabCursor
            | ConfigField::GamescopeAdaptiveSync => FieldKind::Cycle,
            ConfigField::LogAutoOpen => FieldKind::Toggle,
            ConfigField::LogKeep | ConfigField::GamedbInterval => FieldKind::Number,
            ConfigField::PrefixPath
            | ConfigField::PrefixesRoot
            | ConfigField::WindowsVersion
            | ConfigField::LogPath
            | ConfigField::GamescopeOutputWidth
            | ConfigField::GamescopeOutputHeight
            | ConfigField::GamescopeRefresh
            | ConfigField::GamescopeNestedWidth
            | ConfigField::GamescopeNestedHeight
            | ConfigField::LaunchWrapper => FieldKind::Text,
            ConfigField::EnvTable | ConfigField::WineDllOverrideTable => FieldKind::MapEditor,
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
}

/// Rows of the Config tab's separate "Desktop integration" table, rendered
/// below the main `ConfigField` list but sharing one continuous selection
/// index with it (`App::config_selected` — see `App::is_integrate_selected`)
/// so Up/Down flows naturally from one into the other without a separate
/// focus-switch key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrateField {
    /// Info row: installed or not. Not editable — Enter is a no-op.
    Status,
    /// Info row: the full path of the binary the installed `.desktop` entry
    /// points at (from the `.desktop` file itself, not this process's own
    /// `current_exe()` — they can differ if the binary moved since
    /// install). Not editable — Enter is a no-op.
    BinaryPath,
    Setup,
    Reapply,
    Uninstall,
}

impl IntegrateField {
    pub const ALL: [IntegrateField; 5] = [
        IntegrateField::Status,
        IntegrateField::BinaryPath,
        IntegrateField::Setup,
        IntegrateField::Reapply,
        IntegrateField::Uninstall,
    ];

    pub fn label(self) -> &'static str {
        match self {
            IntegrateField::Status => "status",
            IntegrateField::BinaryPath => "binary location",
            IntegrateField::Setup => "setup desktop integration",
            IntegrateField::Reapply => "reapply (refresh binary location)",
            IntegrateField::Uninstall => "uninstall desktop integration",
        }
    }
}

/// Which of the three desktop-integration actions a `IntegrateField::Setup`/
/// `Reapply`/`Uninstall` row runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrateAction {
    Setup,
    Reapply,
    Uninstall,
}

/// Which per-game override a Library "edit profile" row edits. Every field
/// except `TargetPath`/`Slug`/`Name` is an override — blank/`Enter` on the
/// "inherit" choice clears it back to the global default rather than
/// deleting the profile. `TargetPath` is the one mandatory field (a profile
/// with no exe to launch is meaningless); `Slug` and `Name` are identifiers
/// with app-managed uniqueness (a folder rename and an auto-grown `#N`
/// respectively) — all three are validated/computed instead of
/// "inherit"-able — see `profile_editor::apply_text_field`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileField {
    TargetPath,
    Slug,
    Name,
    Title,
    Args,
    Proton,
    PrefixPath,
    WindowsVersion,
    Gamescope,
    GamescopeOutputWidth,
    GamescopeOutputHeight,
    GamescopeRefresh,
    GamescopeNestedWidth,
    GamescopeNestedHeight,
    GamescopeFilter,
    GamescopeScaler,
    GamescopeBorderless,
    GamescopeGrabCursor,
    GamescopeAdaptiveSync,
    LaunchWrapper,
    LogKeep,
    LogRecord,
    LogAutoOpen,
    EnvTable,
    WineDllOverrideTable,
}

impl ProfileField {
    pub const ALL: [ProfileField; 25] = [
        ProfileField::TargetPath,
        ProfileField::Slug,
        ProfileField::Name,
        ProfileField::Title,
        ProfileField::Args,
        ProfileField::Proton,
        ProfileField::PrefixPath,
        ProfileField::WindowsVersion,
        ProfileField::Gamescope,
        ProfileField::GamescopeOutputWidth,
        ProfileField::GamescopeOutputHeight,
        ProfileField::GamescopeRefresh,
        ProfileField::GamescopeNestedWidth,
        ProfileField::GamescopeNestedHeight,
        ProfileField::GamescopeFilter,
        ProfileField::GamescopeScaler,
        ProfileField::GamescopeBorderless,
        ProfileField::GamescopeGrabCursor,
        ProfileField::GamescopeAdaptiveSync,
        ProfileField::LaunchWrapper,
        ProfileField::LogKeep,
        ProfileField::LogRecord,
        ProfileField::LogAutoOpen,
        ProfileField::EnvTable,
        ProfileField::WineDllOverrideTable,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ProfileField::TargetPath => "target-path (exe location)",
            ProfileField::Slug => "slug (folder name)",
            ProfileField::Name => "name (#N auto-managed)",
            ProfileField::Title => "title (for GAMEID matching)",
            ProfileField::Args => "args (space-separated)",
            ProfileField::Proton => "defaults.proton override (per-slug mode only)",
            ProfileField::PrefixPath => "defaults.prefix_path override",
            ProfileField::WindowsVersion => "defaults.windows-version override",
            ProfileField::Gamescope => "defaults.gamescope override",
            ProfileField::GamescopeOutputWidth => "gamescope_settings.output_width override",
            ProfileField::GamescopeOutputHeight => "gamescope_settings.output_height override",
            ProfileField::GamescopeRefresh => "gamescope_settings.refresh override",
            ProfileField::GamescopeNestedWidth => "gamescope_settings.nested_width override",
            ProfileField::GamescopeNestedHeight => "gamescope_settings.nested_height override",
            ProfileField::GamescopeFilter => "gamescope_settings.filter override",
            ProfileField::GamescopeScaler => "gamescope_settings.scaler override",
            ProfileField::GamescopeBorderless => "gamescope_settings.borderless override",
            ProfileField::GamescopeGrabCursor => "gamescope_settings.grab_cursor override",
            ProfileField::GamescopeAdaptiveSync => "gamescope_settings.adaptive_sync override",
            ProfileField::LaunchWrapper => "defaults.launch_wrapper override",
            ProfileField::LogKeep => "logging.keep override",
            ProfileField::LogRecord => "logging.record override",
            ProfileField::LogAutoOpen => "logging.auto_open override",
            ProfileField::EnvTable => "env override",
            ProfileField::WineDllOverrideTable => "winedlloverride override",
        }
    }

    /// Same shape as `ConfigField::kind`, except every non-table field here
    /// (other than `TargetPath`/`Slug`/`Name`) is an `Option` in the
    /// underlying `Profile` — blank text / a dedicated "inherit" choice
    /// means "no override", not "empty string"/"zero". `LogKeep` is a
    /// `Text` field (not `Number`) specifically so blank can mean
    /// "inherit" — a plain number field has no clean way to represent
    /// that.
    pub fn kind(self) -> FieldKind {
        match self {
            ProfileField::Proton => FieldKind::ProtonPicker,
            ProfileField::LogRecord
            | ProfileField::LogAutoOpen
            | ProfileField::Gamescope
            | ProfileField::GamescopeFilter
            | ProfileField::GamescopeScaler
            | ProfileField::GamescopeBorderless
            | ProfileField::GamescopeGrabCursor
            | ProfileField::GamescopeAdaptiveSync => FieldKind::Cycle,
            ProfileField::TargetPath
            | ProfileField::Slug
            | ProfileField::Name
            | ProfileField::Title
            | ProfileField::Args
            | ProfileField::PrefixPath
            | ProfileField::WindowsVersion
            | ProfileField::LogKeep
            | ProfileField::GamescopeOutputWidth
            | ProfileField::GamescopeOutputHeight
            | ProfileField::GamescopeRefresh
            | ProfileField::GamescopeNestedWidth
            | ProfileField::GamescopeNestedHeight
            | ProfileField::LaunchWrapper => FieldKind::Text,
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
        /// Char index (not byte offset — see `mod::handle_text_input`,
        /// which always operates on `buffer.chars()`), so Left/Right can
        /// move within the text instead of only ever appending at the end
        /// — handy for fixing one segment of a path without retyping the
        /// whole thing. Starts at `buffer.chars().count()` (the end) for a
        /// prefilled field, matching how every text editor starts you off.
        cursor: usize,
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
    /// Confirming a profile-editor slug rename that will *also* rename a
    /// real, already-existing prefix directory (only reached in
    /// `PrefixMode::PerSlug`, and only when that directory actually exists
    /// yet — see `profile_editor::rename_slug`). Kept separate from
    /// `ConfirmDeleteProfile` since the two paths on confirm are entirely
    /// different (rename two directories vs. delete one).
    ConfirmRenameSlug {
        slug: String,
        candidate: String,
        old_prefix_dir: std::path::PathBuf,
        new_prefix_dir: std::path::PathBuf,
    },
    /// The `?` popup — the Help tab's own content shown as an overlay from
    /// any tab, without losing your place there. Scroll position
    /// (`App::help_scroll`) is shared with the Help tab itself, so it's
    /// just "the same help, viewed a second way", not separate state.
    Help,
    /// Confirming Library `p` (launch winetricks against the selected
    /// game's actual prefix) before it happens — not destructive, but
    /// launches an external GUI tool, so a stray keypress shouldn't
    /// trigger it silently.
    ConfirmWinetricks {
        slug: String,
        name: String,
    },
    /// Library `s` on a profile not yet in Steam (`s` on one already in
    /// Steam just reports "already added" instead of opening this — see
    /// `library::prompt_add_to_steam`): a small 3-item navigable list
    /// (Up/Down/Enter, exactly like `ProtonPicker`'s), not a plain y/N
    /// prompt, since there are 3 real outcomes, not 2.
    ConfirmAddToSteam {
        slug: String,
        name: String,
        selected: usize,
    },
}

/// The 3 selectable rows of `Mode::ConfirmAddToSteam`, in order — index into
/// this with `selected` for both the label to render and, on `Enter`, which
/// action to take. `len()` is used for `Up`/`Down` wraparound.
pub const CONFIRM_ADD_TO_STEAM_OPTIONS: [&str; 3] = [
    "Add (no gamescope flag)",
    "Add with gamescope flag",
    "Cancel",
];

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

#[derive(Debug)]
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

/// Which input source produced the most recent keypress the TUI handled —
/// tracked purely so the shortcut legends can show the right captions
/// (`Enter` vs. the gamepad button that was actually pressed). Everything
/// else about handling a gamepad button is identical to a keyboard key: see
/// `gamepad::translate`, which turns a `gilrs` button press into the same
/// `crossterm::event::KeyCode` a keyboard would produce, so `on_key`'s
/// dispatch never needs to know which source it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputKind {
    #[default]
    Keyboard,
    Gamepad,
}

pub struct App {
    pub cfg: Config,
    pub tab: Tab,
    pub mode: Mode,
    pub should_quit: bool,
    pub status: Option<String>,
    /// Updated on every handled keypress, keyboard or gamepad — see
    /// `InputKind`.
    pub input_kind: InputKind,

    pub running: Vec<RunningEntry>,
    pub running_selected: usize,
    /// `Some(text)` while the Running tab's quick-search (`f`) is active —
    /// same `Option<String>`-at-the-`App`-level pattern as `profile_editor`
    /// below, not a `Mode` variant: Up/Down/Enter still need to operate on
    /// the (filtered) list while typing, which an exclusive `Mode` like
    /// `TextInput` doesn't allow for. `running_selected` indexes into
    /// whichever set is currently *displayed* — the filtered subset when
    /// this is `Some`, the full list when `None` — see
    /// `filtered_running_indices`.
    pub running_filter: Option<String>,
    /// `true` while actively typing the filter (capturing every key as
    /// filter text/edits); `false` once Enter "locks" it — the filter
    /// text/view stays exactly as it was, but every other key (kill,
    /// refresh, even switching tabs) works normally again on the filtered
    /// subset. Meaningless when `running_filter` is `None`.
    pub running_filter_editing: bool,

    pub profiles: Vec<(String, Profile)>,
    pub library_selected: usize,
    /// Same as `running_filter`, for the Library tab — see
    /// `filtered_profile_indices`.
    pub library_filter: Option<String>,
    /// Same as `running_filter_editing`, for the Library tab.
    pub library_filter_editing: bool,
    /// Every slug `steam_shortcut::slugs_in_steam()` detected at startup (or
    /// the last `r`efresh/successful `s` add) — drives the Library list's
    /// per-row "S" marker and whether pressing `s` opens the add-confirm
    /// popup or just reports "already added" (see `library::prompt_add_to_steam`).
    pub steam_slugs: HashSet<String>,
    /// Every slug whose `target_path` didn't exist on disk as of the last
    /// `refresh_profiles()` (startup or `r`) — drives the Library list's
    /// per-row "!" marker. Recomputed only on refresh, not every render/tick,
    /// since it's one `Path::exists()` (a stat syscall) per profile — see
    /// `refresh_profiles`'s doc comment for why that's fine even with a
    /// large library.
    pub missing_exes: HashSet<String>,

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

    /// Scroll offset (in lines) into the Help tab's static text — shared
    /// between the Help tab itself and the `?` popup (`Mode::Help`), since
    /// they render the exact same content. Render-time code additionally
    /// clamps this against the actual visible height, so it's fine for this
    /// to just be an unclamped running counter here.
    pub help_scroll: u16,

    /// When the current marquee target (whatever `ui::draw` last computed
    /// a selection/mode signature for) started being displayed — reset by
    /// `sync_marquee` whenever that signature changes, so scrolling always
    /// restarts from the beginning after a 2-second pause. See
    /// `marquee_tick`.
    marquee_reset_at: std::time::Instant,
    /// The signature `sync_marquee` last saw — compared against each
    /// frame's freshly-computed one to detect "the user moved to something
    /// else" (a different row selected, a different popup/field open).
    marquee_last_signature: String,
}

impl App {
    pub fn new(cfg: Config) -> Self {
        let mut app = Self {
            cfg,
            tab: Tab::Running,
            mode: Mode::Normal,
            should_quit: false,
            status: None,
            input_kind: InputKind::default(),
            running: Vec::new(),
            running_selected: 0,
            running_filter: None,
            running_filter_editing: false,
            profiles: Vec::new(),
            library_selected: 0,
            library_filter: None,
            library_filter_editing: false,
            steam_slugs: HashSet::new(),
            missing_exes: HashSet::new(),
            config_selected: 0,
            profile_editor: None,
            profile_field_selected: 0,
            help_scroll: 0,
            marquee_reset_at: std::time::Instant::now(),
            marquee_last_signature: String::new(),
        };
        app.refresh_running();
        app.refresh_profiles();
        app.refresh_steam_status();
        app
    }

    /// Re-scans Steam's `shortcuts.vdf` for slugs already added — best-effort,
    /// see `steam_shortcut::slugs_in_steam`'s own doc comment. Called at
    /// startup and after a successful `s` add, so the "S" marker/`s` key's
    /// behavior stay accurate without needing a full TUI restart.
    pub fn refresh_steam_status(&mut self) {
        self.steam_slugs = crate::steam_shortcut::slugs_in_steam();
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

    /// Reloads the library from disk and, alongside it, which profiles'
    /// `target_path` no longer exists (see `missing_exes`). The extra check
    /// is one `Path::exists()` per profile — a single stat syscall, orders
    /// of magnitude cheaper than the TOML parse `Profile::load_all()`
    /// already does for each one — so it stays negligible even with a large
    /// library; only runs here (startup/`r`), never on every render tick.
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
        self.missing_exes = missing_exe_slugs(&self.profiles);
    }

    /// Indices into `self.profiles` currently displayed — every index, in
    /// order, when `library_filter` is `None`; otherwise only the ones
    /// whose `name` or `target_path` contains the filter text
    /// (case-insensitive substring each) — matching by path lets e.g. `1`
    /// tell apart two profiles that only differ by a leading path segment.
    /// `library_selected` is an index *into this*, not into `self.profiles`
    /// directly — callers that need the real profile look it up via
    /// `indices[library_selected]`.
    pub fn filtered_profile_indices(&self) -> Vec<usize> {
        match &self.library_filter {
            None => (0..self.profiles.len()).collect(),
            Some(filter) => {
                let needle = filter.to_lowercase();
                self.profiles
                    .iter()
                    .enumerate()
                    .filter(|(_, (_, p))| p.matches_query(&needle))
                    .map(|(i, _)| i)
                    .collect()
            }
        }
    }

    /// Same as `filtered_profile_indices`, for `self.running` /
    /// `running_filter` / `running_selected`.
    pub fn filtered_running_indices(&self) -> Vec<usize> {
        match &self.running_filter {
            None => (0..self.running.len()).collect(),
            Some(filter) => {
                let needle = filter.to_lowercase();
                self.running
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.name.to_lowercase().contains(&needle))
                    .map(|(i, _)| i)
                    .collect()
            }
        }
    }

    pub fn next_tab(&mut self) {
        self.clear_filters();
        self.tab = self.tab.next();
    }

    pub fn prev_tab(&mut self) {
        self.clear_filters();
        self.tab = self.tab.prev();
    }

    /// Clears a locked-or-still-typing Library quick-search back to no
    /// filter — used both by `Esc` (once the filter's locked, `Esc` isn't
    /// routed to `edit_filter` anymore, so this covers that case
    /// directly) and by switching tabs.
    pub fn clear_library_filter(&mut self) {
        self.library_filter = None;
        self.library_filter_editing = false;
        self.library_selected = 0;
    }

    /// Same as `clear_library_filter`, for Running.
    pub fn clear_running_filter(&mut self) {
        self.running_filter = None;
        self.running_filter_editing = false;
        self.running_selected = 0;
    }

    /// Both at once — switching tabs resets whichever of Running/Library's
    /// filter might be active, regardless of which tab you're leaving.
    pub fn clear_filters(&mut self) {
        self.clear_library_filter();
        self.clear_running_filter();
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

    /// `0` for the first 2 seconds after the current marquee target was
    /// last reset (`sync_marquee`) — so newly-selected text sits still
    /// long enough to actually read before it starts moving — then
    /// advances roughly once every 200ms. Driven entirely by the TUI's
    /// existing idle redraw cadence (`tui::mod::event_loop` calls
    /// `terminal.draw` every loop iteration, including the ~4/sec ticks
    /// where `event::poll`'s 250ms timeout expires with no key pressed), so
    /// animating a marquee needs no extra thread or timer of its own.
    pub fn marquee_tick(&self) -> usize {
        const DELAY: std::time::Duration = std::time::Duration::from_secs(2);
        let elapsed = self.marquee_reset_at.elapsed();
        let Some(scrolling) = elapsed.checked_sub(DELAY) else {
            return 0;
        };
        (scrolling.as_millis() / 200) as usize
    }

    /// Resets the marquee delay/position whenever `signature` — a cheap
    /// identifier for "what's currently selected/open", computed fresh
    /// every frame by `ui::draw` — differs from what it was last frame.
    /// Called once per frame, before anything reads `marquee_tick`, so a
    /// changed selection (a different row, a newly-opened popup, a
    /// different field within one) always restarts at position 0 with a
    /// fresh 2-second pause, instead of picking up mid-scroll from
    /// whatever the *previous* selection's timer happened to be at.
    pub fn sync_marquee(&mut self, signature: String) {
        if self.marquee_last_signature != signature {
            self.marquee_last_signature = signature;
            self.marquee_reset_at = std::time::Instant::now();
        }
    }
}

/// Slugs of every profile whose `target_path` doesn't currently exist —
/// pulled out of `App::refresh_profiles` as its own function so it's
/// testable against a hand-built profile list, without needing to go
/// through `Profile::load_all()`'s real disk read.
fn missing_exe_slugs(profiles: &[(String, Profile)]) -> HashSet<String> {
    profiles
        .iter()
        .filter(|(_, p)| !Path::new(&p.target_path).exists())
        .map(|(slug, _)| slug.clone())
        .collect()
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
        PrefixMode::Single => PrefixMode::PerSlug,
        PrefixMode::PerSlug => PrefixMode::Single,
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

/// Cycles any `Option<bool>` profile override the same way: inherit → on →
/// off → inherit. Shared by `logging.auto_open`, `gamescope_settings.
/// borderless`, and `gamescope_settings.grab_cursor` — all three are a
/// plain on/off toggle with the same "inherit the global default" state.
pub fn next_optional_bool(v: Option<bool>) -> Option<bool> {
    match v {
        None => Some(true),
        Some(true) => Some(false),
        Some(false) => None,
    }
}

pub fn next_gamescope_setting(m: GamescopeSetting) -> GamescopeSetting {
    match m {
        GamescopeSetting::None => GamescopeSetting::Fullscreen,
        GamescopeSetting::Fullscreen => GamescopeSetting::Maximize,
        GamescopeSetting::Maximize => GamescopeSetting::None,
    }
}

/// Same as `next_profile_record_mode`, for a profile's `gamescope`
/// override — inherit → none (explicitly off, distinct from inheriting) →
/// fullscreen → maximize → inherit.
pub fn next_profile_gamescope_setting(m: Option<GamescopeSetting>) -> Option<GamescopeSetting> {
    match m {
        None => Some(GamescopeSetting::None),
        Some(GamescopeSetting::None) => Some(GamescopeSetting::Fullscreen),
        Some(GamescopeSetting::Fullscreen) => Some(GamescopeSetting::Maximize),
        Some(GamescopeSetting::Maximize) => None,
    }
}

/// Cycles `gamescope_settings.filter` — used identically for the global
/// default and a profile override, since both are already `Option`-wrapped
/// (unlike `GamescopeSetting`, there's no separate "inherit" state to
/// distinguish from "explicitly unset": `None` means "don't pass `-F`" at
/// either level, which is a perfectly sensible thing for the global default
/// to be too).
pub fn next_gamescope_filter(m: Option<GamescopeFilter>) -> Option<GamescopeFilter> {
    match m {
        None => Some(GamescopeFilter::Linear),
        Some(GamescopeFilter::Linear) => Some(GamescopeFilter::Nearest),
        Some(GamescopeFilter::Nearest) => Some(GamescopeFilter::Fsr),
        Some(GamescopeFilter::Fsr) => Some(GamescopeFilter::Nis),
        Some(GamescopeFilter::Nis) => Some(GamescopeFilter::Pixel),
        Some(GamescopeFilter::Pixel) => None,
    }
}

/// Cycles `gamescope_settings.scaler` — same treatment as
/// `next_gamescope_filter` (pairs with it, both `Option`-wrapped at every
/// level).
pub fn next_gamescope_scaler(m: Option<GamescopeScaler>) -> Option<GamescopeScaler> {
    match m {
        None => Some(GamescopeScaler::Auto),
        Some(GamescopeScaler::Auto) => Some(GamescopeScaler::Integer),
        Some(GamescopeScaler::Integer) => Some(GamescopeScaler::Fit),
        Some(GamescopeScaler::Fit) => Some(GamescopeScaler::Fill),
        Some(GamescopeScaler::Fill) => Some(GamescopeScaler::Stretch),
        Some(GamescopeScaler::Stretch) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The 2s-delay-then-advances part of `marquee_tick` depends on real
    // elapsed wall-clock time, so it isn't covered here (a test asserting
    // that would either sleep 2+ real seconds or need an injectable clock,
    // neither of which is worth it for this) — verified manually/via tmux
    // instead (see project NOTES.md). What *is* covered: a fresh reset
    // always starts at tick 0, and re-syncing with the *same* signature
    // must not reset it (a real regression this could otherwise have —
    // e.g. resetting on every redraw regardless of signature — since that
    // would make the delay this exists for pointless).

    #[test]
    fn marquee_tick_is_zero_immediately_after_a_reset() {
        let mut app = App::new(Config::default());
        app.sync_marquee("first".to_string());
        assert_eq!(app.marquee_tick(), 0);
    }

    #[test]
    fn resyncing_with_the_same_signature_does_not_reset_the_timer() {
        let mut app = App::new(Config::default());
        app.sync_marquee("same".to_string());
        let reset_at_first = app.marquee_reset_at;
        app.sync_marquee("same".to_string());
        assert_eq!(
            app.marquee_reset_at, reset_at_first,
            "same signature again shouldn't restart the delay"
        );
    }

    #[test]
    fn resyncing_with_a_different_signature_does_reset_the_timer() {
        let mut app = App::new(Config::default());
        app.sync_marquee("one".to_string());
        let reset_at_first = app.marquee_reset_at;
        std::thread::sleep(std::time::Duration::from_millis(5));
        app.sync_marquee("two".to_string());
        assert!(app.marquee_reset_at > reset_at_first);
    }

    #[test]
    fn missing_exe_slugs_flags_only_paths_that_dont_exist() {
        fn profile(target_path: &str) -> Profile {
            Profile {
                name: "whatever".to_string(),
                target_path: target_path.to_string(),
                title: None,
                last_launched: None,
                args: Vec::new(),
                defaults: Default::default(),
                logging: Default::default(),
                env: Default::default(),
                winedlloverride: Default::default(),
            }
        }
        let present = std::env::temp_dir().join("iprolaunch-test-missing-exe-marker.exe");
        std::fs::write(&present, b"").unwrap();
        let profiles = vec![
            ("present".to_string(), profile(&present.to_string_lossy())),
            (
                "gone".to_string(),
                profile("/definitely/does/not/exist/x.exe"),
            ),
        ];
        let result = missing_exe_slugs(&profiles);
        std::fs::remove_file(&present).unwrap();
        assert!(!result.contains("present"));
        assert!(result.contains("gone"));
    }

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
        assert_eq!(m, PrefixMode::PerSlug);
        m = next_prefix_mode(m);
        assert_eq!(m, PrefixMode::Single);

        let mut r = RecordMode::All;
        r = next_record_mode(r);
        assert_eq!(r, RecordMode::Errors);
        r = next_record_mode(r);
        assert_eq!(r, RecordMode::Off);
        r = next_record_mode(r);
        assert_eq!(r, RecordMode::All);

        let mut g = GamescopeSetting::None;
        g = next_gamescope_setting(g);
        assert_eq!(g, GamescopeSetting::Fullscreen);
        g = next_gamescope_setting(g);
        assert_eq!(g, GamescopeSetting::Maximize);
        g = next_gamescope_setting(g);
        assert_eq!(g, GamescopeSetting::None);
    }

    #[test]
    fn profile_gamescope_setting_cycle_includes_inherit_and_returns_to_it() {
        let mut g = None;
        g = next_profile_gamescope_setting(g);
        assert_eq!(g, Some(GamescopeSetting::None));
        g = next_profile_gamescope_setting(g);
        assert_eq!(g, Some(GamescopeSetting::Fullscreen));
        g = next_profile_gamescope_setting(g);
        assert_eq!(g, Some(GamescopeSetting::Maximize));
        g = next_profile_gamescope_setting(g);
        assert_eq!(g, None);
    }

    #[test]
    fn gamescope_filter_cycle_covers_every_variant_and_returns_to_none() {
        let mut f = None;
        f = next_gamescope_filter(f);
        assert_eq!(f, Some(GamescopeFilter::Linear));
        f = next_gamescope_filter(f);
        assert_eq!(f, Some(GamescopeFilter::Nearest));
        f = next_gamescope_filter(f);
        assert_eq!(f, Some(GamescopeFilter::Fsr));
        f = next_gamescope_filter(f);
        assert_eq!(f, Some(GamescopeFilter::Nis));
        f = next_gamescope_filter(f);
        assert_eq!(f, Some(GamescopeFilter::Pixel));
        f = next_gamescope_filter(f);
        assert_eq!(f, None);
    }

    #[test]
    fn gamescope_scaler_cycle_covers_every_variant_and_returns_to_none() {
        let mut s = None;
        s = next_gamescope_scaler(s);
        assert_eq!(s, Some(GamescopeScaler::Auto));
        s = next_gamescope_scaler(s);
        assert_eq!(s, Some(GamescopeScaler::Integer));
        s = next_gamescope_scaler(s);
        assert_eq!(s, Some(GamescopeScaler::Fit));
        s = next_gamescope_scaler(s);
        assert_eq!(s, Some(GamescopeScaler::Fill));
        s = next_gamescope_scaler(s);
        assert_eq!(s, Some(GamescopeScaler::Stretch));
        s = next_gamescope_scaler(s);
        assert_eq!(s, None);
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
    fn optional_bool_cycle_includes_inherit_and_returns_to_it() {
        // Shared by logging.auto_open, gamescope_settings.borderless, and
        // gamescope_settings.grab_cursor overrides — see `next_optional_bool`.
        let mut v = None;
        v = next_optional_bool(v);
        assert_eq!(v, Some(true));
        v = next_optional_bool(v);
        assert_eq!(v, Some(false));
        v = next_optional_bool(v);
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
