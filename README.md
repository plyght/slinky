# Slinky

A modern dotfiles manager with stow-compatible symlink management, remote repository support, and secret encryption.

## Overview

Slinky streamlines dotfile management by combining the simplicity of GNU Stow's symlink paradigm with modern features like Git repository integration and age-based secret encryption. It enables you to organize configuration files into packages, sync them across machines via remote repositories, and securely manage sensitive data within your dotfiles.

## Features

- **Stow-Compatible Symlink Management**: Organize dotfiles into packages with automatic symlink creation
- **Remote Repository Support**: Clone and manage dotfiles from GitHub, GitLab, or any Git remote
- **Secret Encryption**: Scan and encrypt sensitive data using age encryption with passphrase protection
- **Dry Run Mode**: Preview all operations before execution with `--dry-run`
- **Conflict Detection**: Identifies existing files that would be overwritten during linking
- **Flexible Configuration**: TOML-based configuration with sensible defaults
- **Package Discovery**: Automatically detects packages in cloned repositories

## Installation

```bash
# From source
git clone https://github.com/nicojaffer/slinky.git
cd slinky
cargo build --release
cp target/release/slnky /usr/local/bin/

# Using Cargo
cargo install slnky
```

## Usage

```bash
# Clone a dotfiles repository and discover packages
slnky install user/repo
slnky install github.com/user/repo

# Link a package to create symlinks
slnky link nvim
slnky link zsh --target ~/

# Unlink a package to remove symlinks
slnky unlink nvim

# Show all available packages
slnky status

# Scan a file for potential secrets
slnky secrets scan ~/.zshrc

# Encrypt detected secrets in shell configs
slnky secrets encrypt
```

All commands support global flags:
- `--verbose`: Show detailed output
- `--dry-run`: Preview changes without applying
- `--target <DIR>`: Override target directory

## Configuration

Slinky uses `~/.config/slinky/config.toml` for configuration:

```toml
stow_dir = "/Users/username/.dotfiles"
target_dir = "/Users/username"
packages = ["nvim", "zsh", "tmux"]
secrets_enabled = true
```

Configuration is created automatically with defaults on first run. The `stow_dir` contains your dotfile packages, and `target_dir` is where symlinks are created (typically your home directory).

## Secret Management

Slinky detects common secret patterns (API keys, tokens, passwords) in shell configuration files and encrypts them using age:

1. **Scan**: Identifies potential secrets using regex patterns
2. **Template**: Creates `.template` files with placeholders for secrets
3. **Encrypt**: Stores encrypted secrets in `~/.config/slinky/secrets.age`
4. **Decrypt**: Retrieves secrets with passphrase when needed

This allows you to commit template files to version control while keeping actual secrets encrypted locally.

## Architecture

- `cli.rs`: Command-line interface with clap, progress indicators, and formatted output
- `config.rs`: TOML configuration loading, defaults, and persistence
- `error.rs`: Typed error variants using thiserror
- `remote.rs`: Git operations for cloning and updating repositories from multiple providers
- `secrets.rs`: Regex-based secret detection and age encryption/decryption
- `stow.rs`: Symlink analysis, conflict detection, and filesystem operations

## Development

```bash
cargo build
cargo test
cargo run -- link nvim --dry-run
```

Requires Rust 1.70+. Key dependencies: age (encryption), clap (CLI), colored (output), regex (pattern matching), serde/toml (configuration).

## License

MIT License
