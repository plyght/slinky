pub mod cli;
pub mod config;
pub mod daemon;
pub mod error;
pub mod remote;
pub mod secrets;
pub mod service;
pub mod stow;

pub use config::{config_path, load_config, save_config, Config};
pub use error::{Result, SlinkyError};
