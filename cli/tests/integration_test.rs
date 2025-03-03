use std::fs;
use std::path::Path;
use std::process::Command;

#[test]
fn test_go_module_dependency_update() {
    // Set up test directory and files
    let test_dir = Path::new("test_data");
    let go_file_path = test_dir.join("main.go");
    let config_file_path = test_dir.join("config.yaml");

    fs::create_dir_all(test_dir).expect("Failed to create test directory");

    // Write test Go file
    let go_file_content = r#"
        package main

        import (
            "fmt"
            "some.module"
        )

        func main() {
            fmt.Println("Hello, world!")
        }
    "#;
    fs::write(&go_file_path, go_file_content).expect("Failed to write test Go file");

    // Write test config file
    let config_file_content = r#"
        replace:
            go-module:
                from: some.module
                to: another.module
    "#;
    fs::write(&config_file_path, config_file_content).expect("Failed to write test config file");

    // Run the CLI tool
    let output = Command::new("./target/release/cli")
        .arg("--config")
        .arg(config_file_path.to_str().unwrap())
        .arg("--path")
        .arg(test_dir.to_str().unwrap())
        .output()
        .expect("Failed to execute CLI tool");

    assert!(output.status.success());

    // Verify the Go file has been updated
    let updated_go_file_content = fs::read_to_string(&go_file_path).expect("Failed to read updated Go file");
    assert!(updated_go_file_content.contains("another.module"));
    assert!(!updated_go_file_content.contains("some.module"));

    // Clean up test directory and files
    fs::remove_dir_all(test_dir).expect("Failed to remove test directory");
}
