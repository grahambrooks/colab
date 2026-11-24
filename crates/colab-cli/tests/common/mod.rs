//! What every end-to-end test binary of the `colab` CLI shares: the binary
//! itself, scratch workspaces, git repositories, and the fake language
//! server. Each test file starts with `mod common; use common::*;`.
// Each binary uses some of these, never all.
#![allow(dead_code, unused_imports)]

pub use std::fs;
pub use std::io::Write;
pub use std::path::PathBuf;
pub use std::process::Command;

pub use assert_cmd::cargo::CommandCargoExt;
pub use serde_json::Value;

pub const SCRIPT: &str = r#"refactor "rename" {
    match go::import "some.module" { replace "new.module" }
}
"#;

pub const GO_INPUT: &str = "package main\n\nimport (\n\t\"fmt\"\n\t\"some.module\"\n)\n";
pub const GO_EXPECTED: &str = "package main\n\nimport (\n\t\"fmt\"\n\t\"new.module\"\n)\n";

pub fn workspace_temp(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "colab-cli-{}-{}-{}",
        label,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn write(path: &std::path::Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let mut f = fs::File::create(path).unwrap();
    f.write_all(contents.as_bytes()).unwrap();
}

/// The colab binary, each command with a language server of its own:
/// these tests drive a fake server whose behaviour on start-up they check.
/// `kept_server_is_the_default` covers the shared one.
pub fn colab() -> Command {
    let mut cmd = Command::cargo_bin("colab").expect("colab binary built");
    cmd.env("COLAB_KEEP_SERVER", "0");
    // A developer's shell (Claude Code's, with a collector configured) may
    // set an OTLP endpoint; tests must not send it traces.
    cmd.env("OTEL_SDK_DISABLED", "true");
    cmd
}

/// The fake language server from `colab-lsp-client`'s examples, built on
/// demand — `cargo test -p colab-cli` does not build another package's
/// examples by itself.
pub fn fake_lsp() -> String {
    static BUILT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    BUILT
        .get_or_init(|| {
            let output = Command::new(env!("CARGO"))
                .args([
                    "build",
                    "-q",
                    "-p",
                    "colab-lsp-client",
                    "--example",
                    "fake_lsp",
                    "--message-format=json",
                ])
                .output()
                .expect("cargo runs");
            assert!(output.status.success(), "building fake_lsp failed");
            String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .filter_map(|l| serde_json::from_str::<Value>(l).ok())
                .filter(|m| m["target"]["name"] == "fake_lsp")
                .find_map(|m| m["executable"].as_str().map(str::to_string))
                .expect("cargo reported the fake_lsp executable")
        })
        .clone()
}

pub fn git(root: &std::path::Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// A committed git repo holding `files`.
pub fn git_workspace(label: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = workspace_temp(label).canonicalize().unwrap();
    for (name, text) in files {
        write(&root.join(name), text);
    }
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    git(&root, &["config", "user.name", "test"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "init"]);
    root
}

pub const INLINE_RULE: &str = r#"match go::import "some.module" { replace "new.module" }"#;

/// Run `colab` with `stdin_text` piped in, returning the finished output.
pub fn run_with_stdin(args: &[&str], stdin_text: &str) -> std::process::Output {
    let mut child = colab()
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(stdin_text.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

// Workspaces the fake language server's tests share.

pub const RS_A: &str = "fn old() {}\nfn main() { old(); }\n";
pub const RS_B: &str = "use crate::old;\n";

pub fn rename_workspace(label: &str) -> PathBuf {
    let root = workspace_temp(label);
    write(&root.join("a.rs"), RS_A);
    write(&root.join("b.rs"), RS_B);
    root
}

pub const RS_MOD: &str = "mod util;\nfn main() { util::go(); }\n";
pub const RS_UTIL: &str = "pub fn go() {}\n";

pub const RS_ACT_A: &str = "use zeta;\nuse alpha;\nfn helper() {}   \nfn unused() {}\nfn main() {\n    let x = 1 + 2;\n    helper();\n}\n";
pub const RS_ACT_B: &str = "fn other() { helper(); }\n";

pub fn action_workspace(label: &str) -> PathBuf {
    let root = workspace_temp(label);
    write(&root.join("a.rs"), RS_ACT_A);
    write(&root.join("b.rs"), RS_ACT_B);
    root
}

/// A command run against the fake language server, from `root`.
pub fn lsp_cmd(root: &std::path::Path, args: &[&str]) -> std::process::Output {
    colab()
        .args(args)
        .arg("--lsp")
        .arg(fake_lsp())
        .args(["--root", ".", "-C"])
        .arg(root)
        .output()
        .unwrap()
}
