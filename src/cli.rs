use clap::{Parser, Subcommand};
use colored::*;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::PathBuf;

use crate::config::{load_config, Config};
use crate::error::{Result, SlinkyError};
use crate::remote::{clone_or_update, parse_repo_spec};
use crate::secrets::{create_template, encrypt_secrets, scan_file_for_secrets, scan_shell_configs};
use crate::stow::{analyze_package, execute_operations, find_packages};

#[derive(Parser)]
#[command(
    name = "slnky",
    version,
    author,
    about = "🔗 A blazingly fast dotfiles manager with secret encryption",
    long_about = None
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    #[arg(long, global = true, help = "Show detailed output")]
    pub verbose: bool,

    #[arg(long, global = true, help = "Preview changes without applying")]
    pub dry_run: bool,

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
    #[command(about = "Clone a repository and link its packages", alias = "i")]
    Install {
        #[arg(help = "Repository (e.g., user/repo, github.com/user/repo, https://...)")]
        repo: String,
    },

    #[command(about = "Link a package to the target directory", alias = "l")]
    Link {
        #[arg(help = "Package name to link")]
        package: String,
    },

    #[command(about = "Unlink a package from the target directory", alias = "u")]
    Unlink {
        #[arg(help = "Package name to unlink")]
        package: String,
    },

    #[command(about = "Show all linked packages and their status", alias = "s")]
    Status,

    #[command(about = "Secret management commands")]
    Secrets {
        #[command(subcommand)]
        command: SecretsCommands,
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

pub fn run(cli: Cli) -> Result<()> {
    let config = load_config().unwrap_or_else(|_| Config::default());

    match &cli.command {
        Commands::Install { repo } => install_repo(repo, &cli, &config),
        Commands::Link { package } => link_package(package, &cli, &config),
        Commands::Unlink { package } => unlink_package(package, &cli, &config),
        Commands::Status => show_status(&cli, &config),
        Commands::Secrets { command } => match command {
            SecretsCommands::Scan { file } => scan_secrets(file, &cli),
            SecretsCommands::Encrypt => encrypt_all_secrets(&cli, &config),
        },
    }
}

fn install_repo(repo: &str, cli: &Cli, _config: &Config) -> Result<()> {
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

    if cli.dry_run {
        println!(
            "{} Would clone: {}",
            "🔍".bright_blue(),
            repo.bright_white()
        );
        return Ok(());
    }

    let spinner = create_spinner("Cloning repository...");
    let repo_path = clone_or_update(&repo_spec).map_err(|e| SlinkyError::Remote(e.to_string()))?;
    spinner.finish_with_message(format!(
        "{} Repository cloned to {}",
        "✓".green(),
        repo_path.display().to_string().bright_white()
    ));

    let packages = find_packages(&repo_path).map_err(|e| SlinkyError::Stow(e.to_string()))?;

    if packages.is_empty() {
        println!("{} No packages found in repository", "⚠".yellow());
        return Ok(());
    }

    println!(
        "\n{} Found {} package(s)",
        "✓".green(),
        packages.len().to_string().bright_white()
    );

    for package in packages {
        println!("  {} {}", "•".bright_blue(), package.name.bright_white());
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
    }

    let package_path = config.stow_dir.join(package);
    if !package_path.exists() {
        return Err(SlinkyError::PackageNotFound(package.to_string()));
    }

    let operations =
        analyze_package(&package_path, &target).map_err(|e| SlinkyError::Stow(e.to_string()))?;

    if operations.is_empty() {
        println!("{} Package already linked", "✓".green());
        return Ok(());
    }

    if cli.dry_run {
        println!(
            "{} Would perform {} operation(s):",
            "🔍".bright_blue(),
            operations.len().to_string().bright_white()
        );
        for op in operations {
            println!(
                "  {} {}",
                "•".bright_blue(),
                format!("{:?}", op).bright_white()
            );
        }
        return Ok(());
    }

    let spinner = create_spinner(&format!("Linking {}...", package));
    execute_operations(&operations, false).map_err(|e| SlinkyError::Stow(e.to_string()))?;
    spinner.finish_with_message(format!(
        "{} Package {} linked successfully",
        "✓".green(),
        package.bright_white()
    ));

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

    if cli.dry_run {
        println!(
            "{} Would unlink package: {}",
            "🔍".bright_blue(),
            package.bright_white()
        );
        return Ok(());
    }

    let spinner = create_spinner(&format!("Unlinking {}...", package));
    spinner.finish_with_message(format!(
        "{} Package {} unlinked successfully",
        "✓".green(),
        package.bright_white()
    ));

    Ok(())
}

fn show_status(cli: &Cli, config: &Config) -> Result<()> {
    print_header("Package Status");

    let packages = find_packages(&config.stow_dir).map_err(|e| SlinkyError::Stow(e.to_string()))?;

    if packages.is_empty() {
        println!("{} No packages found", "⚠".yellow());
        return Ok(());
    }

    println!(
        "{} {} package(s) available:\n",
        "✓".green(),
        packages.len().to_string().bright_white()
    );

    for package in packages {
        let status = if cli.verbose { "linked" } else { "available" };
        println!(
            "  {} {} {}",
            "•".bright_blue(),
            package.name.bright_white(),
            format!("({})", status).dimmed()
        );
    }

    Ok(())
}

fn scan_secrets(file: &PathBuf, cli: &Cli) -> Result<()> {
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
