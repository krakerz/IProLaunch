use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

pub fn project_dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("", "", "iprolaunch").context("could not determine home directory")
}

pub fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(dirs) = directories::UserDirs::new()
    {
        return dirs.home_dir().join(rest);
    }
    PathBuf::from(path)
}

/// Context message for a TOML parse failure on the global `config.toml`.
/// `toml`'s own error (attached below this by `.with_context`, as "Caused
/// by") already points at the exact line/column with a caret and a
/// diagnosis like `expected \`"\`, \`'\``, but that phrasing assumes TOML
/// familiarity — this adds a plain-English common-fix hint and an escape
/// hatch, found necessary after a real manually-edited `config.toml` (an
/// unquoted string value) broke every subcommand with just the raw parser
/// error (see project NOTES.md, 2026-09-06).
fn config_parse_error_context(path: &std::path::Path) -> String {
    format!(
        "parsing {} — TOML syntax error (see below for the exact line). Common fix: text values need quotes, e.g. `KEY = \"value\"` not `KEY = value`. If you're stuck, delete this file and run `iprolaunch config init` to regenerate the defaults (this loses any manual edits in it)",
        path.display()
    )
}

/// Same as `config_parse_error_context`, but for a per-game `profile.toml` —
/// `config init` only regenerates the global config, so the escape hatch
/// here is just deleting the broken file (iprolaunch recreates a fresh,
/// default profile for that exe the next time it's launched).
fn profile_parse_error_context(path: &std::path::Path) -> String {
    format!(
        "parsing {} — TOML syntax error (see below for the exact line). Common fix: text values need quotes, e.g. `KEY = \"value\"` not `KEY = value`. If you're stuck, delete this file — iprolaunch recreates a fresh profile for that exe the next time you launch it (this loses any manual edits in it)",
        path.display()
    )
}

/// Local wall-clock time, falling back to UTC if the local offset can't be
/// determined (`time`'s detection can fail on some platforms/thread states).
pub fn now_local() -> time::OffsetDateTime {
    let now = time::OffsetDateTime::now_utc();
    now.to_offset(time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC))
}

/// Three-letter month name for `Profile::mark_launched_now`'s
/// `DD-Mon-YYYY` date — `time::Month`'s own `Display` spells out the full
/// name ("September"), not the abbreviation this format wants.
fn month_abbrev(m: time::Month) -> &'static str {
    match m {
        time::Month::January => "Jan",
        time::Month::February => "Feb",
        time::Month::March => "Mar",
        time::Month::April => "Apr",
        time::Month::May => "May",
        time::Month::June => "Jun",
        time::Month::July => "Jul",
        time::Month::August => "Aug",
        time::Month::September => "Sep",
        time::Month::October => "Oct",
        time::Month::November => "Nov",
        time::Month::December => "Dec",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PrefixMode {
    Single,
    /// One prefix per profile, keyed by that profile's own (already-
    /// disambiguated) slug — see `prefix::resolve`. `alias` accepts an
    /// existing `config.toml`'s `"per-exe"` (this variant's old name and
    /// serialized form, before prefixes were keyed by slug instead of a
    /// freshly re-derived-from-the-exe-path value that never got
    /// disambiguated the way a profile's own slug does) so a config
    /// written by an older build keeps loading; re-saving rewrites it to
    /// `"per-slug"`.
    #[serde(rename = "per-slug", alias = "per-exe")]
    PerSlug,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogMode {
    /// All profiles' logs in one shared place — `logging.path` if set,
    /// else `~/.config/iprolaunch/logs/` (see `Config::log_dir`).
    Single,
    /// Each profile's logs live inside that profile's own folder
    /// (`~/.config/iprolaunch/profiles/<slug>/logs/`) — not under a shared
    /// logs root at all, and not affected by `logging.path` (that only
    /// applies to `Single`) — see `logging::LogSession::start`.
    Each,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordMode {
    All,
    Errors,
    Off,
}

/// Whether (and how) to wrap a launch in a nested `gamescope` session —
/// see `launch::GamescopeMode` for what `Fullscreen`/`Maximize` actually
/// pass to `gamescope` itself. Lets `iprolaunch <slug>` remember the same
/// choice `-f`/`-m` would set for one launch, without retyping it —
/// `-f`/`-m` on the command line still win when actually passed (see
/// `launch::run`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum GamescopeSetting {
    #[default]
    None,
    Fullscreen,
    Maximize,
}

/// `-F`/`--filter` (gamescope's upscaler filter) — real gamescope option
/// values, confirmed against the actually-installed `gamescope --help`
/// (3.16.25), not guessed. Always `Option`-wrapped (both globally and per
/// profile) rather than baking in its own "none" variant like
/// `GamescopeSetting` does — there's no sensible global default to fall
/// back to, "don't pass `-F` at all, let gamescope pick" is the only
/// reasonable unset state at any level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GamescopeFilter {
    Linear,
    Nearest,
    Fsr,
    Nis,
    Pixel,
}

/// `-S`/`--scaler` (gamescope's own upscale-strategy option) — real
/// gamescope option values, confirmed against the actually-installed
/// `gamescope --help` (3.16.25), not guessed. Pairs with `filter` (both only
/// matter when the output and nested resolutions differ); same
/// always-`Option`-wrapped treatment for the same reason — no sensible
/// non-`None` global default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GamescopeScaler {
    Auto,
    Integer,
    Fit,
    Fill,
    Stretch,
}

/// Extra `gamescope` launch settings, layered the same way at both the
/// global and per-profile level (each field independently `None` = "don't
/// pass this flag, let gamescope use its own default" — there's no
/// sensible non-`None` global default for a screen resolution, so unlike
/// most of `Defaults`' other fields this can't just always have a real
/// value). `output_*` is gamescope's own `-W`/`-H` (the real display size —
/// gamescope only auto-detects this when it owns the display directly,
/// e.g. bare DRM/KMS or Steam Game Mode's own outer instance; nested inside
/// an existing desktop session it defaults to a small fixed window instead,
/// a real reported bug this exists to fix). `nested_*` is `-w`/`-h` (the
/// game's own internal render resolution — lets it render lower than the
/// output and have gamescope upscale, `filter` picks how).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GamescopeSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nested_width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nested_height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<GamescopeFilter>,
    /// Gamescope's own `-S`/`--scaler` — how it upscales when `filter` is
    /// engaged (nested/output resolutions differ). Config-only, no CLI flag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scaler: Option<GamescopeScaler>,
    /// Gamescope's own `-b`/`--borderless` — merged with the CLI `-b` flag
    /// at launch time (either one turns it on for that launch, see
    /// `launch::run`); `None`/`Some(false)` both mean "don't pass it".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub borderless: Option<bool>,
    /// Gamescope's own `--force-grab-cursor` ("always use relative mouse
    /// mode instead of flipping dependent on cursor visibility") — real
    /// option, confirmed against the actually-installed `gamescope --help`
    /// (3.16.25). Config-only, no CLI flag (distinct from `-g`/`--grab`,
    /// which grabs the *keyboard* — a different, not-yet-exposed option).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grab_cursor: Option<bool>,
    /// Gamescope's own `--adaptive-sync` (VRR) — simple bool toggle, same
    /// pattern as `borderless`/`grab_cursor`. Config-only, no CLI flag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adaptive_sync: Option<bool>,
}

impl GamescopeSettings {
    /// Field-by-field merge: the profile's own value wins when set, else
    /// falls back to the global default — same pattern as every other
    /// per-profile override in `Config::effective`.
    fn merge(self, profile: Self) -> Self {
        Self {
            output_width: profile.output_width.or(self.output_width),
            output_height: profile.output_height.or(self.output_height),
            refresh: profile.refresh.or(self.refresh),
            nested_width: profile.nested_width.or(self.nested_width),
            nested_height: profile.nested_height.or(self.nested_height),
            filter: profile.filter.or(self.filter),
            scaler: profile.scaler.or(self.scaler),
            borderless: profile.borderless.or(self.borderless),
            grab_cursor: profile.grab_cursor.or(self.grab_cursor),
            adaptive_sync: profile.adaptive_sync.or(self.adaptive_sync),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Defaults {
    pub proton: String,
    pub prefix_mode: PrefixMode,
    pub prefix_path: String,
    pub prefixes_root: String,
    #[serde(rename = "windows-version")]
    pub windows_version: String,
    #[serde(default)]
    pub gamescope: GamescopeSetting,
    #[serde(default, skip_serializing_if = "is_default_gamescope_settings")]
    pub gamescope_settings: GamescopeSettings,
    /// A command to run the whole launch *through* — `<wrapper> <what
    /// iprolaunch would otherwise have run directly>`, e.g. `gamemoderun`,
    /// `mangohud`, or a frame-generation layer's own wrapper script.
    /// Distinct from both `env` (values, not a command to exec) and a
    /// profile's `args` (appended *after* the target exe, forwarded to the
    /// exe itself — this instead wraps the *entire* invocation, including
    /// `gamescope` when that's also active). Mirrors what a Steam Launch
    /// Options wrapper prefix + `%command%` already does for a game added
    /// to Steam (see README's "Injecting env vars or a wrapper tool via a
    /// Steam shortcut") — this is the same idea, native to iprolaunch, so
    /// it applies the same way regardless of how the game's launched
    /// (`run`, quick-launch, or a Steam shortcut pointed back at
    /// `iprolaunch <slug>`). Split on whitespace at launch time (see
    /// `launch::wrapper_argv`) — no shell quoting support, matching how
    /// `args` is already parsed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_wrapper: Option<String>,
}

fn is_default_gamescope_settings(s: &GamescopeSettings) -> bool {
    *s == GamescopeSettings::default()
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            proton: "system".into(),
            prefix_mode: PrefixMode::Single,
            prefix_path: "~/.local/share/iprolaunch/prefix".into(),
            prefixes_root: "~/.local/share/iprolaunch/prefixes".into(),
            windows_version: "win10".into(),
            gamescope: GamescopeSetting::None,
            gamescope_settings: GamescopeSettings::default(),
            launch_wrapper: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Logging {
    pub mode: LogMode,
    /// Blank => `<config dir>/logs`. Only consulted when `mode == Single`.
    #[serde(default)]
    pub path: String,
    pub keep: u32,
    pub record: RecordMode,
    pub auto_open: bool,
}

impl Default for Logging {
    fn default() -> Self {
        Self {
            mode: LogMode::Single,
            path: String::new(),
            keep: 3,
            record: RecordMode::Errors,
            auto_open: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameDb {
    /// Days between automatic re-downloads of the umu-database CSV.
    pub update_interval_days: u32,
}

impl Default for GameDb {
    fn default() -> Self {
        Self {
            update_interval_days: 7,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    pub defaults: Defaults,
    pub logging: Logging,
    #[serde(default)]
    pub gamedb: GameDb,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// One line per DLL: `dllname = "n,b"` (Wine's own mode syntax — `n`
    /// native, `b` builtin, comma-separated fallback order, or `d`
    /// disabled). Joined with `;` into a single `WINEDLLOVERRIDES` value at
    /// launch — see `Effective::winedlloverride` / `launch::run`.
    #[serde(default)]
    pub winedlloverride: BTreeMap<String, String>,
}

impl Config {
    fn path() -> Result<PathBuf> {
        Ok(project_dirs()?.config_dir().join("config.toml"))
    }

    /// Loads `config.toml`, writing out the documented defaults on first run.
    pub fn load_or_init() -> Result<Self> {
        let path = Self::path()?;
        if !path.exists() {
            let cfg = Self::default();
            cfg.save()?;
            return Ok(cfg);
        }
        let raw =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&raw).with_context(|| config_parse_error_context(&path))
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let raw = toml::to_string_pretty(self)?;
        fs::write(&path, raw).with_context(|| format!("writing {}", path.display()))
    }

    /// Resolves the log directory, applying the single-mode path override.
    pub fn log_dir(&self) -> Result<PathBuf> {
        if self.logging.mode == LogMode::Single && !self.logging.path.is_empty() {
            return Ok(expand_home(&self.logging.path));
        }
        Ok(project_dirs()?.config_dir().join("logs"))
    }

    /// Merges this global config with an optional profile override.
    pub fn effective(&self, profile: Option<&Profile>) -> Effective {
        let d = &self.defaults;
        let l = &self.logging;
        let (pd, pl, penv, pwdo) = match profile {
            Some(p) => (
                Some(&p.defaults),
                Some(&p.logging),
                Some(&p.env),
                Some(&p.winedlloverride),
            ),
            None => (None, None, None, None),
        };

        // A profile's own proton override only means anything when it owns
        // its own prefix — in `Single` mode every profile shares one
        // prefix, so letting one profile silently launch it with a
        // different Proton version than the rest risks corrupting/
        // confusing that shared prefix's contents (same reasoning as
        // `windows_version` below, which has the identical restriction).
        let proton = if d.prefix_mode == PrefixMode::PerSlug {
            pd.and_then(|pd| pd.proton.clone())
                .unwrap_or_else(|| d.proton.clone())
        } else {
            d.proton.clone()
        };
        let prefix_path = pd
            .and_then(|pd| pd.prefix_path.clone())
            .unwrap_or_else(|| d.prefix_path.clone());
        // windows-version only means anything when each profile owns its own prefix.
        let windows_version = if d.prefix_mode == PrefixMode::PerSlug {
            pd.and_then(|pd| pd.windows_version.clone())
                .or_else(|| Some(d.windows_version.clone()))
        } else {
            None
        };
        let gamescope = pd.and_then(|pd| pd.gamescope).unwrap_or(d.gamescope);
        let gamescope_settings = d
            .gamescope_settings
            .merge(pd.map_or_else(GamescopeSettings::default, |pd| pd.gamescope_settings));
        let launch_wrapper = pd
            .and_then(|pd| pd.launch_wrapper.clone())
            .or_else(|| d.launch_wrapper.clone());

        let keep = pl.and_then(|pl| pl.keep).unwrap_or(l.keep);
        let record = pl.and_then(|pl| pl.record).unwrap_or(l.record);
        // auto_open only means anything when something is actually being recorded.
        let auto_open =
            record != RecordMode::Off && pl.and_then(|pl| pl.auto_open).unwrap_or(l.auto_open);

        let mut env = self.env.clone();
        if let Some(penv) = penv {
            env.extend(penv.clone());
        }

        let mut winedlloverride = self.winedlloverride.clone();
        if let Some(pwdo) = pwdo {
            winedlloverride.extend(pwdo.clone());
        }

        Effective {
            proton,
            prefix_mode: d.prefix_mode,
            prefix_path,
            prefixes_root: d.prefixes_root.clone(),
            windows_version,
            gamescope,
            gamescope_settings,
            launch_wrapper,
            log_mode: l.mode,
            keep,
            record,
            auto_open,
            env,
            winedlloverride,
        }
    }
}

/// Fully-resolved settings for one launch (global defaults + profile overrides applied).
#[derive(Debug, Clone)]
pub struct Effective {
    pub proton: String,
    pub prefix_mode: PrefixMode,
    pub prefix_path: String,
    pub prefixes_root: String,
    /// `None` whenever `prefix_mode != PerSlug` — see `Config::effective`.
    pub windows_version: Option<String>,
    pub gamescope: GamescopeSetting,
    pub gamescope_settings: GamescopeSettings,
    pub launch_wrapper: Option<String>,
    pub log_mode: LogMode,
    pub keep: u32,
    pub record: RecordMode,
    pub auto_open: bool,
    pub env: BTreeMap<String, String>,
    pub winedlloverride: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfileDefaults {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proton: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix_path: Option<String>,
    #[serde(rename = "windows-version", skip_serializing_if = "Option::is_none")]
    pub windows_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gamescope: Option<GamescopeSetting>,
    #[serde(default, skip_serializing_if = "is_default_gamescope_settings")]
    pub gamescope_settings: GamescopeSettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_wrapper: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfileLogging {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<RecordMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_open: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    /// User-facing label shown in the game library — independent of the folder slug.
    pub name: String,
    #[serde(rename = "target-path")]
    pub target_path: String,
    /// The game's real title, for umu-database GAMEID lookup (see
    /// `gamedb::lookup_gameid`) — deliberately separate from `name`, which is
    /// often an auto-generated slug like `eldenring#1`, not a matchable
    /// title. Blank/absent falls back to the exe's file stem, a weaker
    /// signal that rarely matches a proper multi-word title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// `DD-Mon-YYYY, HH:MM:SS`, local time — see `Profile::mark_launched_now`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_launched: Option<String>,
    /// Extra args always forwarded to this exe (e.g. `["--dx11"]`), prepended
    /// to whatever's passed on the command line (`run ... -- extra`, or
    /// quick-launch trailing args) rather than replacing them. A `Vec`, not
    /// one free-text string — an arg containing a space (e.g. a path) has no
    /// ambiguity this way, unlike splitting a string on whitespace would.
    /// No global equivalent (unlike `env`/`winedlloverride`): the same flags
    /// rarely make sense across different games, so this is profile-only.
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub defaults: ProfileDefaults,
    #[serde(default)]
    pub logging: ProfileLogging,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Per-DLL override for this game, merged over the global table (same
    /// per-key precedence as `env`) — see `Config::winedlloverride`.
    #[serde(default)]
    pub winedlloverride: BTreeMap<String, String>,
}

/// The part of a display name before its last literal `#` — safe to split on
/// since `#` is this app's own auto-disambiguator marker, never otherwise
/// used in a name (unlike `-` in a slug, which can't be split the same way).
/// Shared by the TUI profile editor's `Name` field (edits just the base
/// text) and `Profile::display_title` (a Steam shortcut shouldn't show the
/// internal "#N" suffix either).
pub fn name_base(name: &str) -> &str {
    name.rsplit_once('#').map_or(name, |(base, _)| base)
}

impl Profile {
    pub fn profiles_dir() -> Result<PathBuf> {
        Ok(project_dirs()?.config_dir().join("profiles"))
    }

    /// The nicest name available for showing this profile somewhere that
    /// isn't the Library list itself (which wants the real disambiguated
    /// `name`, "#N" included, to tell two same-named profiles apart at a
    /// glance) — `title` (the real game title, when set) if there is one,
    /// else `name` with its "#N" suffix stripped. Used for a Steam
    /// shortcut's own displayed name (see `steam_shortcut.rs`) — showing
    /// "ktsysview#1" there would be a meaningless internal detail to a
    /// user browsing their Steam library.
    pub fn display_title(&self) -> &str {
        self.title
            .as_deref()
            .unwrap_or_else(|| name_base(&self.name))
    }

    /// Sets `last_launched` to now (local time), formatted
    /// `DD-Mon-YYYY, HH:MM:SS`. A plain formatted string rather than TOML's
    /// native datetime type, so the profile file reads in the user's
    /// preferred layout at a glance.
    pub fn mark_launched_now(&mut self) {
        let now = now_local();
        self.last_launched = Some(format!(
            "{:02}-{}-{}, {:02}:{:02}:{:02}",
            now.day(),
            month_abbrev(now.month()),
            now.year(),
            now.hour(),
            now.minute(),
            now.second()
        ));
    }

    /// Whether this profile's name or exe path contains `needle_lower`
    /// (case-insensitive substring each — `needle_lower` must already be
    /// lowercased by the caller, since callers typically check many
    /// profiles against the same one query and shouldn't re-lowercase it
    /// every time). Shared by the TUI's quick-search (`f`) and the CLI's
    /// `library search`, so both use identical matching semantics.
    pub fn matches_query(&self, needle_lower: &str) -> bool {
        self.name.to_lowercase().contains(needle_lower)
            || self.target_path.to_lowercase().contains(needle_lower)
    }

    pub fn load(slug: &str) -> Result<Self> {
        let path = Self::profiles_dir()?.join(slug).join("profile.toml");
        let raw =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&raw).with_context(|| profile_parse_error_context(&path))
    }

    pub fn save(&self, slug: &str) -> Result<()> {
        let dir = Self::profiles_dir()?.join(slug);
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let raw = toml::to_string_pretty(self)?;
        let path = dir.join("profile.toml");
        fs::write(&path, raw).with_context(|| format!("writing {}", path.display()))
    }

    /// Lists `(slug, Profile)` for every profile on disk, skipping unreadable entries.
    pub fn load_all() -> Result<Vec<(String, Profile)>> {
        let dir = Self::profiles_dir()?;
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let slug = entry.file_name().to_string_lossy().into_owned();
            if let Ok(profile) = Self::load(&slug) {
                out.push((slug, profile));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_base_splits_on_the_last_hash() {
        assert_eq!(name_base("game#1"), "game");
        assert_eq!(name_base("no-hash-here"), "no-hash-here");
        assert_eq!(name_base("weird#name#2"), "weird#name"); // last '#' only
    }

    #[test]
    fn display_title_prefers_title_then_falls_back_to_name_base() {
        let mut profile = Profile {
            name: "game#1".into(),
            target_path: "/tmp/game.exe".into(),
            title: None,
            last_launched: None,
            defaults: ProfileDefaults::default(),
            logging: ProfileLogging::default(),
            env: BTreeMap::new(),
            winedlloverride: BTreeMap::new(),
            args: Vec::new(),
        };
        assert_eq!(profile.display_title(), "game");

        profile.title = Some("A Real Game Title".to_string());
        assert_eq!(profile.display_title(), "A Real Game Title");
    }

    #[test]
    fn matches_query_checks_both_name_and_target_path() {
        // Mixed-case name/path on purpose: proves the haystack side is
        // lowercased internally, regardless of the profile's own real
        // casing — `needle_lower` itself is the caller's job to lowercase
        // (see the doc comment), not tested here.
        let profile = Profile {
            name: "EldenRing#1".into(),
            target_path: "/a/b/c/Downloads/EldenRing.exe".into(),
            title: None,
            last_launched: None,
            defaults: ProfileDefaults::default(),
            logging: ProfileLogging::default(),
            env: BTreeMap::new(),
            winedlloverride: BTreeMap::new(),
            args: Vec::new(),
        };
        assert!(profile.matches_query("elden")); // name
        assert!(profile.matches_query("downloads")); // path only, not the name
        assert!(!profile.matches_query("nonexistent"));
    }

    #[test]
    fn windows_version_only_applies_in_per_slug_mode() {
        let mut cfg = Config::default();
        cfg.defaults.prefix_mode = PrefixMode::Single;
        assert_eq!(cfg.effective(None).windows_version, None);

        cfg.defaults.prefix_mode = PrefixMode::PerSlug;
        assert_eq!(
            cfg.effective(None).windows_version,
            Some(cfg.defaults.windows_version.clone())
        );
    }

    #[test]
    fn profile_proton_override_is_ignored_outside_per_slug_mode() {
        let mut cfg = Config::default();
        cfg.defaults.proton = "system".into();
        let mut profile = Profile {
            name: "game#1".into(),
            target_path: "/tmp/game.exe".into(),
            title: None,
            last_launched: None,
            defaults: ProfileDefaults::default(),
            logging: ProfileLogging::default(),
            env: BTreeMap::new(),
            winedlloverride: BTreeMap::new(),
            args: Vec::new(),
        };
        profile.defaults.proton = Some("GE-Proton10-34".into());

        cfg.defaults.prefix_mode = PrefixMode::Single;
        assert_eq!(cfg.effective(Some(&profile)).proton, "system");

        cfg.defaults.prefix_mode = PrefixMode::PerSlug;
        assert_eq!(cfg.effective(Some(&profile)).proton, "GE-Proton10-34");
    }

    #[test]
    fn gamescope_defaults_to_none_and_a_profile_override_wins() {
        let cfg = Config::default();
        assert_eq!(cfg.effective(None).gamescope, GamescopeSetting::None);

        let mut profile = Profile {
            name: "game#1".into(),
            target_path: "/tmp/game.exe".into(),
            title: None,
            last_launched: None,
            defaults: ProfileDefaults::default(),
            logging: ProfileLogging::default(),
            env: BTreeMap::new(),
            winedlloverride: BTreeMap::new(),
            args: Vec::new(),
        };
        profile.defaults.gamescope = Some(GamescopeSetting::Fullscreen);
        assert_eq!(
            cfg.effective(Some(&profile)).gamescope,
            GamescopeSetting::Fullscreen
        );
    }

    #[test]
    fn launch_wrapper_profile_override_wins_over_global_default() {
        let mut cfg = Config::default();
        assert_eq!(cfg.effective(None).launch_wrapper, None);

        cfg.defaults.launch_wrapper = Some("gamemoderun".to_string());
        assert_eq!(
            cfg.effective(None).launch_wrapper,
            Some("gamemoderun".to_string())
        );

        let mut profile = Profile {
            name: "game#1".into(),
            target_path: "/tmp/game.exe".into(),
            title: None,
            last_launched: None,
            defaults: ProfileDefaults::default(),
            logging: ProfileLogging::default(),
            env: BTreeMap::new(),
            winedlloverride: BTreeMap::new(),
            args: Vec::new(),
        };
        assert_eq!(
            cfg.effective(Some(&profile)).launch_wrapper,
            Some("gamemoderun".to_string())
        );

        profile.defaults.launch_wrapper = Some("~/lsfg".to_string());
        assert_eq!(
            cfg.effective(Some(&profile)).launch_wrapper,
            Some("~/lsfg".to_string())
        );
    }

    #[test]
    fn gamescope_settings_merge_per_field_profile_wins_when_set() {
        let mut cfg = Config::default();
        cfg.defaults.gamescope_settings.output_width = Some(1920);
        cfg.defaults.gamescope_settings.output_height = Some(1080);
        cfg.defaults.gamescope_settings.filter = Some(GamescopeFilter::Fsr);

        let mut profile = Profile {
            name: "game#1".into(),
            target_path: "/tmp/game.exe".into(),
            title: None,
            last_launched: None,
            defaults: ProfileDefaults::default(),
            logging: ProfileLogging::default(),
            env: BTreeMap::new(),
            winedlloverride: BTreeMap::new(),
            args: Vec::new(),
        };
        // Nothing overridden yet — every field falls back to the global default.
        let effective = cfg.effective(Some(&profile));
        assert_eq!(effective.gamescope_settings.output_width, Some(1920));
        assert_eq!(effective.gamescope_settings.output_height, Some(1080));
        assert_eq!(
            effective.gamescope_settings.filter,
            Some(GamescopeFilter::Fsr)
        );
        assert_eq!(effective.gamescope_settings.refresh, None);

        // Override just one field — the rest still fall back to the global default.
        profile.defaults.gamescope_settings.output_width = Some(1280);
        let effective = cfg.effective(Some(&profile));
        assert_eq!(effective.gamescope_settings.output_width, Some(1280));
        assert_eq!(effective.gamescope_settings.output_height, Some(1080));

        cfg.defaults.gamescope_settings.borderless = Some(true);
        cfg.defaults.gamescope_settings.grab_cursor = Some(true);
        assert_eq!(
            cfg.effective(Some(&profile)).gamescope_settings.borderless,
            Some(true)
        );
        profile.defaults.gamescope_settings.grab_cursor = Some(false);
        assert_eq!(
            cfg.effective(Some(&profile)).gamescope_settings.grab_cursor,
            Some(false)
        );

        cfg.defaults.gamescope_settings.scaler = Some(GamescopeScaler::Fit);
        cfg.defaults.gamescope_settings.adaptive_sync = Some(true);
        assert_eq!(
            cfg.effective(Some(&profile)).gamescope_settings.scaler,
            Some(GamescopeScaler::Fit)
        );
        profile.defaults.gamescope_settings.scaler = Some(GamescopeScaler::Stretch);
        profile.defaults.gamescope_settings.adaptive_sync = Some(false);
        assert_eq!(
            cfg.effective(Some(&profile)).gamescope_settings.scaler,
            Some(GamescopeScaler::Stretch)
        );
        assert_eq!(
            cfg.effective(Some(&profile))
                .gamescope_settings
                .adaptive_sync,
            Some(false)
        );
    }

    #[test]
    fn per_slug_serializes_as_per_slug_but_still_reads_the_old_per_exe_value() {
        #[derive(Serialize, Deserialize)]
        struct Wrapper {
            mode: PrefixMode,
        }
        let old: Wrapper = toml::from_str("mode = \"per-exe\"").unwrap();
        assert_eq!(old.mode, PrefixMode::PerSlug);
        let new: Wrapper = toml::from_str("mode = \"per-slug\"").unwrap();
        assert_eq!(new.mode, PrefixMode::PerSlug);
        assert_eq!(
            toml::to_string(&Wrapper {
                mode: PrefixMode::PerSlug
            })
            .unwrap()
            .trim(),
            "mode = \"per-slug\""
        );
    }

    #[test]
    fn auto_open_forced_off_when_record_is_off() {
        let mut cfg = Config::default();
        cfg.logging.record = RecordMode::Off;
        cfg.logging.auto_open = true;
        assert!(!cfg.effective(None).auto_open);
    }

    #[test]
    fn profile_env_overrides_global_on_collision() {
        let mut cfg = Config::default();
        cfg.env.insert("DXVK_HUD".into(), "0".into());
        let mut profile = Profile {
            name: "game#1".into(),
            target_path: "/tmp/game.exe".into(),
            title: None,
            last_launched: None,
            defaults: ProfileDefaults::default(),
            logging: ProfileLogging::default(),
            env: BTreeMap::new(),
            winedlloverride: BTreeMap::new(),
            args: Vec::new(),
        };
        profile.env.insert("DXVK_HUD".into(), "fps".into());
        assert_eq!(
            cfg.effective(Some(&profile))
                .env
                .get("DXVK_HUD")
                .map(String::as_str),
            Some("fps")
        );
    }

    #[test]
    fn mark_launched_now_formats_dd_mon_yyyy_hh_mm_ss() {
        let mut profile = Profile {
            name: "game#1".into(),
            target_path: "/tmp/game.exe".into(),
            title: None,
            last_launched: None,
            defaults: ProfileDefaults::default(),
            logging: ProfileLogging::default(),
            env: BTreeMap::new(),
            winedlloverride: BTreeMap::new(),
            args: Vec::new(),
        };
        profile.mark_launched_now();
        let stamp = profile.last_launched.expect("mark_launched_now sets it");

        let (date, time) = stamp.split_once(", ").expect("`DD-Mon-YYYY, HH:MM:SS`");
        let date_parts: Vec<&str> = date.split('-').collect();
        let time_parts: Vec<&str> = time.split(':').collect();
        assert_eq!(date_parts.len(), 3, "date should be DD-Mon-YYYY: {stamp}");
        assert_eq!(time_parts.len(), 3, "time should be HH:MM:SS: {stamp}");
        assert_eq!(date_parts[0].len(), 2, "day should be zero-padded: {stamp}");
        assert_eq!(
            date_parts[1].len(),
            3,
            "month should be a 3-letter abbreviation: {stamp}"
        );
        assert_eq!(date_parts[2].len(), 4, "year should be 4 digits: {stamp}");
        assert!(
            time_parts.iter().all(|p| p.len() == 2),
            "HH/MM/SS should be zero-padded: {stamp}"
        );
    }
}
