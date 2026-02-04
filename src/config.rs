use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub stow_dir: PathBuf,
    pub target_dir: PathBuf,
    pub packages: Vec<String>,
    pub secrets_enabled: bool,
}

impl Default for Config {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        Self {
            stow_dir: home.join(".dotfiles"),
            target_dir: home,
            packages: Vec::new(),
            secrets_enabled: true,
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        load_config()
    }

    pub fn save(&self) -> Result<()> {
        save_config(self)
    }
}

pub fn config_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    home.join(".config").join("slinky").join("config.toml")
}

pub fn load_config() -> Result<Config> {
    let path = config_path();

    if !path.exists() {
        let config = Config::default();
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
