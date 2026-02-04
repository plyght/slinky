use slnky::stow::{analyze_package, execute_operations, find_packages, OpType};
use std::fs;

#[test]
fn test_stow_workflow() {
    let test_root = std::env::temp_dir().join("slinky_integration_test");
    let _ = fs::remove_dir_all(&test_root);
    fs::create_dir_all(&test_root).unwrap();

    let stow_dir = test_root.join("dotfiles");
    let target_dir = test_root.join("home");
    fs::create_dir_all(&stow_dir).unwrap();
    fs::create_dir_all(&target_dir).unwrap();

    let package_path = stow_dir.join("nvim");
    fs::create_dir_all(package_path.join(".config/nvim")).unwrap();
    fs::write(package_path.join(".config/nvim/init.lua"), "-- nvim config").unwrap();

    let packages = find_packages(&stow_dir).unwrap();
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].name, "nvim");

    let operations = analyze_package(&package_path, &target_dir).unwrap();
    assert_eq!(operations.len(), 1);
    assert!(matches!(operations[0].op_type, OpType::Create));

    let results = execute_operations(&operations, false).unwrap();
    assert_eq!(results.len(), 1);

    let target_file = target_dir.join(".config/nvim/init.lua");
    assert!(target_file.exists());
    assert!(target_file.is_symlink());

    fs::remove_dir_all(&test_root).unwrap();
}

#[test]
fn test_stow_with_ignore() {
    let test_root = std::env::temp_dir().join("slinky_ignore_test");
    let _ = fs::remove_dir_all(&test_root);
    fs::create_dir_all(&test_root).unwrap();

    let package_path = test_root.join("package");
    let target_dir = test_root.join("target");
    fs::create_dir_all(&package_path).unwrap();
    fs::create_dir_all(&target_dir).unwrap();

    fs::write(package_path.join("keep.txt"), "keep this").unwrap();
    fs::write(package_path.join("ignore.tmp"), "ignore this").unwrap();
    fs::write(package_path.join(".stow-local-ignore"), "*.tmp\n").unwrap();

    let operations = analyze_package(&package_path, &target_dir).unwrap();

    let create_count = operations
        .iter()
        .filter(|op| matches!(op.op_type, OpType::Create))
        .count();
    let skip_count = operations
        .iter()
        .filter(|op| matches!(op.op_type, OpType::Skip(_)))
        .count();

    assert_eq!(create_count, 1);
    assert_eq!(skip_count, 1);

    fs::remove_dir_all(&test_root).unwrap();
}

#[test]
fn test_stow_dry_run() {
    let test_root = std::env::temp_dir().join("slinky_dryrun_test");
    let _ = fs::remove_dir_all(&test_root);
    fs::create_dir_all(&test_root).unwrap();

    let package_path = test_root.join("package");
    let target_dir = test_root.join("target");
    fs::create_dir_all(&package_path).unwrap();
    fs::create_dir_all(&target_dir).unwrap();

    fs::write(package_path.join("test.txt"), "content").unwrap();

    let operations = analyze_package(&package_path, &target_dir).unwrap();
    let results = execute_operations(&operations, true).unwrap();

    assert_eq!(results.len(), 1);
    assert!(results[0].contains("[DRY-RUN]"));

    let target_file = target_dir.join("test.txt");
    assert!(!target_file.exists());

    fs::remove_dir_all(&test_root).unwrap();
}
