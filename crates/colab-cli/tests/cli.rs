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
        warnings
            .iter()
            .any(|w| w.as_str().unwrap().contains("matched no files")),
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
    // Config-file backends edit data, not source: they have no imports,
    // symbols or calls. They are named here so a new source backend cannot
    // slip past the floor by accident.
    const CONFIG_FORMATS: &[&str] = &["json", "markdown", "toml"];

    let mut source_backends = 0;
    for lang in value["languages"].as_array().expect("languages") {
        let name = lang["name"].as_str().unwrap();
        if CONFIG_FORMATS.contains(&name) {
            continue;
        }
        source_backends += 1;
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
        assert!(
            modules.contains(&"symbol"),
            "{name} lacks symbol: {modules:?}"
        );
        assert!(modules.contains(&"call"), "{name} lacks call: {modules:?}");
    }
    assert_eq!(source_backends, 12, "source backend count changed");
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
    assert!(
        stderr.contains("unknown module `go::improt`"),
        "got: {stderr}"
    );
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

    // MCP stdio transport: one JSON-RPC message per line.
    {
        let stdin = child.stdin.as_mut().unwrap();
        stdin
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n")
            .unwrap();
        stdin.flush().unwrap();
    }
    // Closing stdin signals EOF, which makes the server exit after
    // handling the queued request.
    drop(child.stdin.take());

    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let n = reader.read_line(&mut line).unwrap();
    assert!(n > 0, "EOF before a response line");
    assert!(
        line.ends_with('\n'),
        "response is not newline-terminated: {line:?}"
    );
    let mut rest = String::new();
    reader.read_to_string(&mut rest).unwrap();
    assert!(
        rest.is_empty(),
        "unexpected output after the response: {rest:?}"
    );
    let _ = child.wait_timeout_or_kill(Duration::from_secs(5));

    let response: Value = serde_json::from_str(&line).unwrap();
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
    fn wait_timeout_or_kill(&mut self, timeout: std::time::Duration) -> std::io::Result<()>;
}

impl WaitTimeoutExt for std::process::Child {
    fn wait_timeout_or_kill(&mut self, timeout: std::time::Duration) -> std::io::Result<()> {
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
    let names: Vec<&str> = packs.iter().map(|p| p["name"].as_str().unwrap()).collect();
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

// ---- Inline rules (`-e` / `--rule`) and `--script -` ----------------------

const INLINE_RULE: &str = r#"match go::import "some.module" { replace "new.module" }"#;

/// Run `colab` with `stdin_text` piped in, returning the finished output.
fn run_with_stdin(args: &[&str], stdin_text: &str) -> std::process::Output {
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

#[test]
fn inline_rule_applies_without_a_script_file() {
    let root = workspace_temp("inline-write");
    let target = root.join("main.go");
    write(&target, GO_INPUT);

    let status = colab()
        .args(["refactor", "-e", INLINE_RULE, "--write"])
        .arg(&root)
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));
    assert_eq!(fs::read_to_string(&target).unwrap(), GO_EXPECTED);

    fs::remove_dir_all(&root).ok();
}

#[test]
fn repeated_inline_rules_run_in_order_and_report_per_rule() {
    let root = workspace_temp("inline-multi");
    write(&root.join("main.go"), GO_INPUT);

    let output = colab()
        .args([
            "refactor",
            "-e",
            INLINE_RULE,
            "--rule",
            r#"match go::import "absent.module" { delete }"#,
            "--format",
            "json",
        ])
        .arg(&root)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    let rules = value["rules"].as_array().unwrap();
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0]["files"], 1);
    assert_eq!(rules[1]["files"], 0);

    fs::remove_dir_all(&root).ok();
}

#[test]
fn script_dash_reads_the_script_from_stdin() {
    let root = workspace_temp("script-stdin");
    let target = root.join("main.go");
    write(&target, GO_INPUT);

    let output = run_with_stdin(
        &[
            "refactor",
            "--script",
            "-",
            "--write",
            root.to_str().unwrap(),
        ],
        SCRIPT,
    );
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(fs::read_to_string(&target).unwrap(), GO_EXPECTED);

    fs::remove_dir_all(&root).ok();
}

#[test]
fn inline_rule_composes_with_stdin_source() {
    let output = run_with_stdin(
        &[
            "refactor",
            "-e",
            INLINE_RULE,
            "--stdin",
            "--path",
            "main.go",
        ],
        GO_INPUT,
    );
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8(output.stdout).unwrap(), GO_EXPECTED);
}

#[test]
fn script_dash_with_stdin_source_is_a_config_error() {
    let output = run_with_stdin(
        &["refactor", "--script", "-", "--stdin", "--path", "main.go"],
        SCRIPT,
    );
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn inline_parse_error_points_at_the_rule_as_typed() {
    let output = colab()
        .args([
            "refactor",
            "-e",
            INLINE_RULE,
            "-e",
            r#"match go::import "a" { replac "b" }"#,
            "--format",
            "json",
            ".",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("line 2, column 24"),
        "error should be anchored to the second -e rule, got: {stderr}"
    );
}

#[test]
fn script_and_rule_are_mutually_exclusive() {
    let output = colab()
        .args(["refactor", "--script", "x.codemod", "-e", INLINE_RULE, "."])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "clap usage error");
}

#[test]
fn explain_accepts_inline_rules() {
    let output = colab()
        .args(["explain", "-e", INLINE_RULE])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    assert_eq!(value["name"], "inline");
    assert_eq!(value["items"][0]["namespace"], "go::import");
}

// ---- `--help-json` ---------------------------------------------------------

#[test]
fn help_json_describes_the_root_command() {
    let output = colab().arg("--help-json").output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    assert_eq!(value["name"], "colab");
    let subcommands: Vec<&str> = value["subcommands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert!(subcommands.contains(&"refactor"), "{subcommands:?}");
}

#[test]
fn help_json_works_on_a_subcommand_with_required_args() {
    let output = colab().args(["refactor", "--help-json"]).output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "required args must not block --help-json"
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    assert_eq!(value["path"], serde_json::json!(["colab", "refactor"]));
    let args = value["args"].as_array().unwrap();
    let rule = args
        .iter()
        .find(|a| a["id"] == "rules")
        .expect("--rule listed");
    assert_eq!(rule["short"], "-e");
    assert_eq!(rule["repeatable"], true);
    let write = args.iter().find(|a| a["id"] == "write").unwrap();
    assert_eq!(write["takes_value"], false);
    assert!(write.get("value_name").is_none());
    assert_eq!(
        value["required_groups"][0]["one_of"],
        serde_json::json!(["script_path", "rules"])
    );
}

// ---- `colab find` / `colab ast` (M18) --------------------------------------

const GO_WRAP: &str = "package main\n\nfunc f() error {\n\treturn errors.Wrap(err, \"read\")\n}\n";

#[test]
fn find_lists_pattern_matches_as_path_line_col() {
    let root = workspace_temp("find-human");
    write(&root.join("main.go"), GO_WRAP);

    let output = colab()
        .args([
            "find",
            "-e",
            "match go::pattern 'errors.Wrap($E, $M)' { delete }",
        ])
        .arg(&root)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("main.go:4:9: errors.Wrap(err, \"read\")"),
        "{stdout}"
    );
    assert!(stdout.contains("1 match(es) in 1 file(s)"), "{stdout}");
    assert_eq!(
        fs::read_to_string(root.join("main.go")).unwrap(),
        GO_WRAP,
        "find never writes"
    );

    fs::remove_dir_all(&root).ok();
}

#[test]
fn find_json_carries_captures_truncation_and_near_misses() {
    let root = workspace_temp("find-json");
    write(&root.join("a.go"), GO_WRAP);
    write(&root.join("b.go"), GO_WRAP);

    let output = colab()
        .args([
            "find",
            "-e",
            "match go::pattern 'errors.Wrap($E, $M)' { delete }",
            "-e",
            r#"match go::call "errors.Wra" { replace_call "x()" }"#,
            "--format",
            "json",
            "--detail",
            "detailed",
            "--max-results",
            "1",
        ])
        .arg(&root)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    assert_eq!(value["summary"]["matches"], 2);
    assert_eq!(value["matches"].as_array().unwrap().len(), 1);
    assert_eq!(value["truncated"], 1);
    assert_eq!(value["matches"][0]["captures"]["M"], "\"read\"");
    assert_eq!(value["rules"][1]["matches"], 0);
    assert_eq!(value["rules"][1]["near"][0]["value"], "errors.Wrap");

    fs::remove_dir_all(&root).ok();
}

#[test]
fn find_locates_non_pattern_rules_by_changed_lines() {
    let root = workspace_temp("find-fallback");
    write(&root.join("main.go"), GO_INPUT);

    let output = colab()
        .args(["find", "-e", INLINE_RULE, "--format", "json"])
        .arg(&root)
        .output()
        .unwrap();
    let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    assert_eq!(value["matches"][0]["line"], 5);
    assert_eq!(value["matches"][0]["text"], "\t\"some.module\"");

    fs::remove_dir_all(&root).ok();
}

#[test]
fn ast_prints_a_pattern_tree_with_its_context() {
    let output = colab()
        .args(["ast", "--lang", "go", "--pattern", "errors.Wrap($E, $M)"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("context: package p"), "{stdout}");
    assert!(stdout.contains("prefilter: errors"), "{stdout}");
    assert!(stdout.contains("identifier  \"$E\""), "{stdout}");
}

#[test]
fn ast_parses_a_snippet_from_stdin_as_json() {
    let output = run_with_stdin(
        &["ast", "--lang", "go", "--stdin", "--format", "json"],
        "package p\nfunc f() { g(1) }\n",
    );
    assert_eq!(output.status.code(), Some(0));
    let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    assert_eq!(value["has_error"], false);
    assert!(
        value["tree"]
            .as_str()
            .unwrap()
            .contains("call_expression  2:12")
    );
}

#[test]
fn invalid_pattern_exits_3_naming_the_attempts() {
    let output = colab()
        .args(["find", "-e", "match go::pattern 'func (' { delete }", "."])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("go::pattern"), "{stderr}");
    assert!(stderr.contains("as-is"), "{stderr}");
}

// ---- LSP over stdio (M19) --------------------------------------------------

/// A `colab server` child process spoken to with LSP `Content-Length`
/// framing.
struct LspClient {
    child: std::process::Child,
    stdout: std::io::BufReader<std::process::ChildStdout>,
}

impl LspClient {
    fn start() -> Self {
        let mut child = colab()
            .arg("server")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let stdout = std::io::BufReader::new(child.stdout.take().unwrap());
        Self { child, stdout }
    }

    fn send(&mut self, message: Value) {
        let body = message.to_string();
        let stdin = self.child.stdin.as_mut().unwrap();
        write!(stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body).unwrap();
        stdin.flush().unwrap();
    }

    fn read(&mut self) -> Value {
        use std::io::{BufRead, Read};
        let mut length = 0usize;
        loop {
            let mut line = String::new();
            self.stdout.read_line(&mut line).unwrap();
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(n) = line.strip_prefix("Content-Length: ") {
                length = n.parse().unwrap();
            }
        }
        let mut body = vec![0u8; length];
        self.stdout.read_exact(&mut body).unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    /// Send a request and return its response, skipping notifications.
    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(
            serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
        );
        loop {
            let message = self.read();
            if message.get("id") == Some(&serde_json::json!(id)) {
                return message;
            }
        }
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(serde_json::json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn lsp_hover_and_workspace_code_action_round_trip() {
    let root = workspace_temp("lsp-roundtrip");
    write(&root.join("main.go"), GO_INPUT);
    let script_path = root.join("fix.codemod");
    write(&script_path, SCRIPT);
    let root_uri = format!("file://{}", root.display());
    let script_uri = format!("file://{}", script_path.display());

    let mut lsp = LspClient::start();
    let init = lsp.request(
        1,
        "initialize",
        serde_json::json!({
            "rootUri": root_uri,
            "capabilities": {"textDocument": {"codeAction": {"resolveSupport": {"properties": ["edit"]}}}}
        }),
    );
    let caps = &init["result"]["capabilities"];
    assert_eq!(caps["hoverProvider"], true);
    assert_eq!(caps["codeActionProvider"]["resolveProvider"], true);
    lsp.notify("initialized", serde_json::json!({}));
    lsp.notify(
        "textDocument/didOpen",
        serde_json::json!({"textDocument": {"uri": script_uri, "languageId": "codemod", "version": 1, "text": SCRIPT}}),
    );

    let hover = lsp.request(
        2,
        "textDocument/hover",
        serde_json::json!({"textDocument": {"uri": script_uri}, "position": {"line": 1, "character": 12}}),
    );
    let markdown = hover["result"]["contents"]["value"].as_str().unwrap();
    assert!(markdown.contains("go::import"), "{markdown}");

    let actions = lsp.request(
        3,
        "textDocument/codeAction",
        serde_json::json!({
            "textDocument": {"uri": script_uri},
            "range": {"start": {"line": 1, "character": 8}, "end": {"line": 1, "character": 8}},
            "context": {"diagnostics": []}
        }),
    );
    let list = actions["result"].as_array().unwrap();
    assert_eq!(list.len(), 2, "rule + whole script: {list:?}");
    assert!(list[0].get("edit").is_none(), "edits are resolved lazily");

    let resolved = lsp.request(4, "codeAction/resolve", list[0].clone());
    let changes = resolved["result"]["edit"]["changes"].as_object().unwrap();
    let (uri, edits) = changes.iter().next().unwrap();
    assert!(uri.ends_with("/main.go"), "{uri}");
    assert_eq!(edits[0]["newText"], GO_EXPECTED);
    assert_eq!(fs::read_to_string(root.join("main.go")).unwrap(), GO_INPUT);

    drop(lsp);
    fs::remove_dir_all(&root).ok();
}

// ---- `colab rename --lsp` (M19) --------------------------------------------

/// The fake language server from `colab-lsp-client`'s examples, built on
/// demand — `cargo test -p colab-cli` does not build another package's
/// examples by itself.
fn fake_lsp() -> String {
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

const RS_A: &str = "fn old() {}\nfn main() { old(); }\n";
const RS_B: &str = "use crate::old;\n";

fn rename_workspace(label: &str) -> PathBuf {
    let root = workspace_temp(label);
    write(&root.join("a.rs"), RS_A);
    write(&root.join("b.rs"), RS_B);
    root
}

#[test]
fn rename_reports_like_a_codemod_and_check_exits_10() {
    let root = rename_workspace("rename-check");
    let output = colab()
        .args([
            "rename", "--symbol", "old", "--in", "a.rs", "--to", "new", "--lsp",
        ])
        .arg(fake_lsp())
        .args(["--root", "."])
        .args(["--format", "json", "--check", "-C"])
        .arg(&root)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(10),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    assert_eq!(value["changed"], serde_json::json!(["a.rs", "b.rs"]));
    assert!(
        value["rules"][0]["rule"]
            .as_str()
            .unwrap()
            .starts_with("lsp rename `old` -> `new`")
    );
    assert_eq!(fs::read_to_string(root.join("a.rs")).unwrap(), RS_A);
    fs::remove_dir_all(&root).ok();
}

#[test]
fn rename_writes_with_a_backup_that_undo_restores() {
    let root = rename_workspace("rename-write");
    let backup = root.join(".backup");
    let status = colab()
        .args(["rename", "--at", "a.rs:1:4", "--to", "new", "--lsp"])
        .arg(fake_lsp())
        .args(["--root", ".", "--write", "--backup"])
        .arg(&backup)
        .arg("-C")
        .arg(&root)
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));
    assert_eq!(
        fs::read_to_string(root.join("a.rs")).unwrap(),
        "fn new() {}\nfn main() { new(); }\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("b.rs")).unwrap(),
        "use crate::new;\n"
    );

    let status = colab()
        .args(["undo", "--from"])
        .arg(&backup)
        .current_dir(&root)
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));
    assert_eq!(fs::read_to_string(root.join("a.rs")).unwrap(), RS_A);
    assert_eq!(fs::read_to_string(root.join("b.rs")).unwrap(), RS_B);
    fs::remove_dir_all(&root).ok();
}

#[test]
fn rename_verify_failure_restores_every_file() {
    let root = rename_workspace("rename-verify");
    let output = colab()
        .args(["rename", "--at", "a.rs:1:4", "--to", "new", "--lsp"])
        .arg(fake_lsp())
        .args(["--root", ".", "--verify", "false", "-C"])
        .arg(&root)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("restored 2 file(s)"));
    assert_eq!(fs::read_to_string(root.join("a.rs")).unwrap(), RS_A);
    assert_eq!(fs::read_to_string(root.join("b.rs")).unwrap(), RS_B);
    fs::remove_dir_all(&root).ok();
}

#[test]
fn rename_without_a_default_server_asks_for_lsp() {
    let root = workspace_temp("rename-nodefault");
    write(&root.join("notes.txt"), "old\n");
    let output = colab()
        .args([
            "rename",
            "--symbol",
            "old",
            "--in",
            "notes.txt",
            "--to",
            "new",
            "-C",
        ])
        .arg(&root)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("pass --lsp"));
    fs::remove_dir_all(&root).ok();
}

// ---------- M24: git-aware file operations ------------------------------

fn git(root: &std::path::Path, args: &[&str]) -> String {
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
fn git_workspace(label: &str, files: &[(&str, &str)]) -> PathBuf {
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

/// `colab rename` against the fake in `file-op` mode, which also renames
/// the symbol's file to `<new>.rs` — like a Java class rename.
fn file_op_rename(root: &std::path::Path, extra: &[&str]) -> std::process::Output {
    colab()
        .env("FAKE_LSP_MODE", "file-op")
        .args(["rename", "--at", "a.rs:1:4", "--to", "new", "--lsp"])
        .arg(fake_lsp())
        .args(["--root", "."])
        .args(extra)
        .arg("-C")
        .arg(root)
        .output()
        .unwrap()
}

#[test]
fn a_rename_that_moves_a_file_does_it_with_git_and_undo_reverses_it() {
    let root = git_workspace("fileops-rename", &[("a.rs", RS_A), ("b.rs", RS_B)]);

    let output = file_op_rename(&root, &["--format", "json", "--check"]);
    assert_eq!(
        output.status.code(),
        Some(10),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["moved"],
        serde_json::json!([{"from": "a.rs", "to": "new.rs"}])
    );
    assert_eq!(value["summary"]["moved"], 1);
    assert!(root.join("a.rs").exists(), "--check changes nothing");

    let backup = workspace_temp("fileops-rename-backup");
    let output = file_op_rename(&root, &["--write", "--backup", backup.to_str().unwrap()]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!root.join("a.rs").exists());
    assert_eq!(
        fs::read_to_string(root.join("new.rs")).unwrap(),
        "fn new() {}\nfn main() { new(); }\n"
    );
    // The index holds the move; the work tree holds the edits.
    let status = git(&root, &["status", "--porcelain"]);
    assert!(status.contains("RM a.rs -> new.rs"), "{status}");
    assert!(status.contains(" M b.rs"), "{status}");

    let status = colab()
        .args(["undo", "--from"])
        .arg(&backup)
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));
    assert_eq!(
        git(&root, &["status", "--porcelain"]),
        "",
        "undo leaves a clean tree"
    );
    assert_eq!(fs::read_to_string(root.join("a.rs")).unwrap(), RS_A);
    fs::remove_dir_all(&root).ok();
    fs::remove_dir_all(&backup).ok();
}

#[test]
fn commit_moves_keeps_the_history_of_a_moved_and_rewritten_file() {
    let root = git_workspace("fileops-follow", &[("a.rs", RS_A), ("b.rs", RS_B)]);
    // Without --commit-moves, the rewrite is flagged as a history risk.
    let output = file_op_rename(&root, &["--format", "json", "--dry-run"]);
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        value["warnings"][0]
            .as_str()
            .unwrap()
            .contains("--commit-moves"),
        "{value}"
    );

    let output = file_op_rename(&root, &["--write", "--commit-moves"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        git(&root, &["log", "-1", "--format=%s"]).trim(),
        "colab: move a.rs -> new.rs"
    );
    // The edits are left for the user to commit.
    git(&root, &["commit", "-q", "-am", "rename old to new"]);
    let history = git(&root, &["log", "--follow", "--format=%s", "--", "new.rs"]);
    assert_eq!(
        history.lines().collect::<Vec<_>>(),
        ["rename old to new", "colab: move a.rs -> new.rs", "init"]
    );
    fs::remove_dir_all(&root).ok();
}

#[test]
fn file_operations_refuse_dirty_paths_and_a_tree_outside_git() {
    let root = git_workspace("fileops-dirty", &[("a.rs", RS_A), ("b.rs", RS_B)]);
    write(&root.join("b.rs"), "use crate::old; // wip\n");
    let output = file_op_rename(&root, &["--dry-run"]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("uncommitted changes") && stderr.contains("b.rs"),
        "{stderr}"
    );
    let output = file_op_rename(&root, &["--dry-run", "--allow-dirty"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_dir_all(&root).ok();

    // Outside git the server is told it may not move files, and a move it
    // sends anyway is refused.
    let root = rename_workspace("fileops-nogit");
    let output = file_op_rename(&root, &["--dry-run"]);
    assert_eq!(output.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&output.stderr).contains("git work tree"));
    let output = colab()
        .args(["mv", "--no-lsp", "a.rs", "z.rs", "--write", "-C"])
        .arg(&root)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert!(root.join("a.rs").exists());
    fs::remove_dir_all(&root).ok();
}

const RS_MOD: &str = "mod util;\nfn main() { util::go(); }\n";
const RS_UTIL: &str = "pub fn go() {}\n";

#[test]
fn mv_updates_references_and_prints_a_patch_git_applies() {
    let root = git_workspace("fileops-mv", &[("main.rs", RS_MOD), ("util.rs", RS_UTIL)]);
    let mv = |extra: &[&str]| {
        colab()
            .args(["mv", "util.rs", "helpers.rs", "--lsp"])
            .arg(fake_lsp())
            .args(["--root", "."])
            .args(extra)
            .arg("-C")
            .arg(&root)
            .output()
            .unwrap()
    };

    let output = mv(&["--format", "diff"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let patch = String::from_utf8(output.stdout).unwrap();
    assert!(
        patch.contains("similarity index 100%\nrename from util.rs\nrename to helpers.rs"),
        "{patch}"
    );
    let patch_file = root.join("../mv.patch");
    fs::write(&patch_file, &patch).unwrap();
    git(
        &root,
        &["apply", "--check", "--index", patch_file.to_str().unwrap()],
    );
    fs::remove_file(&patch_file).ok();

    let output = mv(&["--write"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(root.join("main.rs")).unwrap(),
        "mod helpers;\nfn main() { helpers::go(); }\n"
    );
    let status = git(&root, &["status", "--porcelain"]);
    assert!(status.contains("R  util.rs -> helpers.rs"), "{status}");
    fs::remove_dir_all(&root).ok();
}

#[test]
fn mv_says_when_the_server_cannot_update_references_and_verify_reverts_it() {
    let root = git_workspace(
        "fileops-mv-nowill",
        &[("main.rs", RS_MOD), ("util.rs", RS_UTIL)],
    );
    let output = colab()
        .env("FAKE_LSP_MODE", "no-will-rename")
        .args(["mv", "util.rs", "sub/", "--lsp"])
        .arg(fake_lsp())
        .args(["--root", ".", "--format", "json", "--dry-run", "-C"])
        .arg(&root)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        value["warnings"][0]
            .as_str()
            .unwrap()
            .contains("does not update references"),
        "{value}"
    );

    // A failing --verify puts everything back, including the directory
    // the move had to create.
    let output = colab()
        .args(["mv", "util.rs", "lib/util.rs", "--lsp"])
        .arg(fake_lsp())
        .args(["--root", ".", "--verify", "false", "-C"])
        .arg(&root)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("undid 1 file operation(s)"));
    assert_eq!(git(&root, &["status", "--porcelain"]), "");
    assert!(
        !root.join("lib").exists(),
        "the directory the move made is removed again"
    );
    fs::remove_dir_all(&root).ok();
}

// ---------- M25: code actions, references, safe delete, tidy -------------

const RS_ACT_A: &str = "use zeta;\nuse alpha;\nfn helper() {}   \nfn unused() {}\nfn main() {\n    let x = 1 + 2;\n    helper();\n}\n";
const RS_ACT_B: &str = "fn other() { helper(); }\n";

fn action_workspace(label: &str) -> PathBuf {
    let root = workspace_temp(label);
    write(&root.join("a.rs"), RS_ACT_A);
    write(&root.join("b.rs"), RS_ACT_B);
    root
}

fn lsp_cmd(root: &std::path::Path, args: &[&str]) -> std::process::Output {
    colab()
        .args(args)
        .arg("--lsp")
        .arg(fake_lsp())
        .args(["--root", ".", "-C"])
        .arg(root)
        .output()
        .unwrap()
}

#[test]
fn action_lists_and_picks_by_kind_and_title() {
    let root = action_workspace("action-pick");
    let output = lsp_cmd(
        &root,
        &[
            "action",
            "--at",
            "a.rs:6:13-6:18",
            "--list",
            "--format",
            "json",
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let titles: Vec<&str> = value["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        titles,
        [
            "Extract into variable",
            "Uppercase file",
            "Convert to async",
            "Organize imports"
        ]
    );
    assert_eq!(value["actions"][2]["disabled"], "not a function");

    // A selection + a dotted-prefix kind; the edit needs codeAction/resolve.
    let output = lsp_cmd(
        &root,
        &[
            "action",
            "--at",
            "a.rs:6:13-6:18",
            "--kind",
            "refactor.extract",
            "--write",
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        fs::read_to_string(root.join("a.rs"))
            .unwrap()
            .contains("    let extracted = 1 + 2;\n    let x = extracted;\n")
    );

    // Several matches, or only a disabled one: an error naming what exists.
    let output = lsp_cmd(
        &root,
        &[
            "action",
            "--at",
            "a.rs:1:1",
            "--kind",
            "refactor.rewrite",
            "--title",
            "e",
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "only one enabled action matches"
    );
    let output = lsp_cmd(
        &root,
        &[
            "action",
            "--at",
            "a.rs:1:1",
            "--title",
            "async",
            "--dry-run",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no action matched") && stderr.contains("disabled: not a function"),
        "{stderr}"
    );
    fs::remove_dir_all(&root).ok();
}

#[test]
fn a_command_action_is_applied_from_the_edit_the_server_sends_back() {
    let root = action_workspace("action-command");
    let output = lsp_cmd(
        &root,
        &[
            "action",
            "--at",
            "a.rs:1:1",
            "--title",
            "uppercase",
            "--format",
            "json",
            "--check",
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(10),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["changed"], serde_json::json!(["a.rs"]));
    assert!(
        value["rules"][0]["rule"]
            .as_str()
            .unwrap()
            .contains("Uppercase file")
    );
    assert_eq!(
        fs::read_to_string(root.join("a.rs")).unwrap(),
        RS_ACT_A,
        "--check writes nothing"
    );
    fs::remove_dir_all(&root).ok();
}

#[test]
fn references_are_listed_with_and_without_the_declaration() {
    let root = action_workspace("references");
    let output = lsp_cmd(&root, &["references", "--symbol", "helper", "--in", "a.rs"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("a.rs:7:5: helper();"), "{stdout}");
    assert!(
        stdout.contains("b.rs:1:14: fn other() { helper(); }"),
        "{stdout}"
    );
    assert!(stdout.contains("2 reference(s) in 2 file(s)"), "{stdout}");

    let output = lsp_cmd(
        &root,
        &[
            "references",
            "--at",
            "a.rs:3:4",
            "--include-declaration",
            "--format",
            "json",
            "--max-results",
            "2",
        ],
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["summary"]["references"], 3);
    assert_eq!(value["references"].as_array().unwrap().len(), 2);
    assert_eq!(value["truncated"], 1);
    fs::remove_dir_all(&root).ok();
}

#[test]
fn safe_delete_refuses_a_used_symbol_and_deletes_an_unused_one_then_tidies() {
    let root = action_workspace("safe-delete");
    let output = lsp_cmd(
        &root,
        &[
            "safe-delete",
            "--symbol",
            "helper",
            "--in",
            "a.rs",
            "--format",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["deleted"], false);
    assert_eq!(value["summary"]["references"], 2);
    assert_eq!(fs::read_to_string(root.join("a.rs")).unwrap(), RS_ACT_A);

    let output = lsp_cmd(
        &root,
        &[
            "safe-delete",
            "--symbol",
            "unused",
            "--in",
            "a.rs",
            "--tidy",
            "--format",
            "json",
            "--write",
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        value["rules"][0]["rule"]
            .as_str()
            .unwrap()
            .starts_with("safe delete `unused`")
    );
    assert!(
        value["rules"][1]["rule"]
            .as_str()
            .unwrap()
            .starts_with("tidy via")
    );
    assert_eq!(value["rules"][1]["files"], 1);
    // Deleted, imports sorted, trailing whitespace gone.
    assert_eq!(
        fs::read_to_string(root.join("a.rs")).unwrap(),
        "use alpha;\nuse zeta;\nfn helper() {}\nfn main() {\n    let x = 1 + 2;\n    helper();\n}\n"
    );
    fs::remove_dir_all(&root).ok();
}

#[test]
fn mcp_serves_the_language_server_tools() {
    let root = action_workspace("mcp-lsp");
    let lsp = fake_lsp();
    let call = |id: u32, name: &str, args: Value| {
        serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": name, "arguments": args}})
        .to_string()
    };
    let cwd = root.to_str().unwrap();
    let input = [
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}).to_string(),
        call(2, "colab.references", serde_json::json!({"symbol": "helper", "in": "a.rs", "lsp": lsp, "root": ".", "cwd": cwd})),
        call(3, "colab.rename", serde_json::json!({"symbol": "helper", "in": "a.rs", "to": "assist", "lsp": lsp, "root": ".", "cwd": cwd})),
        call(4, "colab.action", serde_json::json!({"at": "a.rs:1:1", "title": "nope", "lsp": lsp, "root": ".", "cwd": cwd})),
    ]
    .join("\n")
        + "\n";
    let mut child = colab()
        .arg("mcp")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let replies: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let names: Vec<&str> = replies[0]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for tool in [
        "colab.references",
        "colab.rename",
        "colab.action",
        "colab.preview",
    ] {
        assert!(names.contains(&tool), "{tool} in {names:?}");
    }
    let text = |i: usize| -> Value {
        serde_json::from_str(replies[i]["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
    };
    assert_eq!(text(1)["summary"]["references"], 2);
    assert_eq!(text(2)["changed"], serde_json::json!(["a.rs", "b.rs"]));
    assert_eq!(text(2)["written"], false);
    assert_eq!(
        fs::read_to_string(root.join("b.rs")).unwrap(),
        RS_ACT_B,
        "preview only"
    );
    assert_eq!(replies[3]["result"]["isError"], true);
    assert!(
        text(3)["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no action matched")
    );
    fs::remove_dir_all(&root).ok();
}

#[test]
fn write_refuses_a_conflicting_rename_and_exits_11() {
    let root = workspace_temp("conflict-refused");
    let target = root.join("main.go");
    let input = "package main\n\nfunc load() {}\n\nfunc parse() {}\n";
    write(&target, input);

    let output = colab()
        .args([
            "refactor",
            "--write",
            "--format",
            "json",
            "-e",
            r#"match go::symbol "load" { replace "parse" }"#,
            target.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(11));
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        input,
        "nothing written"
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let conflict = &json["conflicts"][0];
    assert_eq!(conflict["rule"], 0);
    assert!(
        conflict["message"]
            .as_str()
            .unwrap()
            .contains("`parse` already occurs"),
        "{json}"
    );
    assert!(
        json["warnings"][0]
            .as_str()
            .unwrap()
            .starts_with("wrote nothing")
    );

    let status = colab()
        .args([
            "refactor",
            "--write",
            "--allow-conflicts",
            "-e",
            r#"match go::symbol "load" { replace "parse" }"#,
            target.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));
    assert!(!fs::read_to_string(&target).unwrap().contains("load"));

    fs::remove_dir_all(&root).ok();
}

#[test]
fn a_requires_import_clears_the_unimported_qualifier_conflict() {
    let root = workspace_temp("conflict-qualifier");
    let target = root.join("main.go");
    let input = "package main\n\nfunc f() error {\n\treturn errors.Wrap(err, \"read\")\n}\n";
    write(&target, input);
    let rule = |clause: &str| {
        format!(
            r#"match go::pattern 'errors.Wrap($E, $M)' {{ replace 'fmt.Errorf($M+": %w", $E)' {clause} }}"#
        )
    };

    let output = colab()
        .args([
            "refactor",
            "--write",
            "--format",
            "json",
            "-e",
            &rule(""),
            target.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(11));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        json["conflicts"][0]["message"]
            .as_str()
            .unwrap()
            .contains("`fmt.`"),
        "{json}"
    );

    let status = colab()
        .args([
            "refactor",
            "--write",
            "-e",
            &rule(r#"requires import "fmt""#),
            target.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));
    assert!(fs::read_to_string(&target).unwrap().contains("\"fmt\""));

    fs::remove_dir_all(&root).ok();
}

#[test]
fn verify_lsp_refuses_a_change_that_adds_a_server_error() {
    let root = workspace_temp("verify-lsp");
    let target = root.join("a.rs");
    let input = "fn old() {}\n\nfn main() {\n    old();\n}\n";
    write(&target, input);
    let run = |to: &str| {
        colab()
            .current_dir(&root)
            .args(["refactor", "--write", "--format", "json"])
            .arg(format!("--verify-lsp={}", fake_lsp()))
            .args([
                "-e",
                &format!(r#"match rust::symbol "old" {{ replace "{to}" }}"#),
                "a.rs",
            ])
            .output()
            .unwrap()
    };

    // The fake server reports an error on every line containing BROKEN.
    let output = run("BROKEN");
    assert_eq!(
        output.status.code(),
        Some(11),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), input);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let conflicts = json["conflicts"].as_array().unwrap();
    assert_eq!(conflicts.len(), 2, "{json}");
    assert!(
        conflicts[0].get("rule").is_none(),
        "a server error is the file's, not a rule's"
    );
    assert!(
        conflicts[0]["message"]
            .as_str()
            .unwrap()
            .contains("reports a new error at line 1")
    );

    let output = run("fresh");
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(fs::read_to_string(&target).unwrap().contains("fn fresh()"));

    fs::remove_dir_all(&root).ok();
}
