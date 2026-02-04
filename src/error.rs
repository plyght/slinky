use std::io;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SlinkyError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Stow operation failed: {0}")]
    Stow(String),

    #[error("Remote repository error: {0}")]
    Remote(String),

    #[error("Secrets management error: {0}")]
    Secrets(String),

    #[error("Invalid repository specification: {0}")]
    InvalidRepoSpec(String),

    #[error("Package not found: {0}")]
    PackageNotFound(String),

    #[error("Target directory not found: {0}")]
    TargetNotFound(String),

    #[error("Conflict detected: {0}")]
    Conflict(String),

    #[error("Git error: {0}")]
    Git(String),

    #[error("Encryption error: {0}")]
    Encryption(String),

    #[error("Decryption error: {0}")]
    Decryption(String),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, SlinkyError>;
