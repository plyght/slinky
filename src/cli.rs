use clap::{Parser, Subcommand};
use colored::*;
use indicatif::{ProgressBar, ProgressStyle};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::config::{auto_detect_stow_dir, config_path, load_config, save_config, Config};
use crate::daemon::{
    daemon_status, get_daemon_pid, is_daemon_running, run_daemon, start_daemon_background,
    stop_daemon,
};
use crate::error::{Result, SlinkyError};
use crate::remote::{clone_or_update, get_repo_cache_path, parse_repo_spec};
use crate::secrets::{create_template, encrypt_secrets, scan_file_for_secrets, scan_shell_configs};
use crate::service::{
    get_platform_info, get_service_status, install_service, is_service_installed, service_logs,
    uninstall_service,
};
use crate::stow::{analyze_package, execute_operations, find_packages, OpType};

#[derive(Parser)]
#[command(
    name = "slnky",
    version,
    author,
    about = "🔗 A blazingly fast dotfiles manager with secret encryption",
    long_about = "Slinky (slnky) streamlines dotfile management by combining the simplicity of GNU Stow's\nsymlink paradigm with modern features like Git repository integration and age-based secret encryption.\n\nExamples:\n  slnky init                    # Set up slinky with smart defaults\n  slnky install user/dotfiles   # Clone and discover dotfiles\n  slnky link nvim               # Symlink the nvim package\n  slnky link --all              # Symlink all packages\n  slnky sync                    # Update and re-link all packages\n  slnky status                  # Show package status\n\nFor more info: https://github.com/nicojaffer/slinky"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    #[arg(long, global = true, help = "Show detailed output")]
    pub verbose: bool,

    #[arg(long, global = true, help = "Preview changes without applying")]
    pub dry_run: bool,

    #[arg(
        short = 'y',
        long = "yes",
        global = true,
        help = "Skip confirmations (useful for automation)"
    )]
    pub yes: bool,

    #[arg(
        long,
        global = true,
        value_name = "DIR",
        help = "Override target directory"
    )]
    pub target: Option<PathBuf>,
}

#[derive(Subcommand)]
pub enum Commands {
    #[command(about = "Initialize slinky with smart defaults", alias = "setup")]
    Init {
        #[arg(long, help = "Path to dotfiles directory")]
        stow_dir: Option<PathBuf>,

        #[arg(long, help = "Force re-initialization even if config exists")]
        force: bool,
    },

    #[command(about = "Clone a repository and discover its packages", alias = "i")]
    Install {
        #[arg(help = "Repository (e.g., user/repo, github.com/user/repo, https://...)")]
        repo: String,

        #[arg(long, help = "Link all packages after cloning")]
        link: bool,
    },

    #[command(about = "Link a package to the target directory", alias = "l")]
    Link {
        #[arg(help = "Package name to link (or use --all)")]
        package: Option<String>,

        #[arg(long, short = 'a', help = "Link all available packages")]
        all: bool,
    },

    #[command(about = "Unlink a package from the target directory", alias = "u")]
    Unlink {
        #[arg(help = "Package name to unlink (or use --all)")]
        package: Option<String>,

        #[arg(long, short = 'a', help = "Unlink all linked packages")]
        all: bool,
    },

    #[command(about = "Update repository and re-link all packages")]
    Sync {
        #[arg(long, help = "Only update, don't re-link")]
        no_link: bool,
    },

    #[command(
        about = "Show all packages and their link status",
        alias = "s",
        alias = "st"
    )]
    Status {
        #[arg(long, help = "Show detailed file-by-file status")]
        detailed: bool,
    },

    #[command(about = "View or modify configuration")]
    Config {
        #[command(subcommand)]
        command: Option<ConfigCommands>,
    },

    #[command(about = "Secret management commands")]
    Secrets {
        #[command(subcommand)]
        command: SecretsCommands,
    },

    #[command(about = "Background daemon for automatic syncing")]
    Daemon {
        #[command(subcommand)]
        command: DaemonCommands,
    },
}

#[derive(Subcommand)]
pub enum ConfigCommands {
    #[command(about = "Show current configuration")]
    Show,

    #[command(about = "Open config file in editor")]
    Edit,

    #[command(about = "Show path to config file")]
    Path,

    #[command(about = "Set a configuration value")]
    Set {
        #[arg(help = "Key to set (stow_dir, target_dir, secrets_enabled)")]
        key: String,

        #[arg(help = "Value to set")]
        value: String,
    },
}

#[derive(Subcommand)]
pub enum SecretsCommands {
    #[command(about = "Scan a file for potential secrets")]
    Scan {
        #[arg(help = "File to scan for secrets")]
        file: PathBuf,
    },

    #[command(about = "Encrypt detected secrets in dotfiles")]
    Encrypt,
}

#[derive(Subcommand)]
pub enum DaemonCommands {
    #[command(about = "Start the background daemon")]
    Start {
        #[arg(long, help = "Run in foreground (don't daemonize)")]
        foreground: bool,
    },

    #[command(about = "Stop the background daemon")]
    Stop,

    #[command(about = "Check daemon status")]
    Status {
        #[arg(long, short = 'l', help = "Show recent log entries")]
        logs: bool,

        #[arg(long, default_value = "10", help = "Number of log lines to show")]
        lines: usize,
    },

    #[command(about = "Install as system service (auto-start on boot)")]
    Install,

    #[command(about = "Uninstall system service")]
    Uninstall,

    #[command(about = "View daemon logs")]
    Logs {
        #[arg(long, short = 'n', default_value = "20", help = "Number of lines")]
        lines: usize,

        #[arg(long, short = 'f', help = "Follow log output")]
        follow: bool,
    },

    #[command(hide = true, about = "Run daemon in foreground (internal)")]
    Run,
}

pub fn run(cli: Cli) -> Result<()> {
    let is_first_run = !config_path().exists();
    let config = if is_first_run {
        Config::default()
    } else {
        load_config().unwrap_or_else(|_| Config::default())
    };

    match &cli.command {
        None => {
            if is_first_run {
                show_welcome();
                println!(
                    "\n{} Run {} to get started!",
                    "→".cyan(),
                    "slnky init".bright_white().bold()
                );
            } else {
                show_status_command(&cli, &config, false)?;
            }
            Ok(())
        }
        Some(Commands::Init { stow_dir, force }) => init_slinky(stow_dir.clone(), *force, &cli),
        Some(Commands::Install { repo, link }) => install_repo(repo, *link, &cli, &config),
        Some(Commands::Link { package, all }) => {
            if *all {
                link_all_packages(&cli, &config)
            } else if let Some(pkg) = package {
                link_package(pkg, &cli, &config)
            } else {
                Err(SlinkyError::Other(
                    "Specify a package name or use --all".to_string(),
                ))
            }
        }
        Some(Commands::Unlink { package, all }) => {
            if *all {
                unlink_all_packages(&cli, &config)
            } else if let Some(pkg) = package {
                unlink_package(pkg, &cli, &config)
            } else {
                Err(SlinkyError::Other(
                    "Specify a package name or use --all".to_string(),
                ))
            }
        }
        Some(Commands::Sync { no_link }) => sync_dotfiles(*no_link, &cli, &config),
        Some(Commands::Status { detailed }) => show_status_command(&cli, &config, *detailed),
        Some(Commands::Config { command }) => handle_config_command(command.as_ref(), &cli),
        Some(Commands::Secrets { command }) => match command {
            SecretsCommands::Scan { file } => scan_secrets(file, &cli),
            SecretsCommands::Encrypt => encrypt_all_secrets(&cli, &config),
        },
        Some(Commands::Daemon { command }) => handle_daemon_command(command, &cli, &config),
    }
}

fn show_welcome() {
    println!("\n{}", "Welcome to Slinky! 🔗".bright_cyan().bold());
    println!("{}", "━".repeat(40).dimmed());
    println!("\nSlinky is a modern dotfiles manager that helps you:");
    println!(
        "  {} Organize configs into packages (like GNU Stow)",
        "•".bright_blue()
    );
    println!(
        "  {} Sync dotfiles across machines via Git",
        "•".bright_blue()
    );
    println!(
        "  {} Encrypt secrets with age encryption",
        "•".bright_blue()
    );

    if let Some(detected_dir) = auto_detect_stow_dir() {
        println!(
            "\n{}",
            "✓ Detected existing dotfiles:".bright_green().bold()
        );
        println!(
            "  {} {}",
            "→".cyan(),
            detected_dir.display().to_string().bright_white()
        );
        println!(
            "\n{} Run {} to set up with auto-detected directory",
            "→".cyan(),
            "slnky init".bright_white()
        );
    } else {
        println!("\n{}", "Quick Start:".bright_white().bold());
        println!(
            "  {} {} - Set up with smart defaults",
            "1.".dimmed(),
            "slnky init".bright_white()
        );
        println!(
            "  {} {} - Clone your dotfiles",
            "2.".dimmed(),
            "slnky install user/dotfiles".bright_white()
        );
        println!(
            "  {} {} - Link all packages",
            "3.".dimmed(),
            "slnky link --all".bright_white()
        );
    }
}

fn init_slinky(stow_dir: Option<PathBuf>, force: bool, cli: &Cli) -> Result<()> {
    print_header("Initializing Slinky");

    let config_file = config_path();
    if config_file.exists() && !force {
        println!(
            "{} Configuration already exists at {}",
            "✓".green(),
            config_file.display().to_string().bright_white()
        );
        println!(
            "\n{} Use {} to reinitialize",
            "→".cyan(),
            "--force".bright_white()
        );
        return Ok(());
    }

    let detected_stow_dir = stow_dir.or_else(detect_dotfiles_dir);

    let final_stow_dir = if let Some(dir) = detected_stow_dir {
        if cli.yes {
            dir
        } else {
            println!(
                "{} Detected dotfiles directory: {}",
                "→".cyan(),
                dir.display().to_string().bright_white()
            );
            if confirm("Use this directory?", true)? {
                dir
            } else {
                prompt_path("Enter dotfiles directory", &Config::default().stow_dir)?
            }
        }
    } else {
        let default = Config::default().stow_dir;
        if cli.yes {
            default
        } else {
            prompt_path("Enter dotfiles directory", &default)?
        }
    };

    let home = dirs_home().unwrap_or_else(|| PathBuf::from("/"));
    let config = Config {
        stow_dir: final_stow_dir.clone(),
        target_dir: home.clone(),
        packages: Vec::new(),
        secrets_enabled: true,
        auto_sync: crate::config::AutoSyncConfig::default(),
    };

    if cli.dry_run {
        println!("{} Would create config:", "🔍".bright_blue());
        println!("  stow_dir: {}", config.stow_dir.display());
        println!("  target_dir: {}", config.target_dir.display());
        return Ok(());
    }

    save_config(&config).map_err(|e| SlinkyError::Config(e.to_string()))?;

    println!(
        "{} Configuration saved to {}",
        "✓".green(),
        config_file.display().to_string().bright_white()
    );
    println!(
        "\n{} Dotfiles directory: {}",
        "→".cyan(),
        config.stow_dir.display().to_string().bright_white()
    );
    println!(
        "{} Target directory: {}",
        "→".cyan(),
        config.target_dir.display().to_string().bright_white()
    );

    if !final_stow_dir.exists() {
        println!("\n{} Dotfiles directory doesn't exist yet.", "⚠".yellow());
        println!(
            "{} Run {} to clone your dotfiles",
            "→".cyan(),
            "slnky install user/repo".bright_white()
        );
    } else {
        let packages = find_packages(&final_stow_dir).unwrap_or_default();
        if !packages.is_empty() {
            println!(
                "\n{} Found {} package(s). Run {} to link them",
                "✓".green(),
                packages.len().to_string().bright_white(),
                "slnky link --all".bright_white()
            );
        }
    }

    Ok(())
}

fn detect_dotfiles_dir() -> Option<PathBuf> {
    let home = dirs_home()?;
    let candidates = [
        home.join(".dotfiles"),
        home.join("dotfiles"),
        home.join(".config/dotfiles"),
        home.join("code/dotfiles"),
        home.join("projects/dotfiles"),
    ];

    for candidate in candidates {
        if candidate.exists() && candidate.is_dir() {
            if let Ok(packages) = find_packages(&candidate) {
                if !packages.is_empty() {
                    return Some(candidate);
                }
            }
            if candidate.join(".git").exists() {
                return Some(candidate);
            }
        }
    }

    None
}

fn sync_dotfiles(no_link: bool, cli: &Cli, config: &Config) -> Result<()> {
    print_header("Syncing Dotfiles");

    if !config.stow_dir.exists() {
        return Err(SlinkyError::Other(format!(
            "Dotfiles directory not found: {}\nRun 'slnky install user/repo' first",
            config.stow_dir.display()
        )));
    }

    if config.stow_dir.join(".git").exists() {
        let spinner = create_spinner("Pulling latest changes...");

        if cli.dry_run {
            spinner
                .finish_with_message(format!("{} Would pull latest changes", "🔍".bright_blue()));
        } else {
            let output = std::process::Command::new("git")
                .current_dir(&config.stow_dir)
                .args(["pull", "--ff-only"])
                .output()
                .map_err(|e| SlinkyError::Git(e.to_string()))?;

            if output.status.success() {
                spinner.finish_with_message(format!("{} Repository updated", "✓".green()));
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                if stderr.contains("Already up to date") {
                    spinner.finish_with_message(format!("{} Already up to date", "✓".green()));
                } else {
                    spinner.finish_with_message(format!(
                        "{} Pull failed: {}",
                        "✗".red(),
                        stderr.trim()
                    ));
                }
            }
        }
    } else {
        println!("{} Not a git repository, skipping pull", "⚠".yellow());
    }

    if !no_link {
        println!();
        link_all_packages(cli, config)?;
    }

    Ok(())
}

fn handle_config_command(command: Option<&ConfigCommands>, cli: &Cli) -> Result<()> {
    match command {
        None | Some(ConfigCommands::Show) => {
            print_header("Configuration");

            let path = config_path();
            if !path.exists() {
                println!(
                    "{} No config file found. Run {} to create one.",
                    "⚠".yellow(),
                    "slnky init".bright_white()
                );
                return Ok(());
            }

            let config = load_config().map_err(|e| SlinkyError::Config(e.to_string()))?;

            println!(
                "{} {}",
                "Config file:".dimmed(),
                path.display().to_string().bright_white()
            );
            println!();
            println!(
                "  {} {}",
                "stow_dir:".bright_blue(),
                config.stow_dir.display().to_string().bright_white()
            );
            println!(
                "  {} {}",
                "target_dir:".bright_blue(),
                config.target_dir.display().to_string().bright_white()
            );
            println!(
                "  {} {}",
                "secrets_enabled:".bright_blue(),
                config.secrets_enabled.to_string().bright_white()
            );

            if !config.packages.is_empty() {
                println!("  {} {:?}", "packages:".bright_blue(), config.packages);
            }

            Ok(())
        }
        Some(ConfigCommands::Edit) => {
            let path = config_path();
            let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vim".to_string());

            println!(
                "{} Opening {} in {}",
                "→".cyan(),
                path.display().to_string().bright_white(),
                editor.bright_white()
            );

            if cli.dry_run {
                return Ok(());
            }

            std::process::Command::new(&editor)
                .arg(&path)
                .status()
                .map_err(|e| SlinkyError::Other(format!("Failed to open editor: {}", e)))?;

            Ok(())
        }
        Some(ConfigCommands::Path) => {
            println!("{}", config_path().display());
            Ok(())
        }
        Some(ConfigCommands::Set { key, value }) => {
            let mut config = load_config().map_err(|e| SlinkyError::Config(e.to_string()))?;

            match key.as_str() {
                "stow_dir" => {
                    config.stow_dir = PathBuf::from(value);
                }
                "target_dir" => {
                    config.target_dir = PathBuf::from(value);
                }
                "secrets_enabled" => {
                    config.secrets_enabled = value.parse().map_err(|_| {
                        SlinkyError::Config("secrets_enabled must be 'true' or 'false'".to_string())
                    })?;
                }
                _ => {
                    return Err(SlinkyError::Config(format!(
                        "Unknown config key: {}. Valid keys: stow_dir, target_dir, secrets_enabled",
                        key
                    )));
                }
            }

            if cli.dry_run {
                println!(
                    "{} Would set {} = {}",
                    "🔍".bright_blue(),
                    key.bright_white(),
                    value.bright_white()
                );
                return Ok(());
            }

            save_config(&config).map_err(|e| SlinkyError::Config(e.to_string()))?;
            println!(
                "{} Set {} = {}",
                "✓".green(),
                key.bright_white(),
                value.bright_white()
            );

            Ok(())
        }
    }
}

fn confirm(prompt: &str, default: bool) -> Result<bool> {
    let default_hint = if default { "[Y/n]" } else { "[y/N]" };
    print!(
        "{} {} {} ",
        "?".bright_blue(),
        prompt,
        default_hint.dimmed()
    );
    io::stdout().flush().map_err(SlinkyError::Io)?;

    let mut input = String::new();
    io::stdin().read_line(&mut input).map_err(SlinkyError::Io)?;

    let input = input.trim().to_lowercase();
    Ok(if input.is_empty() {
        default
    } else {
        input.starts_with('y')
    })
}

fn prompt_path(prompt: &str, default: &Path) -> Result<PathBuf> {
    print!(
        "{} {} [{}]: ",
        "?".bright_blue(),
        prompt,
        default.display().to_string().dimmed()
    );
    io::stdout().flush().map_err(SlinkyError::Io)?;

    let mut input = String::new();
    io::stdin().read_line(&mut input).map_err(SlinkyError::Io)?;

    let input = input.trim();
    Ok(if input.is_empty() {
        default.to_path_buf()
    } else {
        PathBuf::from(shellexpand_tilde(input))
    })
}

fn shellexpand_tilde(path: &str) -> String {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs_home() {
            return home.join(stripped).to_string_lossy().to_string();
        }
    }
    path.to_string()
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn link_all_packages(cli: &Cli, config: &Config) -> Result<()> {
    print_header("Linking All Packages");

    let packages = find_packages(&config.stow_dir).map_err(|e| SlinkyError::Stow(e.to_string()))?;

    if packages.is_empty() {
        println!(
            "{} No packages found in {}",
            "⚠".yellow(),
            config.stow_dir.display()
        );
        return Ok(());
    }

    let target = cli
        .target
        .as_ref()
        .cloned()
        .unwrap_or_else(|| config.target_dir.clone());

    println!(
        "{} Linking {} package(s) to {}\n",
        "→".cyan(),
        packages.len().to_string().bright_white(),
        target.display().to_string().bright_white()
    );

    let mut success_count = 0;
    let mut already_linked_count = 0;
    let mut error_count = 0;

    for package in &packages {
        let result = link_single_package(&package.name, &package.path, &target, cli);
        match result {
            Ok(linked) => {
                if linked {
                    success_count += 1;
                } else {
                    already_linked_count += 1;
                }
            }
            Err(e) => {
                println!("  {} {} - {}", "✗".red(), package.name.bright_white(), e);
                error_count += 1;
            }
        }
    }

    println!();
    if success_count > 0 {
        println!(
            "{} {} package(s) linked",
            "✓".green(),
            success_count.to_string().bright_white()
        );
    }
    if already_linked_count > 0 {
        println!(
            "{} {} package(s) already linked",
            "→".cyan(),
            already_linked_count.to_string().dimmed()
        );
    }
    if error_count > 0 {
        println!(
            "{} {} package(s) failed",
            "✗".red(),
            error_count.to_string().bright_red()
        );
    }

    if success_count == 0 && already_linked_count > 0 && error_count == 0 {
        println!("\n{} All packages are already linked!", "✓".green());
    }

    Ok(())
}

fn link_single_package(name: &str, package_path: &Path, target: &Path, cli: &Cli) -> Result<bool> {
    let operations =
        analyze_package(package_path, target).map_err(|e| SlinkyError::Stow(e.to_string()))?;

    let create_ops: Vec<_> = operations
        .iter()
        .filter(|op| matches!(op.op_type, OpType::Create))
        .collect();

    if create_ops.is_empty() {
        println!(
            "  {} {} {}",
            "→".dimmed(),
            name.dimmed(),
            "(already linked)".dimmed()
        );
        return Ok(false);
    }

    if cli.dry_run {
        println!(
            "  {} {} - would create {} symlink(s)",
            "🔍".bright_blue(),
            name.bright_white(),
            create_ops.len()
        );
        return Ok(true);
    }

    execute_operations(&operations, false).map_err(|e| SlinkyError::Stow(e.to_string()))?;
    println!(
        "  {} {} - {} symlink(s) created",
        "✓".green(),
        name.bright_white(),
        create_ops.len()
    );

    Ok(true)
}

fn unlink_all_packages(cli: &Cli, config: &Config) -> Result<()> {
    print_header("Unlinking All Packages");

    let packages = find_packages(&config.stow_dir).map_err(|e| SlinkyError::Stow(e.to_string()))?;

    if packages.is_empty() {
        println!("{} No packages found", "⚠".yellow());
        return Ok(());
    }

    let target = cli
        .target
        .as_ref()
        .cloned()
        .unwrap_or_else(|| config.target_dir.clone());

    if !cli.yes && !cli.dry_run {
        println!(
            "{} This will unlink {} package(s)",
            "⚠".yellow(),
            packages.len()
        );
        if !confirm("Continue?", false)? {
            println!("{} Cancelled", "→".cyan());
            return Ok(());
        }
    }

    for package in &packages {
        unlink_single_package(&package.name, &package.path, &target, cli)?;
    }

    Ok(())
}

fn unlink_single_package(name: &str, package_path: &Path, target: &Path, cli: &Cli) -> Result<()> {
    let operations =
        analyze_package(package_path, target).map_err(|e| SlinkyError::Stow(e.to_string()))?;

    let linked_ops: Vec<_> = operations
        .iter()
        .filter(|op| {
            if let OpType::Skip(reason) = &op.op_type {
                reason.contains("Already linked")
            } else {
                false
            }
        })
        .collect();

    if linked_ops.is_empty() {
        println!(
            "  {} {} {}",
            "→".dimmed(),
            name.dimmed(),
            "(not linked)".dimmed()
        );
        return Ok(());
    }

    if cli.dry_run {
        println!(
            "  {} {} - would remove {} symlink(s)",
            "🔍".bright_blue(),
            name.bright_white(),
            linked_ops.len()
        );
        return Ok(());
    }

    for op in &linked_ops {
        if op.target.is_symlink() {
            fs::remove_file(&op.target).map_err(SlinkyError::Io)?;
        }
    }

    println!(
        "  {} {} - {} symlink(s) removed",
        "✓".green(),
        name.bright_white(),
        linked_ops.len()
    );

    Ok(())
}

fn install_repo(repo: &str, link_after: bool, cli: &Cli, config: &Config) -> Result<()> {
    print_header("Installing Repository");

    let repo_spec =
        parse_repo_spec(repo).map_err(|e| SlinkyError::InvalidRepoSpec(e.to_string()))?;

    if cli.verbose {
        println!("{} Parsing repository: {}", "→".cyan(), repo.bright_white());
        println!(
            "{} Owner: {}, Repo: {}",
            "→".cyan(),
            repo_spec.owner.bright_white(),
            repo_spec.repo.bright_white()
        );
    }

    let repo_path = get_repo_cache_path(&repo_spec);
    let is_update = repo_path.exists();

    if cli.dry_run {
        let action = if is_update { "update" } else { "clone" };
        println!(
            "{} Would {}: {}",
            "🔍".bright_blue(),
            action,
            repo.bright_white()
        );
        return Ok(());
    }

    let spinner_msg = if is_update {
        "Updating repository..."
    } else {
        "Cloning repository..."
    };
    let spinner = create_spinner(spinner_msg);
    let repo_path = clone_or_update(&repo_spec).map_err(|e| SlinkyError::Remote(e.to_string()))?;

    let finish_msg = if is_update {
        format!(
            "{} Repository updated: {}",
            "✓".green(),
            repo_path.display().to_string().bright_white()
        )
    } else {
        format!(
            "{} Repository cloned to {}",
            "✓".green(),
            repo_path.display().to_string().bright_white()
        )
    };
    spinner.finish_with_message(finish_msg);

    let packages = find_packages(&repo_path).map_err(|e| SlinkyError::Stow(e.to_string()))?;

    if packages.is_empty() {
        println!("\n{} No packages found in repository", "⚠".yellow());
        println!(
            "{} Make sure your dotfiles are organized into package directories",
            "→".cyan()
        );
        return Ok(());
    }

    println!(
        "\n{} Found {} package(s):",
        "✓".green(),
        packages.len().to_string().bright_white()
    );

    for package in &packages {
        println!("  {} {}", "•".bright_blue(), package.name.bright_white());
    }

    let mut updated_config = config.clone();
    if updated_config.stow_dir != repo_path {
        updated_config.stow_dir = repo_path.clone();

        if cli.yes || confirm("\nUpdate config to use this repository?", true)? {
            save_config(&updated_config).map_err(|e| SlinkyError::Config(e.to_string()))?;
            println!("{} Config updated with new stow_dir", "✓".green());
        }
    }

    if link_after {
        println!();
        link_all_packages(cli, &updated_config)?;
    } else {
        println!(
            "\n{} Run {} to link packages",
            "→".cyan(),
            "slnky link --all".bright_white()
        );
    }

    Ok(())
}

fn link_package(package: &str, cli: &Cli, config: &Config) -> Result<()> {
    print_header("Linking Package");

    let target = cli
        .target
        .as_ref()
        .cloned()
        .unwrap_or_else(|| config.target_dir.clone());

    if cli.verbose {
        println!("{} Package: {}", "→".cyan(), package.bright_white());
        println!(
            "{} Target: {}",
            "→".cyan(),
            target.display().to_string().bright_white()
        );
        println!(
            "{} Stow dir: {}",
            "→".cyan(),
            config.stow_dir.display().to_string().bright_white()
        );
    }

    let package_path = config.stow_dir.join(package);
    if !package_path.exists() {
        let available = find_packages(&config.stow_dir)
            .map(|pkgs| {
                pkgs.iter()
                    .map(|p| p.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();

        let hint = if available.is_empty() {
            format!("No packages found in {}", config.stow_dir.display())
        } else {
            format!("Available packages: {}", available)
        };

        return Err(SlinkyError::PackageNotFound(format!(
            "{}\n{} {}",
            package,
            "→".cyan(),
            hint.dimmed()
        )));
    }

    let operations =
        analyze_package(&package_path, &target).map_err(|e| SlinkyError::Stow(e.to_string()))?;

    let create_ops: Vec<_> = operations
        .iter()
        .filter(|op| matches!(op.op_type, OpType::Create))
        .collect();

    let skip_ops: Vec<_> = operations
        .iter()
        .filter(|op| {
            if let OpType::Skip(reason) = &op.op_type {
                reason.contains("Already linked")
            } else {
                false
            }
        })
        .collect();

    if create_ops.is_empty() {
        if !skip_ops.is_empty() {
            println!(
                "{} Package {} already linked ({} symlink(s))",
                "✓".green(),
                package.bright_white(),
                skip_ops.len()
            );
        } else {
            println!(
                "{} Package {} has no files to link",
                "→".cyan(),
                package.bright_white()
            );
        }
        return Ok(());
    }

    if cli.dry_run {
        println!(
            "{} Would create {} symlink(s):",
            "🔍".bright_blue(),
            create_ops.len().to_string().bright_white()
        );
        for op in &create_ops {
            println!(
                "  {} {} → {}",
                "•".bright_blue(),
                op.target.display().to_string().dimmed(),
                op.source.display().to_string().bright_white()
            );
        }
        if !skip_ops.is_empty() {
            println!(
                "\n{} {} symlink(s) already linked",
                "→".cyan(),
                skip_ops.len()
            );
        }
        return Ok(());
    }

    let spinner = create_spinner(&format!("Linking {}...", package));
    execute_operations(&operations, false).map_err(|e| SlinkyError::Stow(e.to_string()))?;

    let mut msg = format!(
        "{} Package {} linked ({} symlinks created)",
        "✓".green(),
        package.bright_white(),
        create_ops.len()
    );
    if !skip_ops.is_empty() {
        msg.push_str(&format!(", {} already linked", skip_ops.len()));
    }
    spinner.finish_with_message(msg);

    Ok(())
}

fn unlink_package(package: &str, cli: &Cli, config: &Config) -> Result<()> {
    print_header("Unlinking Package");

    let target = cli
        .target
        .as_ref()
        .cloned()
        .unwrap_or_else(|| config.target_dir.clone());

    if cli.verbose {
        println!("{} Package: {}", "→".cyan(), package.bright_white());
        println!(
            "{} Target: {}",
            "→".cyan(),
            target.display().to_string().bright_white()
        );
    }

    let package_path = config.stow_dir.join(package);
    if !package_path.exists() {
        return Err(SlinkyError::PackageNotFound(package.to_string()));
    }

    let operations =
        analyze_package(&package_path, &target).map_err(|e| SlinkyError::Stow(e.to_string()))?;

    let linked_ops: Vec<_> = operations
        .iter()
        .filter(|op| {
            if let OpType::Skip(reason) = &op.op_type {
                reason.contains("Already linked")
            } else {
                false
            }
        })
        .collect();

    if linked_ops.is_empty() {
        println!(
            "{} Package {} is not linked",
            "→".cyan(),
            package.bright_white()
        );
        return Ok(());
    }

    if !cli.yes && !cli.dry_run {
        println!(
            "{} This will remove {} symlink(s)",
            "⚠".yellow(),
            linked_ops.len()
        );
        if !confirm("Continue?", true)? {
            println!("{} Cancelled", "→".cyan());
            return Ok(());
        }
    }

    if cli.dry_run {
        println!(
            "{} Would remove {} symlink(s):",
            "🔍".bright_blue(),
            linked_ops.len().to_string().bright_white()
        );
        for op in &linked_ops {
            println!(
                "  {} {}",
                "•".bright_blue(),
                op.target.display().to_string().dimmed()
            );
        }
        return Ok(());
    }

    let spinner = create_spinner(&format!("Unlinking {}...", package));
    let mut removed = 0;
    for op in &linked_ops {
        if op.target.is_symlink() {
            fs::remove_file(&op.target).map_err(SlinkyError::Io)?;
            removed += 1;
        }
    }
    spinner.finish_with_message(format!(
        "{} Package {} unlinked ({} symlinks removed)",
        "✓".green(),
        package.bright_white(),
        removed
    ));

    Ok(())
}

fn show_status_command(cli: &Cli, config: &Config, detailed: bool) -> Result<()> {
    print_header("Package Status");

    let mut effective_config = config.clone();
    let mut auto_detected = false;

    if !config.stow_dir.exists() {
        if let Some(detected_dir) = auto_detect_stow_dir() {
            println!(
                "{} Auto-detected dotfiles directory: {}",
                "→".cyan(),
                detected_dir.display().to_string().bright_white()
            );
            effective_config.stow_dir = detected_dir;
            auto_detected = true;
        } else {
            println!(
                "{} Dotfiles directory not found: {}",
                "⚠".yellow(),
                config.stow_dir.display().to_string().bright_white()
            );
            println!(
                "\n{} Run {} to clone your dotfiles",
                "→".cyan(),
                "slnky install user/repo".bright_white()
            );
            return Ok(());
        }
    }

    let packages =
        find_packages(&effective_config.stow_dir).map_err(|e| SlinkyError::Stow(e.to_string()))?;

    if packages.is_empty() {
        println!(
            "{} No packages found in {}",
            "⚠".yellow(),
            effective_config
                .stow_dir
                .display()
                .to_string()
                .bright_white()
        );
        return Ok(());
    }

    let target = cli
        .target
        .as_ref()
        .cloned()
        .unwrap_or_else(|| effective_config.target_dir.clone());

    println!(
        "{} Stow directory: {}",
        "→".cyan(),
        effective_config
            .stow_dir
            .display()
            .to_string()
            .bright_white()
    );
    println!(
        "{} Target directory: {}\n",
        "→".cyan(),
        target.display().to_string().bright_white()
    );

    let mut linked_count = 0;
    let mut partial_count = 0;
    let mut unlinked_count = 0;

    for package in &packages {
        let ops = analyze_package(&package.path, &target).unwrap_or_default();

        let total_files = ops.len();
        let linked_files = ops
            .iter()
            .filter(|op| {
                if let OpType::Skip(reason) = &op.op_type {
                    reason.contains("Already linked")
                } else {
                    false
                }
            })
            .count();
        let _create_needed = ops
            .iter()
            .filter(|op| matches!(op.op_type, OpType::Create))
            .count();

        let (icon, status, status_color) = if linked_files == total_files && total_files > 0 {
            linked_count += 1;
            ("✓", "linked".to_string(), "green")
        } else if linked_files > 0 {
            partial_count += 1;
            (
                "◐",
                format!("partial ({}/{})", linked_files, total_files),
                "yellow",
            )
        } else {
            unlinked_count += 1;
            ("○", "not linked".to_string(), "dimmed")
        };

        let status_display = match status_color {
            "green" => format!("({})", status).green(),
            "yellow" => format!("({})", status).yellow(),
            _ => format!("({})", status).dimmed(),
        };

        println!(
            "  {} {} {}",
            icon.bright_blue(),
            package.name.bright_white(),
            status_display
        );

        if detailed && (cli.verbose || linked_files > 0) {
            for op in &ops {
                let (file_icon, file_status) = match &op.op_type {
                    OpType::Skip(reason) if reason.contains("Already linked") => {
                        ("  ✓".green(), op.target.display().to_string().dimmed())
                    }
                    OpType::Create => (
                        "  ○".dimmed(),
                        format!("{} (would link)", op.target.display()).dimmed(),
                    ),
                    OpType::Skip(reason) => (
                        "  ⊘".yellow(),
                        format!("{} ({})", op.target.display(), reason).dimmed(),
                    ),
                    OpType::Remove => ("  ✗".red(), op.target.display().to_string().dimmed()),
                };
                println!("    {} {}", file_icon, file_status);
            }
        }
    }

    println!();
    println!(
        "{} {} linked, {} partial, {} not linked",
        "Summary:".bright_white().bold(),
        linked_count.to_string().green(),
        partial_count.to_string().yellow(),
        unlinked_count.to_string().dimmed()
    );

    if auto_detected {
        println!(
            "\n{} Run {} to save this configuration",
            "→".cyan(),
            "slnky init".bright_white()
        );
    }

    if unlinked_count > 0 || partial_count > 0 {
        println!(
            "\n{} Run {} to link all packages",
            "→".cyan(),
            "slnky link --all".bright_white()
        );
    }

    Ok(())
}

fn scan_secrets(file: &Path, cli: &Cli) -> Result<()> {
    print_header("Scanning for Secrets");

    if !file.exists() {
        return Err(SlinkyError::Other(format!(
            "File not found: {}",
            file.display()
        )));
    }

    if cli.verbose {
        println!(
            "{} File: {}",
            "→".cyan(),
            file.display().to_string().bright_white()
        );
    }

    let spinner = create_spinner("Scanning for secrets...");
    let secrets = scan_file_for_secrets(file).map_err(|e| SlinkyError::Secrets(e.to_string()))?;
    spinner.finish_and_clear();

    if secrets.is_empty() {
        println!("{} No secrets detected", "✓".green());
    } else {
        println!(
            "{} Found {} potential secret(s):",
            "⚠".yellow(),
            secrets.len().to_string().bright_white()
        );
        for secret in secrets {
            println!("  {} {}", "•".red(), secret.name.bright_white());
        }
    }

    Ok(())
}

fn encrypt_all_secrets(cli: &Cli, _config: &Config) -> Result<()> {
    print_header("Encrypting Secrets");

    if cli.dry_run {
        println!("{} Would scan and encrypt secrets", "🔍".bright_blue());
        return Ok(());
    }

    let spinner = create_spinner("Scanning shell configs...");
    let files = scan_shell_configs().map_err(|e| SlinkyError::Secrets(e.to_string()))?;
    spinner.finish_and_clear();

    let mut all_secrets = Vec::new();
    for file in &files {
        if let Ok(secrets) = scan_file_for_secrets(file) {
            all_secrets.extend(secrets);
        }
    }

    if all_secrets.is_empty() {
        println!("{} No secrets found", "✓".green());
        return Ok(());
    }

    println!(
        "{} Found {} secret(s)",
        "⚠".yellow(),
        all_secrets.len().to_string().bright_white()
    );

    println!("\n{} Enter passphrase to encrypt secrets:", "🔒".cyan());
    let passphrase = rpassword::read_password()
        .map_err(|e| SlinkyError::Other(format!("Failed to read passphrase: {}", e)))?;

    let spinner = create_spinner("Creating templates...");
    for file in &files {
        let file_secrets: Vec<_> = all_secrets
            .iter()
            .filter(|s| s.file == *file)
            .cloned()
            .collect();
        if !file_secrets.is_empty() {
            create_template(file, &file_secrets)
                .map_err(|e| SlinkyError::Secrets(e.to_string()))?;
        }
    }
    spinner.finish_with_message(format!("{} Templates created", "✓".green()));

    let spinner = create_spinner("Encrypting secrets...");
    encrypt_secrets(&all_secrets, &passphrase)
        .map_err(|e| SlinkyError::Encryption(e.to_string()))?;
    spinner.finish_with_message(format!("{} Secrets encrypted", "✓".green()));

    Ok(())
}

fn print_header(title: &str) {
    println!("\n{}", title.bright_cyan().bold());
    println!("{}\n", "─".repeat(title.len()).dimmed());
}

fn create_spinner(msg: &str) -> ProgressBar {
    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} {msg}")
            .unwrap()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
    );
    spinner.set_message(msg.to_string());
    spinner.enable_steady_tick(std::time::Duration::from_millis(80));
    spinner
}

fn handle_daemon_command(command: &DaemonCommands, cli: &Cli, config: &Config) -> Result<()> {
    match command {
        DaemonCommands::Start { foreground } => {
            if *foreground {
                print_header("Starting Daemon (Foreground)");
                println!(
                    "{} Watching: {}",
                    "→".cyan(),
                    config.stow_dir.display().to_string().bright_white()
                );
                println!(
                    "{} Target: {}",
                    "→".cyan(),
                    config.target_dir.display().to_string().bright_white()
                );
                println!("{} Press Ctrl+C to stop\n", "→".cyan());

                run_daemon().map_err(|e| SlinkyError::Other(e.to_string()))?;
            } else {
                print_header("Starting Daemon");

                if is_daemon_running() {
                    let pid = get_daemon_pid().unwrap_or(0);
                    println!(
                        "{} Daemon already running (PID: {})",
                        "⚠".yellow(),
                        pid.to_string().bright_white()
                    );
                    return Ok(());
                }

                if cli.dry_run {
                    println!("{} Would start background daemon", "🔍".bright_blue());
                    return Ok(());
                }

                let spinner = create_spinner("Starting daemon...");
                match start_daemon_background() {
                    Ok(pid) => {
                        spinner.finish_with_message(format!(
                            "{} Daemon started (PID: {})",
                            "✓".green(),
                            pid.to_string().bright_white()
                        ));
                        println!(
                            "\n{} Watching: {}",
                            "→".cyan(),
                            config.stow_dir.display().to_string().bright_white()
                        );
                        println!(
                            "{} Target: {}",
                            "→".cyan(),
                            config.target_dir.display().to_string().bright_white()
                        );
                        println!(
                            "\n{} Run {} to check status",
                            "→".cyan(),
                            "slnky daemon status".bright_white()
                        );
                    }
                    Err(e) => {
                        spinner.finish_with_message(format!(
                            "{} Failed to start daemon: {}",
                            "✗".red(),
                            e
                        ));
                    }
                }
            }
            Ok(())
        }

        DaemonCommands::Stop => {
            print_header("Stopping Daemon");

            if !is_daemon_running() {
                println!("{} Daemon is not running", "→".cyan());
                return Ok(());
            }

            if cli.dry_run {
                println!("{} Would stop daemon", "🔍".bright_blue());
                return Ok(());
            }

            let spinner = create_spinner("Stopping daemon...");
            match stop_daemon() {
                Ok(()) => {
                    spinner.finish_with_message(format!("{} Daemon stopped", "✓".green()));
                }
                Err(e) => {
                    spinner.finish_with_message(format!(
                        "{} Failed to stop daemon: {}",
                        "✗".red(),
                        e
                    ));
                }
            }
            Ok(())
        }

        DaemonCommands::Status { logs, lines } => {
            print_header("Daemon Status");

            let (running, pid, log_excerpt) = daemon_status();

            let (platform, init_system) = get_platform_info();
            println!(
                "{} Platform: {} ({})",
                "→".cyan(),
                platform.bright_white(),
                init_system.dimmed()
            );

            if running {
                println!(
                    "{} Status: {} (PID: {})",
                    "✓".green(),
                    "Running".bright_green(),
                    pid.unwrap_or(0).to_string().bright_white()
                );
            } else {
                println!("{} Status: {}", "○".dimmed(), "Not running".dimmed());
            }

            let (installed, service_running) = get_service_status().unwrap_or((false, false));
            if installed {
                let status = if service_running {
                    "active".bright_green()
                } else {
                    "inactive".yellow()
                };
                println!(
                    "{} Service: {} ({})",
                    "→".cyan(),
                    "Installed".bright_white(),
                    status
                );
            } else {
                println!(
                    "{} Service: {} (run {} to enable auto-start)",
                    "→".cyan(),
                    "Not installed".dimmed(),
                    "slnky daemon install".bright_white()
                );
            }

            println!(
                "\n{} Auto-sync: {}",
                "→".cyan(),
                if config.auto_sync.enabled {
                    "Enabled".bright_green()
                } else {
                    "Disabled".dimmed()
                }
            );
            println!(
                "{} Auto-link new packages: {}",
                "→".cyan(),
                if config.auto_sync.auto_link_new_packages {
                    "Yes".bright_green()
                } else {
                    "No".dimmed()
                }
            );
            println!(
                "{} Auto git pull: {}",
                "→".cyan(),
                if config.auto_sync.auto_git_pull {
                    "Yes".bright_green()
                } else {
                    "No".dimmed()
                }
            );
            println!(
                "{} Conflict resolution: {}",
                "→".cyan(),
                format!("{:?}", config.auto_sync.conflict_resolution)
                    .to_lowercase()
                    .bright_white()
            );

            if *logs || log_excerpt.is_some() {
                println!("\n{}", "Recent Activity:".bright_white().bold());
                println!("{}", "─".repeat(20).dimmed());
                if let Ok(log_content) = service_logs(*lines) {
                    if log_content.is_empty() || log_content == "No logs available" {
                        println!("{}", "  No recent activity".dimmed());
                    } else {
                        for line in log_content.lines() {
                            println!("  {}", line.dimmed());
                        }
                    }
                } else if let Some(excerpt) = log_excerpt {
                    for line in excerpt.lines() {
                        println!("  {}", line.dimmed());
                    }
                }
            }

            Ok(())
        }

        DaemonCommands::Install => {
            print_header("Installing System Service");

            let (platform, init_system) = get_platform_info();
            println!(
                "{} Platform: {} ({})",
                "→".cyan(),
                platform.bright_white(),
                init_system.bright_white()
            );

            if is_service_installed() {
                println!("{} Service already installed", "⚠".yellow());
                println!(
                    "\n{} Run {} to reinstall",
                    "→".cyan(),
                    "slnky daemon uninstall && slnky daemon install".bright_white()
                );
                return Ok(());
            }

            if cli.dry_run {
                println!("{} Would install system service", "🔍".bright_blue());
                return Ok(());
            }

            let spinner = create_spinner("Installing service...");
            match install_service() {
                Ok(msg) => {
                    spinner.finish_with_message(format!(
                        "{} Service installed and enabled",
                        "✓".green()
                    ));
                    println!("\n{}", msg.dimmed());
                    println!(
                        "\n{} The daemon will now start automatically on login",
                        "→".cyan()
                    );
                    println!(
                        "{} Your dotfiles will stay in sync automatically!",
                        "✨".bright_yellow()
                    );
                }
                Err(e) => {
                    spinner.finish_with_message(format!(
                        "{} Failed to install service: {}",
                        "✗".red(),
                        e
                    ));
                }
            }
            Ok(())
        }

        DaemonCommands::Uninstall => {
            print_header("Uninstalling System Service");

            if !is_service_installed() {
                println!("{} Service is not installed", "→".cyan());
                return Ok(());
            }

            if !cli.yes && !cli.dry_run {
                println!("{} This will disable auto-start on boot", "⚠".yellow());
                if !confirm("Continue?", true)? {
                    println!("{} Cancelled", "→".cyan());
                    return Ok(());
                }
            }

            if cli.dry_run {
                println!("{} Would uninstall system service", "🔍".bright_blue());
                return Ok(());
            }

            let spinner = create_spinner("Uninstalling service...");
            match uninstall_service() {
                Ok(msg) => {
                    spinner.finish_with_message(format!("{} Service uninstalled", "✓".green()));
                    println!("\n{}", msg.dimmed());
                }
                Err(e) => {
                    spinner.finish_with_message(format!(
                        "{} Failed to uninstall service: {}",
                        "✗".red(),
                        e
                    ));
                }
            }
            Ok(())
        }

        DaemonCommands::Logs { lines, follow } => {
            print_header("Daemon Logs");

            if *follow {
                println!(
                    "{} Follow mode not yet implemented. Showing last {} lines:",
                    "⚠".yellow(),
                    lines
                );
            }

            match service_logs(*lines) {
                Ok(content) => {
                    if content.is_empty() || content == "No logs available" {
                        println!("{}", "No logs available".dimmed());
                    } else {
                        println!("{}", content);
                    }
                }
                Err(e) => {
                    println!("{} Failed to read logs: {}", "✗".red(), e);
                }
            }
            Ok(())
        }

        DaemonCommands::Run => run_daemon().map_err(|e| SlinkyError::Other(e.to_string())),
    }
}
