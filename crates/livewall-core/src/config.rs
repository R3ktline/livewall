use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::types::{HwDecPreference, ScaleMode};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Default wallpaper applied to outputs without a specific entry.
    pub default_path: Option<PathBuf>,
    pub default_mode: ScaleMode,
    pub hwdec: HwDecPreference,
    /// Mute video audio (always recommended for wallpapers).
    pub mute: bool,
    /// Fail instead of falling back to software decode when hwdec is required.
    pub efficiency_mode: bool,
    /// Cap decode FPS (0 = match source / output refresh).
    pub max_fps: u32,
    pub pause: PauseRules,
    pub outputs: HashMap<String, OutputConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_path: None,
            default_mode: ScaleMode::Fill,
            hwdec: HwDecPreference::Auto,
            mute: true,
            efficiency_mode: false,
            max_fps: 0,
            pause: PauseRules::default(),
            outputs: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PauseRules {
    pub on_dpms: bool,
    pub on_fullscreen: bool,
    /// Halve FPS when on battery (best-effort via `/sys/class/power_supply`).
    pub half_fps_on_battery: bool,
}

impl Default for PauseRules {
    fn default() -> Self {
        Self {
            on_dpms: true,
            on_fullscreen: true,
            half_fps_on_battery: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct OutputConfig {
    pub path: Option<PathBuf>,
    pub mode: Option<ScaleMode>,
}

pub fn config_dir() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("dev", "livewall", "livewall")
        .context("could not resolve config directory")?;
    Ok(dirs.config_dir().to_path_buf())
}

pub fn config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}

pub fn load_config() -> Result<Config> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(Config::default());
    }
    let text = fs::read_to_string(&path)
        .with_context(|| format!("reading config {}", path.display()))?;
    let cfg: Config = toml::from_str(&text)
        .with_context(|| format!("parsing config {}", path.display()))?;
    Ok(cfg)
}

pub fn save_config(cfg: &Config) -> Result<()> {
    let dir = config_dir()?;
    fs::create_dir_all(&dir)?;
    let path = dir.join("config.toml");
    let text = toml::to_string_pretty(cfg)?;
    fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}
