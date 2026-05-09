//! End-to-end tests for the `colab` binary's M3 surface:
//!
//! - exit-code table (0 / 4 / 10),
//! - `--check`, `--dry-run`, `--write` semantics,
//! - `--format json` shape and `--format diff` output,
//! - `--stdin --path` pipeline,
//! - discovery commands (`schema`, `list-languages`, `list-rules`,
//!   `explain`).

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

use assert_cmd::cargo::CommandCargoExt;
use serde_json::Value;

const SCRIPT: &str = r#"refactor "rename" {
    match go::import "some.module" { replace "new.module" }
}
"#;

const GO_INPUT: &str = "package main\n\nimport (\n\t\"fmt\"\n\t\"some.module\"\n)\n";
const GO_EXPECTED: &str = "package main\n\nimport (\n\t\"fmt\"\n\t\"new.module\"\n)\n";

fn workspace_temp(label: &str) -> PathBuf {
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

fn write(path: &std::path::Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let mut f = fs::File::create(path).unwrap();
    f.write_all(contents.as_bytes()).unwrap();
}

fn colab() -> Command {
    Command::cargo_bin("colab").expect("colab binary built")
}

#[test]
fn check_exits_10_when_changes_pending() {
    let root = workspace_temp("check-pending");
    let script = root.join("rename.codemod");
    let target = root.join("main.go");
    write(&script, SCRIPT);
    write(&target, GO_INPUT);

    let status = colab()
        .args([
            "refactor",
            "--script",
            script.to_str().unwrap(),
            "--check",
            target.to_str().unwrap(),
        ])
        .status()
        .unwrap();

    assert_eq!(status.code(), Some(10));
    // --check must not have written anything.
    assert_eq!(fs::read_to_string(&target).unwrap(), GO_INPUT);

    fs::remove_dir_all(&root).ok();
}

#[test]
fn check_exits_0_when_no_changes() {
    let root = workspace_temp("check-clean");
    let script = root.join("rename.codemod");
    let target = root.join("main.go");
    write(&script, SCRIPT);
    write(&target, GO_EXPECTED);

    let status = colab()
        .args([
            "refactor",
            "--script",
            script.to_str().unwrap(),
            "--check",
            target.to_str().unwrap(),
        ])
        .status()
        .unwrap();

    assert_eq!(status.code(), Some(0));

    fs::remove_dir_all(&root).ok();
}

#[test]
fn dry_run_does_not_write() {
    let root = workspace_temp("dry-run");
    let script = root.join("rename.codemod");
    let target = root.join("main.go");
    write(&script, SCRIPT);
    write(&target, GO_INPUT);

    let status = colab()
        .args([
            "refactor",
            "--script",
            script.to_str().unwrap(),
            "--dry-run",
            target.to_str().unwrap(),
        ])
        .status()
        .unwrap();

    assert_eq!(status.code(), Some(0));
    assert_eq!(fs::read_to_string(&target).unwrap(), GO_INPUT);

    fs::remove_dir_all(&root).ok();
}

#[test]
fn write_rewrites_in_place() {
    let root = workspace_temp("write");
    let script = root.join("rename.codemod");
    let target = root.join("main.go");
    write(&script, SCRIPT);
    write(&target, GO_INPUT);

    let status = colab()
        .args([
            "refactor",
            "--script",
            script.to_str().unwrap(),
            "--write",
            target.to_str().unwrap(),
        ])
        .status()
        .unwrap();

    assert_eq!(status.code(), Some(0));
    assert_eq!(fs::read_to_string(&target).unwrap(), GO_EXPECTED);

    fs::remove_dir_all(&root).ok();
}

#[test]
fn format_json_emits_one_object_per_file_and_does_not_write() {
    let root = workspace_temp("format-json");
    let script = root.join("rename.codemod");
    let target = root.join("main.go");
    write(&script, SCRIPT);
    write(&target, GO_INPUT);

    let output = colab()
        .args([
            "refactor",
            "--script",
            script.to_str().unwrap(),
            "--format",
            "json",
            target.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    // --format json defaults to dry-run; file unchanged.
    assert_eq!(fs::read_to_string(&target).unwrap(), GO_INPUT);

    let stdout = String::from_utf8(output.stdout).unwrap();
    let value: Value = serde_json::from_str(stdout.trim()).expect("valid JSON line");
    assert_eq!(value["changed"], true);
    assert_eq!(
        value["bytes_before"].as_u64().unwrap(),
        GO_INPUT.len() as u64
    );
    assert_eq!(
        value["bytes_after"].as_u64().unwrap(),
        GO_EXPECTED.len() as u64
    );

    fs::remove_dir_all(&root).ok();
}

#[test]
fn format_diff_emits_unified_diff() {
    let root = workspace_temp("format-diff");
    let script = root.join("rename.codemod");
    let target = root.join("main.go");
    write(&script, SCRIPT);
    write(&target, GO_INPUT);

    let output = colab()
        .args([
            "refactor",
            "--script",
            script.to_str().unwrap(),
            "--format",
            "diff",
            target.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("--- a/"), "got: {}", stdout);
    assert!(stdout.contains("+++ b/"), "got: {}", stdout);
    assert!(stdout.contains("-\t\"some.module\""), "got: {}", stdout);
    assert!(stdout.contains("+\t\"new.module\""), "got: {}", stdout);

    fs::remove_dir_all(&root).ok();
}

#[test]
fn stdin_emits_transformed_source_to_stdout() {
    let root = workspace_temp("stdin");
    let script = root.join("rename.codemod");
    write(&script, SCRIPT);

    let mut child = colab()
        .args([
            "refactor",
            "--script",
            script.to_str().unwrap(),
            "--stdin",
            "--path",
            "main.go",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(GO_INPUT.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8(output.stdout).unwrap(), GO_EXPECTED);

    fs::remove_dir_all(&root).ok();
}

#[test]
fn schema_lists_go_import_replace() {
    let output = colab().arg("schema").output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    let go = &value["languages"][0];
    assert_eq!(go["name"], "go");
    assert_eq!(go["modules"][0]["name"], "import");
    assert_eq!(go["modules"][0]["actions"][0]["name"], "replace");
}

#[test]
fn list_languages_returns_registered_backends() {
    let output = colab().arg("list-languages").output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    let names: Vec<&str> = value["languages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"go"));
}

#[test]
fn list_rules_for_unknown_lang_exits_3() {
    let output = colab().args(["list-rules", "klingon"]).output().unwrap();
    assert_eq!(output.status.code(), Some(3));
}

#[test]
fn explain_returns_parsed_ir() {
    let root = workspace_temp("explain");
    let script = root.join("rename.codemod");
    write(&script, SCRIPT);

    let output = colab()
        .args(["explain", "--script", script.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));

    let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    assert_eq!(value["name"], "rename");
    assert_eq!(value["rules"][0]["namespace"], "go::import");
    assert_eq!(value["rules"][0]["match"], "some.module");
    assert_eq!(value["rules"][0]["action"]["replace"], "new.module");

    fs::remove_dir_all(&root).ok();
}

#[test]
fn parse_error_exits_2() {
    let root = workspace_temp("parse-err");
    let script = root.join("broken.codemod");
    write(&script, "this is not valid syntax");

    let output = colab()
        .args(["explain", "--script", script.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));

    fs::remove_dir_all(&root).ok();
}

#[test]
fn delete_action_round_trips_through_cli() {
    let root = workspace_temp("delete-action");
    let script = root.join("drop.codemod");
    let target = root.join("main.go");
    write(
        &script,
        "refactor \"drop\" { match go::import \"some.module\" { delete } }\n",
    );
    write(&target, GO_INPUT);

    let status = colab()
        .args([
            "refactor",
            "--script",
            script.to_str().unwrap(),
            "--write",
            target.to_str().unwrap(),
        ])
        .status()
        .unwrap();

    assert_eq!(status.code(), Some(0));
    let out = fs::read_to_string(&target).unwrap();
    assert!(!out.contains("some.module"), "got: {out}");
    assert!(out.contains("\"fmt\""));

    fs::remove_dir_all(&root).ok();
}

#[test]
fn ensure_action_is_idempotent_through_cli() {
    let root = workspace_temp("ensure-action");
    let script = root.join("ensure.codemod");
    let target = root.join("main.go");
    write(
        &script,
        "refactor \"e\" { match go::import \"fmt\" { ensure } }\n",
    );
    write(&target, "package main\n\nfunc main() {}\n");

    // First pass: should change.
    let status = colab()
        .args([
            "refactor",
            "--script",
            script.to_str().unwrap(),
            "--write",
            target.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));
    let after_first = fs::read_to_string(&target).unwrap();
    assert!(after_first.contains("import \"fmt\""), "got: {after_first}");

    // Second pass with --check: nothing to do.
    let status2 = colab()
        .args([
            "refactor",
            "--script",
            script.to_str().unwrap(),
            "--check",
            target.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert_eq!(status2.code(), Some(0));

    fs::remove_dir_all(&root).ok();
}

#[test]
fn mcp_server_responds_to_tools_list() {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::process::Stdio;
    use std::time::Duration;

    let mut child = colab()
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn colab mcp");

    let body = br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    {
        let stdin = child.stdin.as_mut().unwrap();
        stdin.write_all(header.as_bytes()).unwrap();
        stdin.write_all(body).unwrap();
        stdin.flush().unwrap();
    }
    // Closing stdin signals EOF, which makes the server exit after
    // handling the queued request.
    drop(child.stdin.take());

    // Wait for the child to exit (with a generous timeout).
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    // Parse the framed response.
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).unwrap();
        assert!(n > 0, "EOF before headers");
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed.strip_prefix("Content-Length:") {
            content_length = value.trim().parse().ok();
        }
    }
    let len = content_length.expect("Content-Length header");
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).unwrap();
    let _ = child.wait_timeout_or_kill(Duration::from_secs(5));

    let response: Value = serde_json::from_slice(&buf).unwrap();
    assert_eq!(response["id"], 1);
    let names: Vec<&str> = response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"colab.schema"));
    assert!(names.contains(&"colab.lint_script"));
    assert!(names.contains(&"colab.preview"));
    assert!(names.contains(&"colab.apply"));
}

/// Tiny helper: wait for child with a timeout (kills on expiry).
trait WaitTimeoutExt {
    fn wait_timeout_or_kill(
        &mut self,
        timeout: std::time::Duration,
    ) -> std::io::Result<()>;
}

impl WaitTimeoutExt for std::process::Child {
    fn wait_timeout_or_kill(
        &mut self,
        timeout: std::time::Duration,
    ) -> std::io::Result<()> {
        let start = std::time::Instant::now();
        loop {
            match self.try_wait()? {
                Some(_) => return Ok(()),
                None if start.elapsed() >= timeout => {
                    let _ = self.kill();
                    return Ok(());
                }
                None => std::thread::sleep(std::time::Duration::from_millis(50)),
            }
        }
    }
}

#[test]
fn missing_script_exits_4() {
    let output = colab()
        .args([
            "refactor",
            "--script",
            "/this/path/does/not/exist.codemod",
            ".",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4));
}
