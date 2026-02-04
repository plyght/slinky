use clap::Parser;
use colored::*;
use std::process;

mod cli;
mod config;
mod daemon;
mod error;
mod remote;
mod secrets;
mod service;
mod stow;

use cli::Cli;

fn main() {
    let cli = Cli::parse();

    if let Err(e) = cli::run(cli) {
        eprintln!("\n{} {}", "✗".red().bold(), e.to_string().bright_red());
        process::exit(1);
    }
}
