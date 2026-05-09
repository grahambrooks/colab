use std::env;
use std::process::Command;

fn main() {
    let git_sha = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .expect("Failed to execute git command")
        .stdout;

    let git_sha = String::from_utf8_lossy(&git_sha).trim().to_string();

    let out_dir = env::var("OUT_DIR").unwrap();
    let version_file = format!("{}/version.txt", out_dir);

    std::fs::write(&version_file, git_sha).expect("Failed to write version file");

    println!("cargo:rustc-env=VERSION_FILE={}", version_file);
}
