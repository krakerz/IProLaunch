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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PrefixMode {
    Single,
    #[serde(rename = "per-exe")]
    PerExe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogMode {
    Single,
    Each,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordMode {
    All,
    Errors,
    Off,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Defaults {
    pub proton: String,
    pub prefix_mode: PrefixMode,
    pub prefix_path: String,
    pub prefixes_root: String,
    #[serde(rename = "windows-version")]
    pub windows_version: String,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            proton: "system".into(),
            prefix_mode: PrefixMode::Single,
            prefix_path: "~/.local/share/iprolaunch/prefix".into(),
            prefixes_root: "~/.local/share/iprolaunch/prefixes".into(),
            windows_version: "win10".into(),
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    pub defaults: Defaults,
    pub logging: Logging,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
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
        toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
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
        let (pd, pl, penv) = match profile {
            Some(p) => (Some(&p.defaults), Some(&p.logging), Some(&p.env)),
            None => (None, None, None),
        };

        let proton = pd
            .and_then(|pd| pd.proton.clone())
            .unwrap_or_else(|| d.proton.clone());
        let prefix_path = pd
            .and_then(|pd| pd.prefix_path.clone())
            .unwrap_or_else(|| d.prefix_path.clone());
        // windows-version only means anything when each exe owns its own prefix.
        let windows_version = if d.prefix_mode == PrefixMode::PerExe {
            pd.and_then(|pd| pd.windows_version.clone())
                .or_else(|| Some(d.windows_version.clone()))
        } else {
            None
        };

        let keep = pl.and_then(|pl| pl.keep).unwrap_or(l.keep);
        let record = pl.and_then(|pl| pl.record).unwrap_or(l.record);
        // auto_open only means anything when something is actually being recorded.
        let auto_open =
            record != RecordMode::Off && pl.and_then(|pl| pl.auto_open).unwrap_or(l.auto_open);

        let mut env = self.env.clone();
        if let Some(penv) = penv {
            env.extend(penv.clone());
        }

        Effective {
            proton,
            prefix_mode: d.prefix_mode,
            prefix_path,
            prefixes_root: d.prefixes_root.clone(),
            windows_version,
            log_mode: l.mode,
            keep,
            record,
            auto_open,
            env,
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
    /// `None` whenever `prefix_mode != PerExe` — see `Config::effective`.
    pub windows_version: Option<String>,
    pub log_mode: LogMode,
    pub keep: u32,
    pub record: RecordMode,
    pub auto_open: bool,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfileDefaults {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proton: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix_path: Option<String>,
    #[serde(rename = "windows-version", skip_serializing_if = "Option::is_none")]
    pub windows_version: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_launched: Option<time::OffsetDateTime>,
    #[serde(default)]
    pub defaults: ProfileDefaults,
    #[serde(default)]
    pub logging: ProfileLogging,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

impl Profile {
    pub fn profiles_dir() -> Result<PathBuf> {
        Ok(project_dirs()?.config_dir().join("profiles"))
    }

    pub fn load(slug: &str) -> Result<Self> {
        let path = Self::profiles_dir()?.join(slug).join("profile.toml");
        let raw =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
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
    fn windows_version_only_applies_in_per_exe_mode() {
        let mut cfg = Config::default();
        cfg.defaults.prefix_mode = PrefixMode::Single;
        assert_eq!(cfg.effective(None).windows_version, None);

        cfg.defaults.prefix_mode = PrefixMode::PerExe;
        assert_eq!(
            cfg.effective(None).windows_version,
            Some(cfg.defaults.windows_version.clone())
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
            last_launched: None,
            defaults: ProfileDefaults::default(),
            logging: ProfileLogging::default(),
            env: BTreeMap::new(),
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
}
