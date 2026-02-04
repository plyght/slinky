use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug)]
pub enum ServiceError {
    Io(std::io::Error),
    UnsupportedPlatform,
    AlreadyInstalled,
    NotInstalled,
    CommandFailed(String),
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceError::Io(e) => write!(f, "IO error: {}", e),
            ServiceError::UnsupportedPlatform => write!(f, "Unsupported platform"),
            ServiceError::AlreadyInstalled => write!(f, "Service already installed"),
            ServiceError::NotInstalled => write!(f, "Service not installed"),
            ServiceError::CommandFailed(s) => write!(f, "Command failed: {}", s),
        }
    }
}

impl std::error::Error for ServiceError {}

impl From<std::io::Error> for ServiceError {
    fn from(e: std::io::Error) -> Self {
        ServiceError::Io(e)
    }
}

const LAUNCHD_LABEL: &str = "com.slinky.daemon";
const SYSTEMD_SERVICE_NAME: &str = "slinky";

fn get_launchd_plist_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
    PathBuf::from(home)
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{}.plist", LAUNCHD_LABEL))
}

fn get_systemd_service_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
    PathBuf::from(home)
        .join(".config")
        .join("systemd")
        .join("user")
        .join(format!("{}.service", SYSTEMD_SERVICE_NAME))
}

fn generate_launchd_plist() -> Result<String, ServiceError> {
    let exe_path = std::env::current_exe()?;
    let exe_str = exe_path.to_string_lossy();

    let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
    let log_path = PathBuf::from(&home)
        .join(".config")
        .join("slinky")
        .join("daemon.log");
    let err_path = PathBuf::from(&home)
        .join(".config")
        .join("slinky")
        .join("daemon.err");

    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
        <string>daemon</string>
        <string>run</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>StandardOutPath</key>
    <string>{}</string>
    <key>StandardErrorPath</key>
    <string>{}</string>
    <key>ProcessType</key>
    <string>Background</string>
    <key>Nice</key>
    <integer>10</integer>
    <key>ThrottleInterval</key>
    <integer>30</integer>
</dict>
</plist>
"#,
        LAUNCHD_LABEL,
        exe_str,
        log_path.display(),
        err_path.display()
    );

    Ok(plist)
}

fn generate_systemd_service() -> Result<String, ServiceError> {
    let exe_path = std::env::current_exe()?;
    let exe_str = exe_path.to_string_lossy();

    let service = format!(
        r#"[Unit]
Description=Slinky Dotfiles Sync Daemon
After=network.target

[Service]
Type=simple
ExecStart={} daemon run
Restart=on-failure
RestartSec=10

[Install]
WantedBy=default.target
"#,
        exe_str
    );

    Ok(service)
}

pub fn is_service_installed() -> bool {
    #[cfg(target_os = "macos")]
    {
        get_launchd_plist_path().exists()
    }

    #[cfg(target_os = "linux")]
    {
        get_systemd_service_path().exists()
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        false
    }
}

pub fn get_service_status() -> Result<(bool, bool), ServiceError> {
    let installed = is_service_installed();

    #[cfg(target_os = "macos")]
    {
        if !installed {
            return Ok((false, false));
        }

        let output = Command::new("launchctl")
            .args(["list", LAUNCHD_LABEL])
            .output()?;

        let running = output.status.success();
        Ok((true, running))
    }

    #[cfg(target_os = "linux")]
    {
        if !installed {
            return Ok((false, false));
        }

        let output = Command::new("systemctl")
            .args(["--user", "is-active", SYSTEMD_SERVICE_NAME])
            .output()?;

        let running = output.status.success();
        Ok((true, running))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Err(ServiceError::UnsupportedPlatform)
    }
}

pub fn install_service() -> Result<String, ServiceError> {
    if is_service_installed() {
        return Err(ServiceError::AlreadyInstalled);
    }

    #[cfg(target_os = "macos")]
    {
        let plist_path = get_launchd_plist_path();

        if let Some(parent) = plist_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let plist_content = generate_launchd_plist()?;
        let mut file = File::create(&plist_path)?;
        file.write_all(plist_content.as_bytes())?;

        let output = Command::new("launchctl")
            .args(["load", "-w"])
            .arg(&plist_path)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            fs::remove_file(&plist_path)?;
            return Err(ServiceError::CommandFailed(stderr.to_string()));
        }

        Ok(format!(
            "Service installed and started. Plist: {}",
            plist_path.display()
        ))
    }

    #[cfg(target_os = "linux")]
    {
        let service_path = get_systemd_service_path();

        if let Some(parent) = service_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let service_content = generate_systemd_service()?;
        let mut file = File::create(&service_path)?;
        file.write_all(service_content.as_bytes())?;

        let reload = Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .output()?;

        if !reload.status.success() {
            let stderr = String::from_utf8_lossy(&reload.stderr);
            return Err(ServiceError::CommandFailed(format!(
                "daemon-reload failed: {}",
                stderr
            )));
        }

        let enable = Command::new("systemctl")
            .args(["--user", "enable", "--now", SYSTEMD_SERVICE_NAME])
            .output()?;

        if !enable.status.success() {
            let stderr = String::from_utf8_lossy(&enable.stderr);
            fs::remove_file(&service_path)?;
            return Err(ServiceError::CommandFailed(format!(
                "enable failed: {}",
                stderr
            )));
        }

        Ok(format!(
            "Service installed and enabled. Unit file: {}",
            service_path.display()
        ))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Err(ServiceError::UnsupportedPlatform)
    }
}

pub fn uninstall_service() -> Result<String, ServiceError> {
    if !is_service_installed() {
        return Err(ServiceError::NotInstalled);
    }

    #[cfg(target_os = "macos")]
    {
        let plist_path = get_launchd_plist_path();

        let _ = Command::new("launchctl")
            .args(["unload", "-w"])
            .arg(&plist_path)
            .output();

        fs::remove_file(&plist_path)?;

        Ok("Service uninstalled".to_string())
    }

    #[cfg(target_os = "linux")]
    {
        let service_path = get_systemd_service_path();

        let _ = Command::new("systemctl")
            .args(["--user", "stop", SYSTEMD_SERVICE_NAME])
            .output();

        let _ = Command::new("systemctl")
            .args(["--user", "disable", SYSTEMD_SERVICE_NAME])
            .output();

        fs::remove_file(&service_path)?;

        let _ = Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .output();

        Ok("Service uninstalled".to_string())
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Err(ServiceError::UnsupportedPlatform)
    }
}

pub fn start_service() -> Result<String, ServiceError> {
    if !is_service_installed() {
        return Err(ServiceError::NotInstalled);
    }

    #[cfg(target_os = "macos")]
    {
        let output = Command::new("launchctl")
            .args(["start", LAUNCHD_LABEL])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ServiceError::CommandFailed(stderr.to_string()));
        }

        Ok("Service started".to_string())
    }

    #[cfg(target_os = "linux")]
    {
        let output = Command::new("systemctl")
            .args(["--user", "start", SYSTEMD_SERVICE_NAME])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ServiceError::CommandFailed(stderr.to_string()));
        }

        Ok("Service started".to_string())
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Err(ServiceError::UnsupportedPlatform)
    }
}

pub fn stop_service() -> Result<String, ServiceError> {
    if !is_service_installed() {
        return Err(ServiceError::NotInstalled);
    }

    #[cfg(target_os = "macos")]
    {
        let output = Command::new("launchctl")
            .args(["stop", LAUNCHD_LABEL])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ServiceError::CommandFailed(stderr.to_string()));
        }

        Ok("Service stopped".to_string())
    }

    #[cfg(target_os = "linux")]
    {
        let output = Command::new("systemctl")
            .args(["--user", "stop", SYSTEMD_SERVICE_NAME])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ServiceError::CommandFailed(stderr.to_string()));
        }

        Ok("Service stopped".to_string())
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Err(ServiceError::UnsupportedPlatform)
    }
}

pub fn service_logs(lines: usize) -> Result<String, ServiceError> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
        let log_path = PathBuf::from(&home)
            .join(".config")
            .join("slinky")
            .join("daemon.log");

        if !log_path.exists() {
            return Ok("No logs available".to_string());
        }

        let content = fs::read_to_string(&log_path)?;
        let last_lines: Vec<&str> = content.lines().rev().take(lines).collect();
        let result: Vec<&str> = last_lines.into_iter().rev().collect();
        Ok(result.join("\n"))
    }

    #[cfg(target_os = "linux")]
    {
        let output = Command::new("journalctl")
            .args([
                "--user",
                "-u",
                SYSTEMD_SERVICE_NAME,
                "-n",
                &lines.to_string(),
                "--no-pager",
            ])
            .output()?;

        if !output.status.success() {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
            let log_path = PathBuf::from(&home)
                .join(".config")
                .join("slinky")
                .join("daemon.log");

            if log_path.exists() {
                let content = fs::read_to_string(&log_path)?;
                let last_lines: Vec<&str> = content.lines().rev().take(lines).collect();
                let result: Vec<&str> = last_lines.into_iter().rev().collect();
                return Ok(result.join("\n"));
            }

            return Ok("No logs available".to_string());
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Err(ServiceError::UnsupportedPlatform)
    }
}

pub fn get_platform_info() -> (&'static str, &'static str) {
    #[cfg(target_os = "macos")]
    {
        ("macOS", "launchd")
    }

    #[cfg(target_os = "linux")]
    {
        ("Linux", "systemd")
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        ("Unknown", "none")
    }
}
