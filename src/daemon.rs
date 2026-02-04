use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, FileIdMap};
use tokio::sync::mpsc;

use crate::config::{daemon_log_path, daemon_pid_path, load_config, Config, ConflictResolution};
use crate::stow::{analyze_package, execute_operations, find_packages, OpType};

#[derive(Debug)]
pub enum DaemonError {
    AlreadyRunning(u32),
    NotRunning,
    Io(std::io::Error),
    Config(String),
    #[allow(dead_code)]
    Watch(String),
}

impl std::fmt::Display for DaemonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DaemonError::AlreadyRunning(pid) => write!(f, "Daemon already running (PID: {})", pid),
            DaemonError::NotRunning => write!(f, "Daemon is not running"),
            DaemonError::Io(e) => write!(f, "IO error: {}", e),
            DaemonError::Config(s) => write!(f, "Config error: {}", s),
            DaemonError::Watch(s) => write!(f, "Watch error: {}", s),
        }
    }
}

impl std::error::Error for DaemonError {}

impl From<std::io::Error> for DaemonError {
    fn from(e: std::io::Error) -> Self {
        DaemonError::Io(e)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonEvent {
    DotfileChanged(PathBuf),
    NewPackage(String),
    GitChanged,
    SymlinkDeleted(PathBuf),
    #[allow(dead_code)]
    Shutdown,
}

pub struct DaemonState {
    #[allow(dead_code)]
    config: Config,
    known_packages: HashSet<String>,
    running: Arc<AtomicBool>,
    log_file: Option<File>,
}

impl DaemonState {
    pub fn new(config: Config) -> Self {
        let known_packages = find_packages(&config.stow_dir)
            .map(|pkgs| pkgs.into_iter().map(|p| p.name).collect())
            .unwrap_or_default();

        Self {
            config,
            known_packages,
            running: Arc::new(AtomicBool::new(true)),
            log_file: None,
        }
    }

    fn log(&mut self, msg: &str) {
        let timestamp = chrono_lite_now();
        let line = format!("[{}] {}\n", timestamp, msg);

        if let Some(ref mut f) = self.log_file {
            let _ = f.write_all(line.as_bytes());
            let _ = f.flush();
        }

        eprintln!("{}", msg);
    }

    fn open_log(&mut self) -> Result<(), DaemonError> {
        let log_path = daemon_log_path();
        if let Some(parent) = log_path.parent() {
            fs::create_dir_all(parent)?;
        }
        self.log_file = Some(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)?,
        );
        Ok(())
    }
}

fn chrono_lite_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    let hours = (secs / 3600) % 24;
    let mins = (secs / 60) % 60;
    let s = secs % 60;
    format!("{:02}:{:02}:{:02}", hours, mins, s)
}

fn should_ignore_path(path: &Path) -> bool {
    let path_str = path.to_string_lossy();

    #[cfg(unix)]
    let ignored_patterns = [
        ".git/",
        ".DS_Store",
        ".swp",
        ".swo",
        "~",
        ".tmp",
        ".temp",
        "4913", // vim temp file
        ".gitignore",
    ];

    #[cfg(windows)]
    let ignored_patterns = [
        ".git/",
        ".git\\",
        ".DS_Store",
        ".swp",
        ".swo",
        "~",
        ".tmp",
        ".temp",
        "4913", // vim temp file
        ".gitignore",
    ];

    for pattern in &ignored_patterns {
        if path_str.contains(pattern) {
            return true;
        }
    }

    if let Some(file_name) = path.file_name() {
        let name = file_name.to_string_lossy();
        if name.starts_with('.') && name.ends_with(".swp") {
            return true;
        }
        if name.starts_with('#') && name.ends_with('#') {
            return true;
        }
    }

    false
}

fn is_git_dir_change(path: &Path, stow_dir: &Path) -> bool {
    let git_dir = stow_dir.join(".git");
    path.starts_with(&git_dir)
}

fn get_package_from_path(path: &Path, stow_dir: &Path) -> Option<String> {
    let relative = path.strip_prefix(stow_dir).ok()?;
    let first_component = relative.components().next()?;
    let name = first_component.as_os_str().to_string_lossy().to_string();

    if name.starts_with('.') {
        return None;
    }

    Some(name)
}

fn backup_file(path: &Path) -> Result<PathBuf, std::io::Error> {
    let backup_path = PathBuf::from(format!("{}.backup", path.display()));
    fs::copy(path, &backup_path)?;
    Ok(backup_path)
}

fn handle_conflict(target: &Path, resolution: ConflictResolution) -> Result<bool, std::io::Error> {
    match resolution {
        ConflictResolution::Backup => {
            if target.exists() && !target.is_symlink() {
                backup_file(target)?;
                fs::remove_file(target)?;
            }
            Ok(true)
        }
        ConflictResolution::Skip => Ok(false),
        ConflictResolution::Overwrite => {
            if target.exists() {
                if target.is_dir() && !target.is_symlink() {
                    fs::remove_dir_all(target)?;
                } else {
                    fs::remove_file(target)?;
                }
            }
            Ok(true)
        }
    }
}

pub fn get_daemon_pid() -> Option<u32> {
    let pid_path = daemon_pid_path();
    if !pid_path.exists() {
        return None;
    }

    let mut contents = String::new();
    File::open(&pid_path)
        .and_then(|mut f| f.read_to_string(&mut contents))
        .ok()?;

    let pid: u32 = contents.trim().parse().ok()?;

    if is_process_running(pid) {
        Some(pid)
    } else {
        let _ = fs::remove_file(&pid_path);
        None
    }
}

fn is_process_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let result = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        matches!(result, Ok(status) if status.success())
    }

    #[cfg(windows)]
    {
        let result = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", pid)])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output();
        match result {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                stdout.contains(&pid.to_string())
            }
            Err(_) => false,
        }
    }
}

fn write_pid_file() -> Result<(), DaemonError> {
    let pid_path = daemon_pid_path();
    if let Some(parent) = pid_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = File::create(&pid_path)?;
    write!(file, "{}", process::id())?;
    Ok(())
}

fn remove_pid_file() {
    let _ = fs::remove_file(daemon_pid_path());
}

pub fn is_daemon_running() -> bool {
    get_daemon_pid().is_some()
}

pub fn stop_daemon() -> Result<(), DaemonError> {
    let pid = get_daemon_pid().ok_or(DaemonError::NotRunning)?;

    #[cfg(unix)]
    {
        let status = Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status()?;
        if !status.success() {
            return Err(DaemonError::Io(std::io::Error::other(
                "Failed to send SIGTERM",
            )));
        }
    }

    #[cfg(windows)]
    {
        let status = Command::new("taskkill")
            .args(["/PID", &pid.to_string()])
            .status()?;
        if !status.success() {
            return Err(DaemonError::Io(std::io::Error::other(
                "Failed to terminate process",
            )));
        }
    }

    std::thread::sleep(Duration::from_millis(500));
    let _ = fs::remove_file(daemon_pid_path());

    Ok(())
}

pub fn start_daemon_background() -> Result<u32, DaemonError> {
    if let Some(pid) = get_daemon_pid() {
        return Err(DaemonError::AlreadyRunning(pid));
    }

    let exe = std::env::current_exe()?;
    let log_path = daemon_log_path();

    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;

    let child = Command::new(&exe)
        .args(["daemon", "run"])
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file.try_clone()?))
        .stderr(Stdio::from(log_file))
        .spawn()?;

    let pid = child.id();

    std::thread::sleep(Duration::from_millis(200));

    Ok(pid)
}

#[tokio::main]
pub async fn run_daemon() -> Result<(), DaemonError> {
    if let Some(pid) = get_daemon_pid() {
        return Err(DaemonError::AlreadyRunning(pid));
    }

    let config = load_config().map_err(|e| DaemonError::Config(e.to_string()))?;

    if !config.auto_sync.enabled {
        return Err(DaemonError::Config(
            "Auto-sync is disabled in config".to_string(),
        ));
    }

    if !config.stow_dir.exists() {
        return Err(DaemonError::Config(format!(
            "Stow directory does not exist: {}",
            config.stow_dir.display()
        )));
    }

    write_pid_file()?;

    let mut state = DaemonState::new(config.clone());
    state.open_log()?;
    state.log("Daemon starting...");
    state.log(&format!("Watching: {}", config.stow_dir.display()));
    state.log(&format!("Target: {}", config.target_dir.display()));

    let running = state.running.clone();
    let running_signal = running.clone();

    #[cfg(unix)]
    {
        tokio::spawn(async move {
            let mut sigterm =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("Failed to register SIGTERM handler");
            let mut sigint =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
                    .expect("Failed to register SIGINT handler");

            tokio::select! {
                _ = sigterm.recv() => {},
                _ = sigint.recv() => {},
            }

            running_signal.store(false, Ordering::SeqCst);
        });
    }

    #[cfg(windows)]
    {
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            running_signal.store(false, Ordering::SeqCst);
        });
    }

    let (tx, mut rx) = mpsc::channel::<DaemonEvent>(100);

    let stow_dir = config.stow_dir.clone();
    let target_dir = config.target_dir.clone();
    let debounce_duration = Duration::from_millis(config.auto_sync.debounce_ms);

    let tx_watcher = tx.clone();
    let stow_dir_watcher = stow_dir.clone();

    let (debouncer_tx, mut debouncer_rx) = mpsc::channel::<DebounceEventResult>(100);

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to build runtime");

        rt.block_on(async move {
            let mut debouncer: Debouncer<RecommendedWatcher, FileIdMap> = new_debouncer(
                debounce_duration,
                None,
                move |result: DebounceEventResult| {
                    let _ = debouncer_tx.blocking_send(result);
                },
            )
            .expect("Failed to create debouncer");

            debouncer
                .watcher()
                .watch(&stow_dir_watcher, RecursiveMode::Recursive)
                .expect("Failed to watch stow directory");

            debouncer
                .cache()
                .add_root(&stow_dir_watcher, RecursiveMode::Recursive);

            while let Some(result) = debouncer_rx.recv().await {
                match result {
                    Ok(events) => {
                        for event in events {
                            for path in &event.paths {
                                if should_ignore_path(path) {
                                    continue;
                                }

                                if is_git_dir_change(path, &stow_dir_watcher) {
                                    let _ = tx_watcher.send(DaemonEvent::GitChanged).await;
                                } else if let Some(pkg) =
                                    get_package_from_path(path, &stow_dir_watcher)
                                {
                                    let _ = tx_watcher
                                        .send(DaemonEvent::DotfileChanged(path.clone()))
                                        .await;
                                    let _ = tx_watcher.send(DaemonEvent::NewPackage(pkg)).await;
                                }
                            }
                        }
                    }
                    Err(errors) => {
                        for error in errors {
                            eprintln!("Watch error: {:?}", error);
                        }
                    }
                }
            }
        });
    });

    let tx_target = tx.clone();
    let target_dir_watcher = target_dir.clone();

    std::thread::spawn(move || {
        let (target_debouncer_tx, target_debouncer_rx) =
            std::sync::mpsc::channel::<DebounceEventResult>();

        let mut target_debouncer: Debouncer<RecommendedWatcher, FileIdMap> = new_debouncer(
            debounce_duration,
            None,
            move |result: DebounceEventResult| {
                let _ = target_debouncer_tx.send(result);
            },
        )
        .expect("Failed to create target debouncer");

        target_debouncer
            .watcher()
            .watch(&target_dir_watcher, RecursiveMode::NonRecursive)
            .expect("Failed to watch target directory");

        while let Ok(result) = target_debouncer_rx.recv() {
            if let Ok(events) = result {
                for event in events {
                    use notify::EventKind;
                    if matches!(event.kind, EventKind::Remove(_)) {
                        for path in &event.paths {
                            if path.is_symlink()
                                || (!path.exists()
                                    && path
                                        .file_name()
                                        .map(|n| !n.to_string_lossy().starts_with('.'))
                                        .unwrap_or(false))
                            {
                                let _ = tx_target
                                    .blocking_send(DaemonEvent::SymlinkDeleted(path.clone()));
                            }
                        }
                    }
                }
            }
        }
    });

    state.log("Daemon started successfully");

    let mut git_pull_pending = false;
    let mut packages_to_relink: HashSet<String> = HashSet::new();

    while running.load(Ordering::SeqCst) {
        tokio::select! {
            Some(event) = rx.recv() => {
                match event {
                    DaemonEvent::DotfileChanged(path) => {
                        state.log(&format!("File changed: {}", path.display()));
                        if let Some(pkg) = get_package_from_path(&path, &stow_dir) {
                            packages_to_relink.insert(pkg);
                        }
                    }
                    DaemonEvent::NewPackage(name) => {
                        if !state.known_packages.contains(&name) {
                            state.log(&format!("New package detected: {}", name));
                            state.known_packages.insert(name.clone());

                            if config.auto_sync.auto_link_new_packages {
                                let pkg_path = stow_dir.join(&name);
                                if pkg_path.is_dir() {
                                    match link_package_auto(&pkg_path, &target_dir, &config) {
                                        Ok(count) => {
                                            state.log(&format!(
                                                "Auto-linked package '{}': {} symlinks",
                                                name, count
                                            ));
                                        }
                                        Err(e) => {
                                            state.log(&format!(
                                                "Failed to auto-link '{}': {}",
                                                name, e
                                            ));
                                        }
                                    }
                                }
                            }
                        } else {
                            packages_to_relink.insert(name);
                        }
                    }
                    DaemonEvent::GitChanged => {
                        if config.auto_sync.auto_git_pull && !git_pull_pending {
                            git_pull_pending = true;
                            state.log("Git change detected, scheduling pull...");
                        }
                    }
                    DaemonEvent::SymlinkDeleted(path) => {
                        state.log(&format!("Symlink deleted: {}", path.display()));
                        for pkg in find_packages(&stow_dir).unwrap_or_default() {
                            packages_to_relink.insert(pkg.name);
                        }
                    }
                    DaemonEvent::Shutdown => {
                        state.log("Shutdown requested");
                        running.store(false, Ordering::SeqCst);
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(2)) => {
                if git_pull_pending {
                    git_pull_pending = false;
                    state.log("Pulling latest changes...");
                    match git_pull(&stow_dir) {
                        Ok(true) => {
                            state.log("Git pull completed with changes, re-linking all packages");
                            for pkg in find_packages(&stow_dir).unwrap_or_default() {
                                packages_to_relink.insert(pkg.name);
                            }
                        }
                        Ok(false) => {
                            state.log("Already up to date");
                        }
                        Err(e) => {
                            state.log(&format!("Git pull failed: {}", e));
                        }
                    }
                }

                if !packages_to_relink.is_empty() {
                    let packages: Vec<String> = packages_to_relink.drain().collect();
                    for pkg_name in packages {
                        let pkg_path = stow_dir.join(&pkg_name);
                        if pkg_path.is_dir() {
                            match link_package_auto(&pkg_path, &target_dir, &config) {
                                Ok(count) if count > 0 => {
                                    state.log(&format!(
                                        "Re-linked package '{}': {} symlinks",
                                        pkg_name, count
                                    ));
                                }
                                Ok(_) => {}
                                Err(e) => {
                                    state.log(&format!(
                                        "Failed to re-link '{}': {}",
                                        pkg_name, e
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    state.log("Daemon shutting down...");
    remove_pid_file();
    state.log("Daemon stopped");

    Ok(())
}

fn link_package_auto(
    package_path: &Path,
    target_dir: &Path,
    config: &Config,
) -> Result<usize, String> {
    let operations = analyze_package(package_path, target_dir).map_err(|e| e.to_string())?;

    for op in &operations {
        if matches!(op.op_type, OpType::Create) && op.target.exists() {
            match handle_conflict(&op.target, config.auto_sync.conflict_resolution) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(e) => {
                    eprintln!(
                        "Conflict resolution failed for {}: {}",
                        op.target.display(),
                        e
                    );
                    continue;
                }
            }
        }
    }

    let results = execute_operations(&operations, false).map_err(|e| e.to_string())?;
    let created = results
        .iter()
        .filter(|r| r.contains("Created symlink"))
        .count();

    Ok(created)
}

fn git_pull(repo_path: &Path) -> Result<bool, String> {
    let git_dir = repo_path.join(".git");
    if !git_dir.exists() {
        return Err("Not a git repository".to_string());
    }

    let output = Command::new("git")
        .current_dir(repo_path)
        .args(["pull", "--ff-only"])
        .output()
        .map_err(|e| e.to_string())?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("Already up to date") {
            return Ok(false);
        }
        return Err(stderr.to_string());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(!stdout.contains("Already up to date"))
}

pub fn daemon_status() -> (bool, Option<u32>, Option<String>) {
    let pid = get_daemon_pid();
    let running = pid.is_some();

    let log_excerpt = if running {
        let log_path = daemon_log_path();
        if log_path.exists() {
            fs::read_to_string(&log_path).ok().map(|content| {
                content
                    .lines()
                    .rev()
                    .take(5)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        } else {
            None
        }
    } else {
        None
    };

    (running, pid, log_excerpt)
}
