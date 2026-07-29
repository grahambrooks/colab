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
fn format_json_emits_one_document_and_does_not_write() {
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
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "expected a single document, got: {stdout}");

    let doc: Value = serde_json::from_str(lines[0]).expect("valid JSON");
    assert_eq!(doc["summary"]["scanned"], 1);
    assert_eq!(doc["summary"]["changed"], 1);
    assert_eq!(
        doc["summary"]["bytes_before"].as_u64().unwrap(),
        GO_INPUT.len() as u64
    );
    assert_eq!(
        doc["summary"]["bytes_after"].as_u64().unwrap(),
        GO_EXPECTED.len() as u64
    );
    assert!(doc["summary"]["elapsed_ms"].is_u64());

    // Per-rule attribution, and the changed path — but no diff at the
    // default `counts` detail.
    assert_eq!(doc["rules"][0]["i"], 0);
    assert_eq!(doc["rules"][0]["files"], 1);
    assert_eq!(doc["changed"].as_array().unwrap().len(), 1);
    assert!(doc.get("diffs").is_none(), "got: {stdout}");
    assert!(doc.get("warnings").is_none(), "got: {stdout}");

    fs::remove_dir_all(&root).ok();
}

#[test]
fn json_detail_diff_adds_capped_diffs() {
    let root = workspace_temp("format-json-diff");
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
            "--detail",
            "diff",
            target.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));

    let stdout = String::from_utf8(output.stdout).unwrap();
    let doc: Value = serde_json::from_str(stdout.trim()).expect("valid JSON");
    let diff = doc["diffs"][0]["diff"].as_str().expect("a diff");
    assert!(diff.contains("@@"), "got: {diff}");
}

#[test]
fn a_rule_that_matches_nothing_is_reported_as_dead() {
    let root = workspace_temp("dead-rule");
    let script = root.join("dead.codemod");
    let target = root.join("main.go");
    write(
        &script,
        "refactor \"dead\" {\n  match go::import \"nowhere/at/all\" { replace \"x\" }\n}\n",
    );
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

    let stdout = String::from_utf8(output.stdout).unwrap();
    let doc: Value = serde_json::from_str(stdout.trim()).expect("valid JSON");
    assert_eq!(doc["rules"][0]["files"], 0);
    let warnings = doc["warnings"].as_array().expect("warnings");
    assert!(
        warnings.iter().any(|w| w
            .as_str()
            .unwrap()
            .contains("matched no files")),
        "got: {stdout}"
    );
    // And the summary explains which of the three empty-result causes
    // this was: files were scanned, the rule just did not match.
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap().contains("no rule matched")),
        "got: {stdout}"
    );

    fs::remove_dir_all(&root).ok();
}

#[test]
fn summary_only_suppresses_per_file_detail() {
    let root = workspace_temp("summary-only");
    let script = root.join("rename.codemod");
    let a = root.join("a.go");
    let b = root.join("b.go");
    write(&script, SCRIPT);
    write(&a, GO_INPUT);
    write(&b, GO_INPUT);

    let output = colab()
        .args([
            "refactor",
            "--script",
            script.to_str().unwrap(),
            "--format",
            "json",
            "--summary-only",
            root.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));

    let stdout = String::from_utf8(output.stdout).unwrap();
    let doc: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(doc["summary"]["scanned"], 2);
    assert_eq!(doc["summary"]["changed"], 2);
    // `summary` detail carries the counters and nothing else.
    assert!(doc.get("rules").is_none(), "got: {stdout}");
    assert!(doc.get("changed").is_none(), "got: {stdout}");

    fs::remove_dir_all(&root).ok();
}

#[test]
fn ndjson_streams_changed_files_then_a_summary() {
    let root = workspace_temp("format-ndjson");
    let script = root.join("rename.codemod");
    let changed = root.join("a.go");
    let untouched = root.join("b.go");
    write(&script, SCRIPT);
    write(&changed, GO_INPUT);
    write(&untouched, "package demo\n");

    let output = colab()
        .args([
            "refactor",
            "--script",
            script.to_str().unwrap(),
            "--format",
            "ndjson",
            root.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));

    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    // The unchanged file produces no event at all.
    assert_eq!(lines.len(), 2, "got: {stdout}");

    let file_event: Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(file_event["type"], "file");
    assert_eq!(file_event["rules"][0], 0);

    let summary: Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(summary["type"], "summary");
    assert_eq!(summary["summary"]["scanned"], 2);
    assert_eq!(summary["summary"]["changed"], 1);
    assert_eq!(summary["rules"][0]["files"], 1);

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
    // Look the language up by name rather than by position — registration
    // order changes whenever a backend is added.
    let go = value["languages"]
        .as_array()
        .expect("languages array")
        .iter()
        .find(|l| l["name"] == "go")
        .expect("go backend registered");
    let import = go["modules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "import")
        .expect("go::import module");
    let actions: Vec<&str> = import["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    assert!(actions.contains(&"replace"), "got: {actions:?}");
}

#[test]
fn every_backend_advertises_import_symbol_and_call() {
    // The capability floor: whatever the language, an agent can rely on
    // an import-equivalent, a symbol rename, and call rewriting existing.
    let output = colab().arg("schema").output().unwrap();
    let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");

    // The import-equivalent module is named for the language's own
    // construct, so accept any of these.
    const IMPORT_MODULES: &[&str] = &["import", "include", "use", "using", "require"];

    for lang in value["languages"].as_array().expect("languages") {
        let name = lang["name"].as_str().unwrap();
        let modules: Vec<&str> = lang["modules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["name"].as_str().unwrap())
            .collect();

        assert!(
            modules.iter().any(|m| IMPORT_MODULES.contains(m)),
            "{name} has no import-equivalent module: {modules:?}"
        );
        assert!(modules.contains(&"symbol"), "{name} lacks symbol: {modules:?}");
        assert!(modules.contains(&"call"), "{name} lacks call: {modules:?}");
    }
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
    assert_eq!(value["items"][0]["kind"], "match");
    assert_eq!(value["items"][0]["namespace"], "go::import");
    assert_eq!(value["items"][0]["match"], "some.module");
    // One shape for every action: a name plus an optional value.
    assert_eq!(value["items"][0]["action"], "replace");
    assert_eq!(value["items"][0]["value"], "new.module");
    assert!(value["items"][0]["scope"].is_null());

    fs::remove_dir_all(&root).ok();
}

#[test]
fn explain_reports_an_in_scope_clause() {
    let root = workspace_temp("explain-scope");
    let script = root.join("scoped.codemod");
    write(
        &script,
        "refactor \"scoped\" {\n  \
         match rust::symbol \"Config\" in \"crates/colab-core/**\" { replace \"CoreConfig\" }\n\
         }\n",
    );

    let output = colab()
        .args(["explain", "--script", script.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));

    let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    assert_eq!(value["items"][0]["scope"], "crates/colab-core/**");

    fs::remove_dir_all(&root).ok();
}

#[test]
fn a_parse_error_reports_line_column_and_expected_tokens() {
    let root = workspace_temp("parse-detail");
    let script = root.join("broken.codemod");
    write(
        &script,
        "refactor \"x\" {\n  match go::import \"a\" { replac \"b\" }\n}\n",
    );

    let output = colab()
        .args(["explain", "--script", script.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));

    // The CLI reports errors on stderr; the position must be a line and
    // column, not the parser's raw byte offset.
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("line 2, column 26"), "got: {stderr}");
    assert!(stderr.contains("expected one of"), "got: {stderr}");
    assert!(stderr.contains("\"replace\""), "got: {stderr}");

    fs::remove_dir_all(&root).ok();
}

#[test]
fn an_unknown_module_names_the_closest_real_one() {
    let root = workspace_temp("typo-module");
    let script = root.join("typo.codemod");
    write(
        &script,
        "refactor \"x\" {\n  match go::improt \"a\" { replace \"b\" }\n}\n",
    );

    let output = colab()
        .args([
            "refactor",
            "--script",
            script.to_str().unwrap(),
            "--dry-run",
            root.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("unknown module `go::improt`"), "got: {stderr}");
    assert!(stderr.contains("did you mean `import`?"), "got: {stderr}");
    // And it no longer leaks the Rust Debug form of RuleSpec.
    assert!(!stderr.contains("RuleSpec"), "got: {stderr}");
    assert!(!stderr.contains("Replace {"), "got: {stderr}");

    fs::remove_dir_all(&root).ok();
}

#[test]
fn an_unsupported_action_names_the_action_and_the_valid_ones() {
    let root = workspace_temp("bad-action");
    let script = root.join("bad.codemod");
    // `rust::crate` supports replace and delete but not ensure: adding a
    // dependency needs a version, which the DSL has no way to express.
    write(
        &script,
        "refactor \"x\" {\n  match rust::crate \"serde\" { ensure }\n}\n",
    );

    let output = colab()
        .args([
            "refactor",
            "--script",
            script.to_str().unwrap(),
            "--dry-run",
            root.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("does not support the `ensure` action"),
        "got: {stderr}"
    );
    assert!(stderr.contains("known actions:"), "got: {stderr}");

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
fn include_glob_restricts_processed_files() {
    let root = workspace_temp("include-glob");
    let script = root.join("rename.codemod");
    write(&script, SCRIPT);
    let want = root.join("src/main.go");
    let nope = root.join("vendor/main.go");
    write(&want, GO_INPUT);
    write(&nope, GO_INPUT);

    let status = colab()
        .args([
            "refactor",
            "--script",
            script.to_str().unwrap(),
            "--write",
            "--include",
            "src/**",
            root.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));

    // src/main.go was rewritten; vendor/main.go was not.
    assert_eq!(fs::read_to_string(&want).unwrap(), GO_EXPECTED);
    assert_eq!(fs::read_to_string(&nope).unwrap(), GO_INPUT);

    fs::remove_dir_all(&root).ok();
}

#[test]
fn exclude_glob_skips_matching_files() {
    let root = workspace_temp("exclude-glob");
    let script = root.join("rename.codemod");
    write(&script, SCRIPT);
    let kept = root.join("main.go");
    let skipped = root.join("vendor/main.go");
    write(&kept, GO_INPUT);
    write(&skipped, GO_INPUT);

    let status = colab()
        .args([
            "refactor",
            "--script",
            script.to_str().unwrap(),
            "--write",
            "--exclude",
            "vendor/**",
            root.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));

    assert_eq!(fs::read_to_string(&kept).unwrap(), GO_EXPECTED);
    assert_eq!(fs::read_to_string(&skipped).unwrap(), GO_INPUT);

    fs::remove_dir_all(&root).ok();
}

#[test]
fn gitignore_default_skips_ignored_files() {
    let root = workspace_temp("gitignore-default");
    fs::create_dir_all(root.join(".git")).unwrap();
    write(&root.join(".gitignore"), "vendor/\n");
    let script = root.join("rename.codemod");
    write(&script, SCRIPT);
    let kept = root.join("main.go");
    let ignored = root.join("vendor/main.go");
    write(&kept, GO_INPUT);
    write(&ignored, GO_INPUT);

    let status = colab()
        .args([
            "refactor",
            "--script",
            script.to_str().unwrap(),
            "--write",
            root.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));

    assert_eq!(fs::read_to_string(&kept).unwrap(), GO_EXPECTED);
    // vendor/main.go is gitignored; should be untouched.
    assert_eq!(fs::read_to_string(&ignored).unwrap(), GO_INPUT);

    fs::remove_dir_all(&root).ok();
}

#[test]
fn no_ignore_visits_gitignored_files() {
    let root = workspace_temp("no-ignore");
    fs::create_dir_all(root.join(".git")).unwrap();
    write(&root.join(".gitignore"), "vendor/\n");
    let script = root.join("rename.codemod");
    write(&script, SCRIPT);
    let in_vendor = root.join("vendor/main.go");
    write(&in_vendor, GO_INPUT);

    let status = colab()
        .args([
            "refactor",
            "--script",
            script.to_str().unwrap(),
            "--write",
            "--no-ignore",
            root.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));

    assert_eq!(fs::read_to_string(&in_vendor).unwrap(), GO_EXPECTED);

    fs::remove_dir_all(&root).ok();
}

#[test]
fn parallel_walk_produces_identical_results_across_jobs_settings() {
    // Build a wider tree, run with --jobs 1 and --jobs 8, compare
    // the resulting files. Both must converge on the same output.
    let make_tree = |label: &str| {
        let root = workspace_temp(label);
        let script = root.join("rename.codemod");
        write(&script, SCRIPT);
        for dir in 0..4 {
            for file in 0..6 {
                let path = root.join(format!("dir{}/{:02}.go", dir, file));
                write(&path, GO_INPUT);
            }
        }
        (root, script)
    };

    let (root1, script1) = make_tree("parallel-j1");
    let status = colab()
        .args([
            "refactor",
            "--script",
            script1.to_str().unwrap(),
            "--write",
            "--jobs",
            "1",
            root1.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));

    let (root8, script8) = make_tree("parallel-j8");
    let status = colab()
        .args([
            "refactor",
            "--script",
            script8.to_str().unwrap(),
            "--write",
            "--jobs",
            "8",
            root8.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));

    // Compare every file in both trees — they must match.
    for dir in 0..4 {
        for file in 0..6 {
            let rel = format!("dir{}/{:02}.go", dir, file);
            let a = fs::read_to_string(root1.join(&rel)).unwrap();
            let b = fs::read_to_string(root8.join(&rel)).unwrap();
            assert_eq!(a, b, "mismatch at {}", rel);
            assert_eq!(a, GO_EXPECTED, "wrong content at {}", rel);
        }
    }

    fs::remove_dir_all(&root1).ok();
    fs::remove_dir_all(&root8).ok();
}

#[test]
fn verify_passes_through_when_command_succeeds() {
    let root = workspace_temp("verify-ok");
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
            "--verify",
            "true",
            target.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));
    // Rule applied because verify succeeded.
    assert_eq!(fs::read_to_string(&target).unwrap(), GO_EXPECTED);

    fs::remove_dir_all(&root).ok();
}

#[test]
fn verify_reverts_and_errors_when_command_fails() {
    let root = workspace_temp("verify-fail");
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
            "--verify",
            "false",
            target.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    // Error → exit code 1 (Config) per the documented mapping.
    assert_eq!(status.code(), Some(1));
    // File reverted to its original contents.
    assert_eq!(fs::read_to_string(&target).unwrap(), GO_INPUT);

    fs::remove_dir_all(&root).ok();
}

#[test]
fn commit_per_rule_produces_one_commit_per_rule() {
    let root = workspace_temp("commit-per-rule");

    // Initialise a git repo with a baseline commit so `git
    // commit` succeeds.
    let git = |args: &[&str]| -> std::process::ExitStatus {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&root)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .status()
            .unwrap()
    };
    assert!(git(&["init", "-q", "-b", "main"]).success());
    assert!(git(&["config", "user.name", "test"]).success());
    assert!(git(&["config", "user.email", "test@example.com"]).success());
    let script = root.join("rules.codemod");
    let target = root.join("main.go");
    // Two rules; both will fire on the same file.
    write(
        &script,
        r#"refactor "two-rules" {
            match go::import "some.module" { replace "mid.module" }
            match go::import "mid.module" { replace "new.module" }
        }
        "#,
    );
    write(&target, GO_INPUT);
    assert!(git(&["add", "main.go"]).success());
    assert!(git(&["commit", "-q", "-m", "baseline"]).success());

    let status = colab()
        .args([
            "refactor",
            "--script",
            script.to_str().unwrap(),
            "--write",
            "--commit-per-rule",
            target.to_str().unwrap(),
        ])
        .current_dir(&root)
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));

    let log = std::process::Command::new("git")
        .args(["log", "--pretty=%s"])
        .current_dir(&root)
        .output()
        .unwrap();
    let messages = String::from_utf8_lossy(&log.stdout);
    let lines: Vec<&str> = messages.lines().collect();
    // Two colab commits + the baseline = 3 entries.
    assert_eq!(lines.len(), 3, "git log:\n{messages}");
    assert!(lines[0].starts_with("colab: "), "got: {}", lines[0]);
    assert!(lines[1].starts_with("colab: "), "got: {}", lines[1]);
    assert_eq!(lines[2], "baseline");

    // File reflects both rules applied in order.
    let final_text = fs::read_to_string(&target).unwrap();
    assert!(final_text.contains("\"new.module\""), "got: {final_text}");

    fs::remove_dir_all(&root).ok();
}

#[test]
fn bisect_passes_when_full_apply_verifies() {
    let root = workspace_temp("bisect-pass");
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
            "--bisect",
            "true",
            target.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));
    // Full apply succeeded; final state has the rename applied.
    assert_eq!(fs::read_to_string(&target).unwrap(), GO_EXPECTED);

    fs::remove_dir_all(&root).ok();
}

#[test]
fn bisect_isolates_breaking_rule_and_reverts() {
    // Three rules. Rule 2 introduces the substring "BAD" (which
    // rule 3 doesn't fully clean up — the substring survives into
    // "another.BAD.module"), so the full-apply state matches the
    // verify pattern and triggers bisection. Bisect should
    // identify rule 2 as the culprit.
    let root = workspace_temp("bisect-find");
    let script = root.join("rules.codemod");
    let target = root.join("main.go");
    write(
        &script,
        r#"refactor "chain" {
            match go::import "some.module" { replace "step1.module" }
            match go::import "step1.module" { replace "BAD.module" }
            match go::import "BAD.module" { replace "another.BAD.module" }
        }
        "#,
    );
    write(&target, GO_INPUT);

    // Verify fails (exit non-zero) when "BAD" is found in the file.
    let target_str = target.to_str().unwrap().to_string();
    let verify_cmd = format!("! grep -q 'BAD' {}", target_str);

    let output = colab()
        .args([
            "refactor",
            "--script",
            script.to_str().unwrap(),
            "--write",
            "--bisect",
            verify_cmd.as_str(),
            target.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    // The error message names rule 2 (the one that first introduces
    // "BAD"). Its Display form contains the from→to mapping.
    assert!(
        stderr.contains("step1.module") && stderr.contains("BAD.module"),
        "stderr: {stderr}"
    );
    // Working tree was reverted to original.
    assert_eq!(fs::read_to_string(&target).unwrap(), GO_INPUT);

    fs::remove_dir_all(&root).ok();
}

#[test]
fn backup_and_undo_roundtrip() {
    let root = workspace_temp("backup-undo");
    let backup = workspace_temp("backup-store");
    let script = root.join("rename.codemod");
    let target = root.join("main.go");
    write(&script, SCRIPT);
    write(&target, GO_INPUT);

    // Apply with --backup; file should change, backup should hold
    // the pre-state.
    let status = colab()
        .args([
            "refactor",
            "--script",
            script.to_str().unwrap(),
            "--write",
            "--backup",
            backup.to_str().unwrap(),
            target.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));
    assert_eq!(fs::read_to_string(&target).unwrap(), GO_EXPECTED);

    // Now undo — file should go back to its original state.
    let status = colab()
        .args(["undo", "--from", backup.to_str().unwrap()])
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));
    assert_eq!(fs::read_to_string(&target).unwrap(), GO_INPUT);

    fs::remove_dir_all(&root).ok();
    fs::remove_dir_all(&backup).ok();
}

#[test]
fn pack_list_finds_repo_packs() {
    // Build a fake "repo" with a .git marker and a populated
    // .colab/packs directory, then cd into it so the discovery
    // walk finds the pack.
    let root = workspace_temp("pack-list");
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::create_dir_all(root.join(".colab/packs")).unwrap();
    write(
        &root.join(".colab/packs/example.codemod"),
        "refactor \"x\" {}\n",
    );

    let output = colab()
        .arg("pack")
        .arg("list")
        .current_dir(&root)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let v: Value = serde_json::from_slice(&output.stdout).unwrap();
    let packs = v["packs"].as_array().unwrap();
    let names: Vec<&str> = packs
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"example"), "got: {names:?}");
    let sources: Vec<&str> = packs
        .iter()
        .map(|p| p["source"].as_str().unwrap())
        .collect();
    assert!(sources.contains(&"repo"));

    fs::remove_dir_all(&root).ok();
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
