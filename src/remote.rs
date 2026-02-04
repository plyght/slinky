use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use thiserror::Error;
use url::Url;

#[derive(Debug, Error)]
pub enum RemoteError {
    #[error("git is not installed or not found in PATH")]
    GitNotFound,

    #[error("failed to execute git command: {0}")]
    GitCommandFailed(String),

    #[error("git command exited with status {status}: {stderr}")]
    GitExitError { status: i32, stderr: String },

    #[error("invalid repository specification: {0}")]
    InvalidRepoSpec(String),

    #[error("failed to create cache directory: {0}")]
    CacheDirectoryError(#[from] std::io::Error),

    #[error("unsupported URL scheme: {0}")]
    UnsupportedScheme(String),

    #[error("failed to parse URL: {0}")]
    UrlParseError(#[from] url::ParseError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provider {
    GitHub,
    GitLab,
    GenericGit,
}

#[derive(Debug, Clone)]
pub struct RepoSpec {
    pub provider: Provider,
    pub owner: String,
    pub repo: String,
    pub branch: Option<String>,
}

impl RepoSpec {
    pub fn to_clone_url(&self) -> String {
        match self.provider {
            Provider::GitHub => format!("https://github.com/{}/{}.git", self.owner, self.repo),
            Provider::GitLab => format!("https://gitlab.com/{}/{}.git", self.owner, self.repo),
            Provider::GenericGit => format!("{}/{}", self.owner, self.repo),
        }
    }

    pub fn cache_key(&self) -> String {
        match self.provider {
            Provider::GitHub => format!("github.com/{}/{}", self.owner, self.repo),
            Provider::GitLab => format!("gitlab.com/{}/{}", self.owner, self.repo),
            Provider::GenericGit => format!("git/{}/{}", self.owner, self.repo)
                .replace("://", "/")
                .replace(":", "/"),
        }
    }
}

pub fn parse_repo_spec(spec: &str) -> Result<RepoSpec, RemoteError> {
    let spec = spec.trim();

    if spec.is_empty() {
        return Err(RemoteError::InvalidRepoSpec(
            "empty repository specification".to_string(),
        ));
    }

    if spec.starts_with("github:") {
        let rest = spec.strip_prefix("github:").unwrap();
        parse_shorthand(rest, Provider::GitHub)
    } else if spec.starts_with("gitlab:") {
        let rest = spec.strip_prefix("gitlab:").unwrap();
        parse_shorthand(rest, Provider::GitLab)
    } else if spec.starts_with("http://")
        || spec.starts_with("https://")
        || spec.starts_with("git@")
        || spec.starts_with("ssh://")
    {
        parse_full_url(spec)
    } else {
        parse_shorthand(spec, Provider::GitHub)
    }
}

fn parse_shorthand(spec: &str, provider: Provider) -> Result<RepoSpec, RemoteError> {
    let parts: Vec<&str> = spec.split('/').collect();

    if parts.len() < 2 {
        return Err(RemoteError::InvalidRepoSpec(format!(
            "shorthand must be in format 'owner/repo', got: {}",
            spec
        )));
    }

    let owner = parts[0].trim();
    let repo_part = parts[1].trim();

    if owner.is_empty() || repo_part.is_empty() {
        return Err(RemoteError::InvalidRepoSpec(
            "owner and repo cannot be empty".to_string(),
        ));
    }

    let (repo, branch) = if let Some(at_pos) = repo_part.find('@') {
        let (r, b) = repo_part.split_at(at_pos);
        (r.to_string(), Some(b[1..].to_string()))
    } else {
        (repo_part.to_string(), None)
    };

    let repo = repo.strip_suffix(".git").unwrap_or(&repo).to_string();

    Ok(RepoSpec {
        provider,
        owner: owner.to_string(),
        repo,
        branch,
    })
}

fn parse_full_url(spec: &str) -> Result<RepoSpec, RemoteError> {
    if spec.starts_with("git@") {
        parse_ssh_url(spec)
    } else {
        let url = Url::parse(spec)?;

        if url.scheme() != "http" && url.scheme() != "https" && url.scheme() != "ssh" {
            return Err(RemoteError::UnsupportedScheme(url.scheme().to_string()));
        }

        let host = url
            .host_str()
            .ok_or_else(|| RemoteError::InvalidRepoSpec("URL has no host".to_string()))?;

        let path = url.path().trim_start_matches('/').trim_end_matches('/');
        let path = path.strip_suffix(".git").unwrap_or(path);

        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() < 2 {
            return Err(RemoteError::InvalidRepoSpec(format!(
                "URL path must contain owner/repo: {}",
                spec
            )));
        }

        let provider = match host {
            "github.com" => Provider::GitHub,
            "gitlab.com" => Provider::GitLab,
            _ => Provider::GenericGit,
        };

        Ok(RepoSpec {
            provider,
            owner: parts[0].to_string(),
            repo: parts[1].to_string(),
            branch: None,
        })
    }
}

fn parse_ssh_url(spec: &str) -> Result<RepoSpec, RemoteError> {
    if !spec.starts_with("git@") {
        return Err(RemoteError::InvalidRepoSpec(format!(
            "invalid SSH URL: {}",
            spec
        )));
    }

    let without_prefix = spec.strip_prefix("git@").unwrap();

    let parts: Vec<&str> = without_prefix.split(':').collect();
    if parts.len() != 2 {
        return Err(RemoteError::InvalidRepoSpec(format!(
            "invalid SSH URL format: {}",
            spec
        )));
    }

    let host = parts[0];
    let path = parts[1].trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);

    let path_parts: Vec<&str> = path.split('/').collect();
    if path_parts.len() < 2 {
        return Err(RemoteError::InvalidRepoSpec(format!(
            "SSH URL path must contain owner/repo: {}",
            spec
        )));
    }

    let provider = match host {
        "github.com" => Provider::GitHub,
        "gitlab.com" => Provider::GitLab,
        _ => Provider::GenericGit,
    };

    Ok(RepoSpec {
        provider,
        owner: path_parts[0].to_string(),
        repo: path_parts[1].to_string(),
        branch: None,
    })
}

pub fn get_repo_cache_path(spec: &RepoSpec) -> PathBuf {
    let base_dirs = directories::BaseDirs::new().expect("failed to determine base directories");
    let data_dir = base_dirs.data_local_dir();

    data_dir.join("slinky").join("repos").join(spec.cache_key())
}

pub fn clone_or_update(spec: &RepoSpec) -> Result<PathBuf, RemoteError> {
    check_git_installed()?;

    let cache_path = get_repo_cache_path(spec);

    if cache_path.exists() {
        update_repo(&cache_path, spec)?;
    } else {
        clone_repo(spec, &cache_path)?;
    }

    Ok(cache_path)
}

fn check_git_installed() -> Result<(), RemoteError> {
    let result = Command::new("git")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    match result {
        Ok(status) if status.success() => Ok(()),
        _ => Err(RemoteError::GitNotFound),
    }
}

fn clone_repo(spec: &RepoSpec, target_path: &Path) -> Result<(), RemoteError> {
    if let Some(parent) = target_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let clone_url = spec.to_clone_url();

    let mut cmd = Command::new("git");
    cmd.arg("clone");

    if let Some(branch) = &spec.branch {
        cmd.arg("--branch").arg(branch);
    }

    cmd.arg("--depth").arg("1");
    cmd.arg(&clone_url);
    cmd.arg(target_path);

    let output = cmd
        .output()
        .map_err(|e| RemoteError::GitCommandFailed(e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(RemoteError::GitExitError {
            status: output.status.code().unwrap_or(-1),
            stderr: stderr.to_string(),
        });
    }

    Ok(())
}

fn update_repo(repo_path: &Path, spec: &RepoSpec) -> Result<(), RemoteError> {
    let mut cmd = Command::new("git");
    cmd.current_dir(repo_path);
    cmd.arg("pull");

    if let Some(branch) = &spec.branch {
        cmd.arg("origin").arg(branch);
    } else {
        cmd.arg("--ff-only");
    }

    let output = cmd
        .output()
        .map_err(|e| RemoteError::GitCommandFailed(e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(RemoteError::GitExitError {
            status: output.status.code().unwrap_or(-1),
            stderr: stderr.to_string(),
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_shorthand_github() {
        let spec = parse_repo_spec("user/repo").unwrap();
        assert_eq!(spec.provider, Provider::GitHub);
        assert_eq!(spec.owner, "user");
        assert_eq!(spec.repo, "repo");
        assert_eq!(spec.branch, None);
    }

    #[test]
    fn test_parse_shorthand_with_branch() {
        let spec = parse_repo_spec("user/repo@main").unwrap();
        assert_eq!(spec.provider, Provider::GitHub);
        assert_eq!(spec.owner, "user");
        assert_eq!(spec.repo, "repo");
        assert_eq!(spec.branch, Some("main".to_string()));
    }

    #[test]
    fn test_parse_github_prefix() {
        let spec = parse_repo_spec("github:user/repo").unwrap();
        assert_eq!(spec.provider, Provider::GitHub);
        assert_eq!(spec.owner, "user");
        assert_eq!(spec.repo, "repo");
    }

    #[test]
    fn test_parse_gitlab_prefix() {
        let spec = parse_repo_spec("gitlab:user/repo").unwrap();
        assert_eq!(spec.provider, Provider::GitLab);
        assert_eq!(spec.owner, "user");
        assert_eq!(spec.repo, "repo");
    }

    #[test]
    fn test_parse_https_url() {
        let spec = parse_repo_spec("https://github.com/user/repo.git").unwrap();
        assert_eq!(spec.provider, Provider::GitHub);
        assert_eq!(spec.owner, "user");
        assert_eq!(spec.repo, "repo");
    }

    #[test]
    fn test_parse_ssh_url() {
        let spec = parse_repo_spec("git@github.com:user/repo.git").unwrap();
        assert_eq!(spec.provider, Provider::GitHub);
        assert_eq!(spec.owner, "user");
        assert_eq!(spec.repo, "repo");
    }

    #[test]
    fn test_invalid_shorthand() {
        let result = parse_repo_spec("invalid");
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_spec() {
        let result = parse_repo_spec("");
        assert!(result.is_err());
    }

    #[test]
    fn test_cache_key_generation() {
        let spec = RepoSpec {
            provider: Provider::GitHub,
            owner: "user".to_string(),
            repo: "repo".to_string(),
            branch: None,
        };
        assert_eq!(spec.cache_key(), "github.com/user/repo");
    }

    #[test]
    fn test_clone_url_generation() {
        let spec = RepoSpec {
            provider: Provider::GitHub,
            owner: "user".to_string(),
            repo: "repo".to_string(),
            branch: None,
        };
        assert_eq!(spec.to_clone_url(), "https://github.com/user/repo.git");
    }
}
