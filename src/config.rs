use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ConflictResolution {
    #[default]
    Backup,
    Skip,
    Overwrite,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoSyncConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub auto_link_new_packages: bool,
    #[serde(default = "default_true")]
    pub auto_git_pull: bool,
    #[serde(default)]
    pub conflict_resolution: ConflictResolution,
    #[serde(default = "default_debounce_ms")]
    pub debounce_ms: u64,
}

fn default_true() -> bool {
    true
}

fn default_debounce_ms() -> u64 {
    1000
}

impl Default for AutoSyncConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_link_new_packages: true,
            auto_git_pull: true,
            conflict_resolution: ConflictResolution::Backup,
            debounce_ms: 1000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub stow_dir: PathBuf,
    pub target_dir: PathBuf,
    pub packages: Vec<String>,
    pub secrets_enabled: bool,
    #[serde(default)]
    pub auto_sync: AutoSyncConfig,
}

impl Default for Config {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        Self {
            stow_dir: home.join(".dotfiles"),
            target_dir: home,
            packages: Vec::new(),
            secrets_enabled: true,
            auto_sync: AutoSyncConfig::default(),
        }
    }
}

impl Config {
    #[allow(dead_code)]
    pub fn load() -> Result<Self> {
        load_config()
    }

    #[allow(dead_code)]
    pub fn save(&self) -> Result<()> {
        save_config(self)
    }
}

pub fn config_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    home.join(".config").join("slinky").join("config.toml")
}

pub fn config_dir() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    home.join(".config").join("slinky")
}

pub fn daemon_pid_path() -> PathBuf {
    config_dir().join("daemon.pid")
}

pub fn daemon_log_path() -> PathBuf {
    config_dir().join("daemon.log")
}

pub fn auto_detect_stow_dir() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let candidates = [
        home.join(".dotfiles"),
        home.join("dotfiles"),
        home.join(".config/dotfiles"),
        home.join("code/dotfiles"),
        home.join("projects/dotfiles"),
    ];

    for candidate in candidates {
        if candidate.exists() && candidate.is_dir() {
            if candidate.join(".git").exists() {
                return Some(candidate);
            }
            if let Ok(entries) = fs::read_dir(&candidate) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        let name = path.file_name().unwrap_or_default().to_string_lossy();
                        if !name.starts_with('.') {
                            return Some(candidate);
                        }
                    }
                }
            }
        }
    }

    None
}

pub fn load_config() -> Result<Config> {
    let path = config_path();

    if !path.exists() {
        let mut config = Config::default();
        if let Some(detected_dir) = auto_detect_stow_dir() {
            config.stow_dir = detected_dir;
        }
        save_config(&config)?;
        return Ok(config);
    }

    let contents = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read config file: {}", path.display()))?;

    let config: Config =
        toml::from_str(&contents).with_context(|| "Failed to parse config file")?;

    Ok(config)
}

pub fn save_config(config: &Config) -> Result<()> {
    let path = config_path();

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config directory: {}", parent.display()))?;
    }

    let contents = toml::to_string_pretty(config).with_context(|| "Failed to serialize config")?;

    fs::write(&path, contents)
        .with_context(|| format!("Failed to write config file: {}", path.display()))?;

    Ok(())
}

mod dirs {
    use std::path::PathBuf;

    pub fn home_dir() -> Option<PathBuf> {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}
