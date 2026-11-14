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

    // Re-run when HEAD moves, or the embedded suffix names an old commit:
    // `HEAD` changes on checkout, the branch's ref file (or `packed-refs`)
    // on commit. A linked worktree keeps HEAD in its own git dir and refs
    // in the common one.
    if let Some(git_dir) = git(&["rev-parse", "--path-format=absolute", "--git-dir"]) {
        println!("cargo:rerun-if-changed={git_dir}/HEAD");
    }
    if let Some(common) = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"]) {
        println!("cargo:rerun-if-changed={common}/packed-refs");
        if let Some(head_ref) = git(&["symbolic-ref", "-q", "HEAD"]) {
            println!("cargo:rerun-if-changed={common}/{head_ref}");
        }
    }
}

/// The trimmed output of a successful, non-empty `git` command.
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !text.is_empty()).then_some(text)
}
