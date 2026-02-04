use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct StowPackage {
    pub name: String,
    #[allow(dead_code)]
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct SymlinkOp {
    pub source: PathBuf,
    pub target: PathBuf,
    pub op_type: OpType,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OpType {
    Create,
    #[allow(dead_code)]
    Remove,
    Skip(String),
}

#[derive(Debug)]
pub enum StowError {
    Io(io::Error),
    InvalidPackage(String),
    ConflictDetected(String),
    InvalidPath(String),
}

impl std::fmt::Display for StowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StowError::Io(e) => write!(f, "IO error: {}", e),
            StowError::InvalidPackage(s) => write!(f, "Invalid package: {}", s),
            StowError::ConflictDetected(s) => write!(f, "Conflict detected: {}", s),
            StowError::InvalidPath(s) => write!(f, "Invalid path: {}", s),
        }
    }
}

impl std::error::Error for StowError {}

impl From<io::Error> for StowError {
    fn from(error: io::Error) -> Self {
        StowError::Io(error)
    }
}

pub fn find_packages(stow_dir: &Path) -> Result<Vec<StowPackage>, StowError> {
    if !stow_dir.exists() {
        return Err(StowError::InvalidPath(format!(
            "Stow directory does not exist: {}",
            stow_dir.display()
        )));
    }

    if !stow_dir.is_dir() {
        return Err(StowError::InvalidPath(format!(
            "Stow path is not a directory: {}",
            stow_dir.display()
        )));
    }

    let mut packages = Vec::new();

    for entry in fs::read_dir(stow_dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            if let Some(name) = path.file_name() {
                let name_str = name.to_string_lossy().to_string();
                if !name_str.starts_with('.') {
                    packages.push(StowPackage {
                        name: name_str,
                        path,
                    });
                }
            }
        }
    }

    Ok(packages)
}

pub fn analyze_package(
    package_path: &Path,
    target_dir: &Path,
) -> Result<Vec<SymlinkOp>, StowError> {
    if !package_path.exists() {
        return Err(StowError::InvalidPackage(format!(
            "Package path does not exist: {}",
            package_path.display()
        )));
    }

    if !package_path.is_dir() {
        return Err(StowError::InvalidPackage(format!(
            "Package path is not a directory: {}",
            package_path.display()
        )));
    }

    let ignore_patterns = load_stow_ignore(package_path)?;
    let mut operations = Vec::new();

    scan_package_recursive(
        package_path,
        package_path,
        target_dir,
        &ignore_patterns,
        &mut operations,
    )?;

    Ok(operations)
}

pub fn execute_operations(ops: &[SymlinkOp], dry_run: bool) -> Result<Vec<String>, StowError> {
    let mut results = Vec::new();

    for op in ops {
        match &op.op_type {
            OpType::Create => {
                let result = if dry_run {
                    format!(
                        "[DRY-RUN] Would create symlink: {} -> {}",
                        op.target.display(),
                        op.source.display()
                    )
                } else {
                    if let Some(parent) = op.target.parent() {
                        if !parent.exists() {
                            fs::create_dir_all(parent)?;
                        }
                    }

                    #[cfg(unix)]
                    std::os::unix::fs::symlink(&op.source, &op.target)?;

                    #[cfg(windows)]
                    {
                        if op.source.is_dir() {
                            std::os::windows::fs::symlink_dir(&op.source, &op.target)?;
                        } else {
                            std::os::windows::fs::symlink_file(&op.source, &op.target)?;
                        }
                    }

                    format!(
                        "Created symlink: {} -> {}",
                        op.target.display(),
                        op.source.display()
                    )
                };
                results.push(result);
            }
            OpType::Remove => {
                let result = if dry_run {
                    format!("[DRY-RUN] Would remove symlink: {}", op.target.display())
                } else if op.target.is_symlink() {
                    fs::remove_file(&op.target)?;
                    format!("Removed symlink: {}", op.target.display())
                } else {
                    format!("Skipped non-symlink: {}", op.target.display())
                };
                results.push(result);
            }
            OpType::Skip(reason) => {
                results.push(format!("Skipped {}: {}", op.target.display(), reason));
            }
        }
    }

    Ok(results)
}

fn scan_package_recursive(
    package_root: &Path,
    current_path: &Path,
    target_dir: &Path,
    ignore_patterns: &HashSet<String>,
    operations: &mut Vec<SymlinkOp>,
) -> Result<(), StowError> {
    for entry in fs::read_dir(current_path)? {
        let entry = entry?;
        let path = entry.path();
        let file_name = entry.file_name();
        let file_name_str = file_name.to_string_lossy();

        if file_name_str == ".stow-local-ignore" {
            continue;
        }

        let relative_path = path.strip_prefix(package_root).map_err(|_| {
            StowError::InvalidPath(format!(
                "Failed to compute relative path for {}",
                path.display()
            ))
        })?;

        if is_ignored(relative_path, ignore_patterns) {
            operations.push(SymlinkOp {
                source: path.clone(),
                target: target_dir.join(relative_path),
                op_type: OpType::Skip("Ignored by .stow-local-ignore".to_string()),
            });
            continue;
        }

        let target_path = target_dir.join(relative_path);

        if path.is_dir() {
            scan_package_recursive(package_root, &path, target_dir, ignore_patterns, operations)?;
        } else {
            let op_type = determine_operation(&path, &target_path)?;
            operations.push(SymlinkOp {
                source: path,
                target: target_path,
                op_type,
            });
        }
    }

    Ok(())
}

fn determine_operation(source: &Path, target: &Path) -> Result<OpType, StowError> {
    if !target.exists() {
        return Ok(OpType::Create);
    }

    if target.is_symlink() {
        let target_link = fs::read_link(target)?;
        if target_link == source {
            return Ok(OpType::Skip("Already linked correctly".to_string()));
        } else {
            return Err(StowError::ConflictDetected(format!(
                "Target {} is a symlink to {} but should point to {}",
                target.display(),
                target_link.display(),
                source.display()
            )));
        }
    }

    Err(StowError::ConflictDetected(format!(
        "Target {} exists and is not a symlink",
        target.display()
    )))
}

fn load_stow_ignore(package_path: &Path) -> Result<HashSet<String>, StowError> {
    let ignore_file = package_path.join(".stow-local-ignore");
    let mut patterns = HashSet::new();

    if ignore_file.exists() {
        let content = fs::read_to_string(&ignore_file)?;
        for line in content.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() && !trimmed.starts_with('#') {
                patterns.insert(trimmed.to_string());
            }
        }
    }

    Ok(patterns)
}

fn is_ignored(path: &Path, patterns: &HashSet<String>) -> bool {
    let path_str = path.to_string_lossy();

    for pattern in patterns {
        if pattern.contains('*') {
            if glob_match(&path_str, pattern) {
                return true;
            }
        } else if path_str.contains(pattern.as_str()) {
            return true;
        }

        if let Some(file_name) = path.file_name() {
            let file_name_str = file_name.to_string_lossy();
            if file_name_str == pattern.as_str() {
                return true;
            }
        }
    }

    false
}

fn glob_match(text: &str, pattern: &str) -> bool {
    let pattern_parts: Vec<&str> = pattern.split('*').collect();

    if pattern_parts.is_empty() {
        return text.is_empty();
    }

    let mut text_pos = 0;

    for (i, part) in pattern_parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }

        if i == 0 {
            if !text[text_pos..].starts_with(part) {
                return false;
            }
            text_pos += part.len();
        } else if i == pattern_parts.len() - 1 {
            if !text[text_pos..].ends_with(part) {
                return false;
            }
            return true;
        } else if let Some(pos) = text[text_pos..].find(part) {
            text_pos += pos + part.len();
        } else {
            return false;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn setup_test_package(base: &Path, package_name: &str) -> PathBuf {
        let package_path = base.join(package_name);
        fs::create_dir_all(&package_path).unwrap();
        package_path
    }

    fn create_test_file(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn test_glob_match() {
        assert!(glob_match("test.txt", "*.txt"));
        assert!(glob_match("foo.bar.txt", "*.txt"));
        assert!(glob_match("test.txt", "test.*"));
        assert!(glob_match("test.txt", "*"));
        assert!(!glob_match("test.md", "*.txt"));
    }

    #[test]
    fn test_find_packages() {
        let temp_dir = std::env::temp_dir().join("slinky_test_find");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        setup_test_package(&temp_dir, "package1");
        setup_test_package(&temp_dir, "package2");
        fs::create_dir_all(temp_dir.join(".hidden")).unwrap();

        let packages = find_packages(&temp_dir).unwrap();
        assert_eq!(packages.len(), 2);

        let names: Vec<&str> = packages.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"package1"));
        assert!(names.contains(&"package2"));

        fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn test_analyze_package_simple() {
        let temp_dir = std::env::temp_dir().join("slinky_test_analyze");
        let _ = fs::remove_dir_all(&temp_dir);

        let package_path = setup_test_package(&temp_dir, "testpkg");
        create_test_file(&package_path.join(".config").join("test.conf"), "config");

        let target_dir = temp_dir.join("target");
        fs::create_dir_all(&target_dir).unwrap();

        let ops = analyze_package(&package_path, &target_dir).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0].op_type, OpType::Create));

        fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn test_stow_ignore() {
        let temp_dir = std::env::temp_dir().join("slinky_test_ignore");
        let _ = fs::remove_dir_all(&temp_dir);

        let package_path = setup_test_package(&temp_dir, "testpkg");
        create_test_file(&package_path.join(".config").join("keep.conf"), "keep");
        create_test_file(&package_path.join(".config").join("ignore.tmp"), "ignore");
        create_test_file(&package_path.join(".stow-local-ignore"), "*.tmp");

        let target_dir = temp_dir.join("target");
        fs::create_dir_all(&target_dir).unwrap();

        let ops = analyze_package(&package_path, &target_dir).unwrap();

        let create_ops: Vec<_> = ops
            .iter()
            .filter(|op| matches!(op.op_type, OpType::Create))
            .collect();
        let skip_ops: Vec<_> = ops
            .iter()
            .filter(|op| matches!(op.op_type, OpType::Skip(_)))
            .collect();

        assert_eq!(create_ops.len(), 1);
        assert_eq!(skip_ops.len(), 1);

        fs::remove_dir_all(&temp_dir).unwrap();
    }
}
