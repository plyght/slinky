use slnky::stow::{analyze_package, execute_operations, find_packages, OpType};
use std::fs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Slinky Stow Engine Demo ===\n");

    let demo_root = std::env::temp_dir().join("slinky_demo");
    let _ = fs::remove_dir_all(&demo_root);
    fs::create_dir_all(&demo_root)?;

    let stow_dir = demo_root.join("dotfiles");
    let target_dir = demo_root.join("home");
    fs::create_dir_all(&stow_dir)?;
    fs::create_dir_all(&target_dir)?;

    println!("1. Creating sample package structure...");
    let nvim_pkg = stow_dir.join("nvim");
    fs::create_dir_all(nvim_pkg.join(".config/nvim"))?;
    fs::write(nvim_pkg.join(".config/nvim/init.lua"), "-- Neovim config\n")?;
    fs::write(nvim_pkg.join(".config/nvim/lazy-lock.json"), "{}\n")?;

    let zsh_pkg = stow_dir.join("zsh");
    fs::create_dir_all(&zsh_pkg)?;
    fs::write(zsh_pkg.join(".zshrc"), "# ZSH config\n")?;
    fs::write(zsh_pkg.join(".zshenv"), "# ZSH env\n")?;
    fs::write(zsh_pkg.join(".stow-local-ignore"), "*.backup\n.DS_Store\n")?;
    fs::write(zsh_pkg.join("notes.backup"), "ignored file\n")?;

    println!("   Created packages: nvim, zsh\n");

    println!("2. Scanning for packages...");
    let packages = find_packages(&stow_dir)?;
    println!("   Found {} package(s):", packages.len());
    for pkg in &packages {
        println!("   - {}", pkg.name);
    }
    println!();

    println!("3. Analyzing nvim package...");
    let nvim_ops = analyze_package(&nvim_pkg, &target_dir)?;
    println!("   Operations planned: {}", nvim_ops.len());
    for op in &nvim_ops {
        match &op.op_type {
            OpType::Create => println!(
                "   [CREATE] {} -> {}",
                op.target.strip_prefix(&target_dir).unwrap().display(),
                op.source.strip_prefix(&stow_dir).unwrap().display()
            ),
            OpType::Remove => println!(
                "   [REMOVE] {}",
                op.target.strip_prefix(&target_dir).unwrap().display()
            ),
            OpType::Skip(reason) => println!(
                "   [SKIP] {}: {}",
                op.target.strip_prefix(&target_dir).unwrap().display(),
                reason
            ),
        }
    }
    println!();

    println!("4. Analyzing zsh package (with ignore rules)...");
    let zsh_ops = analyze_package(&zsh_pkg, &target_dir)?;
    println!("   Operations planned: {}", zsh_ops.len());
    for op in &zsh_ops {
        match &op.op_type {
            OpType::Create => println!(
                "   [CREATE] {} -> {}",
                op.target.strip_prefix(&target_dir).unwrap().display(),
                op.source.strip_prefix(&stow_dir).unwrap().display()
            ),
            OpType::Skip(reason) => println!(
                "   [SKIP] {}: {}",
                op.target.strip_prefix(&target_dir).unwrap().display(),
                reason
            ),
            _ => {}
        }
    }
    println!();

    println!("5. Executing operations (dry-run)...");
    let dry_results = execute_operations(&nvim_ops, true)?;
    for result in &dry_results {
        println!("   {}", result);
    }
    println!();

    println!("6. Executing operations (for real)...");
    let results = execute_operations(&nvim_ops, false)?;
    for result in &results {
        println!("   {}", result);
    }
    println!();

    println!("7. Verifying symlinks...");
    let target_file = target_dir.join(".config/nvim/init.lua");
    if target_file.exists() && target_file.is_symlink() {
        let link_target = fs::read_link(&target_file)?;
        println!(
            "   ✓ {} is a symlink to {}",
            target_file.display(),
            link_target.display()
        );
    } else {
        println!("   ✗ Symlink verification failed");
    }
    println!();

    println!("8. Re-analyzing (should detect existing symlinks)...");
    let reanalyze_ops = analyze_package(&nvim_pkg, &target_dir)?;
    let skip_count = reanalyze_ops
        .iter()
        .filter(|op| matches!(op.op_type, OpType::Skip(_)))
        .count();
    println!("   Operations that will be skipped: {}", skip_count);
    println!();

    println!("=== Demo complete ===");
    println!("Demo files created in: {}", demo_root.display());
    println!("Run 'rm -rf {}' to clean up", demo_root.display());

    Ok(())
}
