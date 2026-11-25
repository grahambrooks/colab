//! The MCP server's tests. They live in the crate, not in `tests/`, because
//! they drive private tool functions (`run_script`, `find`, `test_rule`).

use super::*;

/// Rewrites `path` on disk at the first tick: someone saving the file
/// after the walk read it and before the run writes it.
struct EditsMidRun<'a> {
    path: &'a Path,
    text: &'a str,
}

impl ProgressSink for EditsMidRun<'_> {
    fn tick(&mut self, _processed: u64) {
        std::fs::write(self.path, self.text).unwrap();
    }
    fn finish(&mut self, _total: u64) {}
}

#[test]
fn generalize_learns_a_rule_from_one_edit_and_finds_the_others() {
    let dir = std::env::temp_dir().join(format!("colab-mcp-generalize-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("a.go"),
        "package p\n\nimport (\n\t\"github.com/stretchr/testify/assert\"\n\t\"github.com/stretchr/testify/require\"\n)\n\nfunc a() {\n\tassert.Equal(t, 1, got)\n}\n\nfunc b() {\n\tassert.Equal(t, want, f(x))\n}\n",
    )
    .unwrap();
    // The agent changed one call by hand: the first one, before the edit.
    let learned = crate::rules::generalize(
        &json!({
            "lang": "go",
            "before": "assert.Equal(t, 1, got)",
            "after": "require.Equal(t, got, 1)",
            "paths": ["."],
            "cwd": dir.to_string_lossy()
        }),
        &registry(),
    )
    .unwrap();
    assert_eq!(learned["rule"]["match"], "assert.Equal($A, $B, $C)");
    assert_eq!(learned["rule"]["replace"], "require.Equal($A, $C, $B)");
    assert_eq!(learned["matches"]["summary"]["matches"], 2, "{learned}");
    // The rule goes straight to colab.apply.
    let arguments =
        json!({"rules": [learned["rule"]], "paths": ["."], "cwd": dir.to_string_lossy()});
    run_script(
        &arguments,
        &registry(),
        &NoTools,
        RunMode::Apply,
        &mut NoProgress,
    )
    .unwrap();
    assert!(
        std::fs::read_to_string(dir.join("a.go"))
            .unwrap()
            .contains("require.Equal(t, f(x), want)")
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn rules_run_as_a_script_does_and_the_reply_holds_the_script() {
    let dir = std::env::temp_dir().join(format!("colab-mcp-rules-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("main.go");
    std::fs::write(
        &file,
        "package main\n\nfunc main() {\n\tlog.Print(\"a \\\"q\\\"\")\n\tdebug(1)\n}\n",
    )
    .unwrap();
    // Quotes and backslashes in the values need no DSL quoting.
    let arguments = json!({
        "rules": [
            {"lang": "go", "match": "log.Print($M)", "replace": "log.Println($M)"},
            {"lang": "go", "match": "debug($$$A)", "delete": true,
             "where": [{"inside": "func main() { $$$B }"}]}
        ],
        "paths": ["."],
        "cwd": dir.to_string_lossy()
    });
    let value = run_script(
        &arguments,
        &registry(),
        &NoTools,
        RunMode::Apply,
        &mut NoProgress,
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "package main\n\nfunc main() {\n\tlog.Println(\"a \\\"q\\\"\")\n}\n"
    );
    let script = value["script"].as_str().expect("the generated script");
    assert!(
        script.contains("match go::pattern \"log.Print($M)\""),
        "{script}"
    );
    // The saved script compiles to the same rules.
    assert_eq!(
        colab_dsl::compile(script, &registry()).unwrap().rules.len(),
        2
    );
    // A malformed rule is a tool error naming the rule.
    let bad = json!({"rules": [{"lang": "go", "match": "f()"}], "paths": ["."]});
    let err = run_script(
        &bad,
        &registry(),
        &NoTools,
        RunMode::Preview,
        &mut NoProgress,
    )
    .unwrap_err();
    assert!(format!("{err:?}").contains("rules[0]: give exactly one action"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn apply_merges_a_file_changed_after_the_walk_read_it() {
    let dir = std::env::temp_dir().join(format!("colab-mcp-stale-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("main.go");
    let read = "package main\n\nimport \"old/pkg\"\n\nfunc main() {}\n";
    let script = "refactor \"r\" { match go::import \"old/pkg\" { replace \"new/pkg\" } }";
    let arguments = json!({"script": script, "paths": ["."], "cwd": dir.to_string_lossy()});

    // Their edit is far from ours: both survive.
    std::fs::write(&file, read).unwrap();
    let theirs = "package main\n\nimport \"old/pkg\"\n\nfunc main() { run() }\n";
    let mut sink = EditsMidRun {
        path: &file,
        text: theirs,
    };
    let value = run_script(&arguments, &registry(), &NoTools, RunMode::Apply, &mut sink).unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "package main\n\nimport \"new/pkg\"\n\nfunc main() { run() }\n"
    );
    assert!(value.to_string().contains("merged with the run's changes"));

    // Their edit is on our line: a conflict, and their text stands.
    std::fs::write(&file, read).unwrap();
    let theirs = "package main\n\nimport \"other/pkg\"\n\nfunc main() {}\n";
    let mut sink = EditsMidRun {
        path: &file,
        text: theirs,
    };
    let value = run_script(&arguments, &registry(), &NoTools, RunMode::Apply, &mut sink).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), theirs);
    assert!(
        value
            .to_string()
            .contains("changed on disk after colab read it")
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

fn registry() -> BackendRegistry {
    colab_backends::registry()
}

#[test]
fn preview_and_apply_run_a_script_with_scans() {
    let dir = std::env::temp_dir().join(format!("colab-mcp-scan-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("api")).unwrap();
    std::fs::create_dir_all(dir.join("app")).unwrap();
    std::fs::write(
        dir.join("api/api.go"),
        "package api\n\nfunc Exported() {}\n\nfunc internal() {}\n",
    )
    .unwrap();
    let main = dir.join("app/main.go");
    std::fs::write(
        &main,
        "package main\n\nfunc main() {\n\tapi.Exported()\n\tapi.internal()\n}\n",
    )
    .unwrap();
    let script = r#"refactor "s" {
    scan go::pattern 'func $F() {}' where $F matches /^[A-Z]/ as exported
    match go::pattern 'api.$F()' where $F in exported { replace 'api.$F(nil)' }
}"#;
    let arguments = json!({"script": script, "paths": ["."], "cwd": dir.to_string_lossy()});
    let preview = run_script(
        &arguments,
        &registry(),
        &NoTools,
        RunMode::Preview,
        &mut NoProgress,
    )
    .unwrap();
    assert_eq!(preview["summary"]["changed"], 1, "{preview}");
    // colab.find reads the same facts: only the exported call. A tool
    // call is a run of its own (`handle_call`); the walk's workers join it.
    let found = colab_core::run::Run::new()
        .enter(|| find(&arguments, &registry()))
        .unwrap();
    assert_eq!(found["summary"]["matches"], 1, "{found}");
    assert!(found.to_string().contains("\"line\":4"), "{found}");
    // colab.test_rule's scans read its one snippet.
    let tested = test_rule(
        &json!({
            "script": script,
            "path": "x.go",
            "source": "package x\n\nfunc Exported() {}\n\nfunc main() { api.Exported(); api.internal() }\n",
        }),
        &registry(),
    )
    .unwrap();
    assert!(
        tested["output"]
            .as_str()
            .unwrap()
            .contains("api.Exported(nil); api.internal()"),
        "{tested}"
    );
    run_script(
        &arguments,
        &registry(),
        &NoTools,
        RunMode::Apply,
        &mut NoProgress,
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(&main).unwrap(),
        "package main\n\nfunc main() {\n\tapi.Exported(nil)\n\tapi.internal()\n}\n"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A host whose one check finds a conflict in every changed file.
struct Objects;

struct ObjectToAll;

impl colab_dsl::pipeline::Check for ObjectToAll {
    fn verify(
        &mut self,
        _rules: &[Box<dyn colab_core::Operation>],
        changes: &[colab_core::FileChange],
        report: &mut RunReport,
    ) -> colab_core::Result<()> {
        for change in changes {
            report.add_conflict(colab_core::report::Conflict {
                path: change.path.clone(),
                rule: None,
                message: "the host objects".into(),
            });
        }
        Ok(())
    }
}

impl ToolProvider for Objects {
    fn tools(&self) -> Vec<Value> {
        Vec::new()
    }

    fn call(&self, _name: &str, _args: &Value) -> Option<colab_core::Result<Value>> {
        None
    }

    fn checks(&self, _roots: &[PathBuf]) -> Vec<Box<dyn colab_dsl::pipeline::Check>> {
        vec![Box::new(ObjectToAll)]
    }
}

/// A host whose project lists `my_assert` as an expression macro.
struct ListsMacros;

impl ToolProvider for ListsMacros {
    fn tools(&self) -> Vec<Value> {
        Vec::new()
    }

    fn call(&self, _name: &str, _args: &Value) -> Option<colab_core::Result<Value>> {
        None
    }

    fn macros(&self, _cwd: Option<&Path>) -> Vec<String> {
        vec!["my_assert".into()]
    }
}

#[test]
fn a_tool_call_reads_the_hosts_expression_macros() {
    let dir = std::env::temp_dir().join(format!("colab-mcp-macros-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("lib.rs"), "fn t() {\n    my_assert!(old(1));\n}\n").unwrap();
    let req = json!({
        "jsonrpc":"2.0","id":7,"method":"tools/call",
        "params":{"name": "colab.find", "arguments": {
            "script": "match rust::pattern 'old($X)' { delete }",
            "paths": ["."], "cwd": dir.to_string_lossy()
        }}
    });
    let found = |extra: &dyn ToolProvider| {
        let resp = handle_with(&req, &registry(), extra).unwrap();
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        serde_json::from_str::<Value>(text).unwrap()["summary"]["matches"].clone()
    };
    assert_eq!(found(&NoTools), 0);
    assert_eq!(found(&ListsMacros), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn apply_runs_the_hosts_checks_before_it_writes() {
    let dir = std::env::temp_dir().join(format!("colab-mcp-checks-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("main.go");
    let text = "package main\n\nimport \"old/pkg\"\n\nfunc main() {}\n";
    std::fs::write(&file, text).unwrap();
    let script = "match go::import \"old/pkg\" { replace \"new/pkg\" }";
    let arguments = json!({"script": script, "paths": ["."], "cwd": dir.to_string_lossy()});

    let value = run_script(
        &arguments,
        &registry(),
        &Objects,
        RunMode::Apply,
        &mut NoProgress,
    )
    .unwrap();
    assert_eq!(value["applied"], false, "{value}");
    assert!(value.to_string().contains("the host objects"), "{value}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text);

    // `no_link_check` turns the host's checks off.
    let mut skip = arguments.clone();
    skip["no_link_check"] = json!(true);
    let value = run_script(
        &skip,
        &registry(),
        &Objects,
        RunMode::Apply,
        &mut NoProgress,
    )
    .unwrap();
    assert_eq!(value["applied"], true, "{value}");
    assert!(std::fs::read_to_string(&file).unwrap().contains("new/pkg"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_rule_that_needs_a_language_server_is_refused() {
    let script = r#"refactor "t" { match go::pattern '$F($$$A)' where $F resolves "alog.Print" { delete } }"#;
    let e = compile_script(script, None, &registry())
        .err()
        .unwrap()
        .to_string();
    assert!(
        e.contains("language server") && e.contains("colab CLI"),
        "{e}"
    );
}

#[test]
fn initialize_returns_protocol_version_and_tool_capability() {
    let req = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}});
    let resp = handle(&req, &registry()).unwrap();
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["result"]["protocolVersion"], PROTOCOL_VERSION);
    assert!(resp["result"]["capabilities"]["tools"].is_object());
    let instructions = resp["result"]["instructions"].as_str().unwrap();
    assert!(instructions.contains("instead of sed"), "{instructions}");
}

#[test]
fn instructions_name_only_tools_the_server_has() {
    // The binary adds the LSP and Markdown tools; name those here.
    let injected = [
        "colab.rename_symbol",
        "colab.rename",
        "colab.edit_symbol",
        "colab.outline",
        "colab.section",
    ];
    let known: Vec<String> = tools::list()
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_string))
        .chain(injected.iter().map(ToString::to_string))
        .collect();
    for word in INSTRUCTIONS.split(|c: char| !(c.is_alphanumeric() || c == '.' || c == '_')) {
        let word = word.trim_end_matches('.');
        if word.starts_with("colab.") {
            assert!(known.iter().any(|k| k == word), "{word} is not a tool");
        }
    }
}

#[test]
fn a_bare_rule_needs_no_refactor_block() {
    let ok = call_ok(
        "colab.test_rule",
        json!({
            "script": "match rust::pattern 'f($A)' { replace 'g($A)' }",
            "source": "fn m() { f(1); }\n",
            "path": "m.rs",
        }),
    );
    assert_eq!(ok["changed"], true, "{ok}");

    let req = json!({
        "jsonrpc":"2.0","id":41,"method":"tools/call",
        "params":{"name":"colab.test_rule","arguments": {
            "script": "match rust::pattern 'f($A)' { nope }",
            "source": "fn m() {}\n",
            "path": "m.rs",
        }}
    });
    let resp = handle(&req, &registry()).unwrap();
    assert_eq!(resp["result"]["isError"], true, "{resp}");
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    let err: Value = serde_json::from_str(text).unwrap();
    assert_eq!(err["error"]["kind"], "parse", "{err}");
    assert_eq!(err["error"]["line"], 1);
    assert_eq!(
        err["error"]["column"], 31,
        "the column in the text sent, not the wrapper"
    );
}

#[test]
fn initialized_notification_has_no_response() {
    let req = json!({"jsonrpc":"2.0","method":"initialized"});
    assert!(handle(&req, &registry()).is_none());
    let req2 = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
    assert!(handle(&req2, &registry()).is_none());
}

#[test]
fn tools_list_includes_all_four_tools() {
    let req = json!({"jsonrpc":"2.0","id":2,"method":"tools/list"});
    let resp = handle(&req, &registry()).unwrap();
    let names: Vec<&str> = resp["result"]["tools"]
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

#[test]
fn unknown_method_returns_method_not_found() {
    let req = json!({"jsonrpc":"2.0","id":3,"method":"nope"});
    let resp = handle(&req, &registry()).unwrap();
    assert_eq!(resp["error"]["code"], METHOD_NOT_FOUND);
}

#[test]
fn schema_tool_returns_languages_object() {
    let req = json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"colab.schema","arguments":{}}
    });
    let resp = handle(&req, &registry()).unwrap();
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    let parsed: Value = serde_json::from_str(text).unwrap();
    assert!(parsed["languages"].is_array());
}

#[test]
fn lint_script_reports_ok_for_valid_script() {
    let req = json!({
        "jsonrpc":"2.0","id":5,"method":"tools/call",
        "params":{"name":"colab.lint_script","arguments":{
            "script":"refactor \"x\" { match go::import \"a\" { replace \"b\" } }"
        }}
    });
    let resp = handle(&req, &registry()).unwrap();
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    let parsed: Value = serde_json::from_str(text).unwrap();
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["name"], "x");
    assert_eq!(parsed["rule_count"], 1);
}

#[test]
fn lint_script_reports_error_for_unsupported_namespace() {
    let req = json!({
        "jsonrpc":"2.0","id":6,"method":"tools/call",
        "params":{"name":"colab.lint_script","arguments":{
            "script":"refactor \"x\" { match klingon::module \"a\" { replace \"b\" } }"
        }}
    });
    let resp = handle(&req, &registry()).unwrap();
    // A failed tool call is flagged as such — not silently `isError:
    // false` with the failure buried in the body.
    assert_eq!(resp["result"]["isError"], true, "got: {resp}");
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    let parsed: Value = serde_json::from_str(text).unwrap();
    assert_eq!(parsed["error"]["kind"], "unsupported");
    assert_eq!(parsed["error"]["exit_code"], 3);
    // The message names the languages that do exist.
    let message = parsed["error"]["message"].as_str().unwrap();
    assert!(message.contains("known languages:"), "{message}");
    assert!(message.contains("go"), "{message}");
    assert!(message.contains("rust"), "{message}");
}

#[test]
fn a_parse_failure_reports_line_column_and_expected_tokens() {
    let req = json!({
        "jsonrpc":"2.0","id":60,"method":"tools/call",
        "params":{"name":"colab.lint_script","arguments":{
            "script":"refactor \"x\" {\n  match go::import \"a\" { replac \"b\" }\n}\n"
        }}
    });
    let resp = handle(&req, &registry()).unwrap();
    assert_eq!(resp["result"]["isError"], true, "got: {resp}");
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    let parsed: Value = serde_json::from_str(text).unwrap();

    assert_eq!(parsed["error"]["kind"], "parse");
    assert_eq!(parsed["error"]["exit_code"], 2);
    assert_eq!(parsed["error"]["line"], 2);
    assert_eq!(parsed["error"]["column"], 26);
    let expected = parsed["error"]["expected"]
        .as_array()
        .expect("expected list");
    assert!(
        expected
            .iter()
            .any(|e| e.as_str().unwrap().contains("replace")),
        "got: {expected:?}"
    );
    assert!(parsed["error"]["snippet"].as_str().unwrap().contains('^'));
}

#[test]
fn an_unknown_tool_is_a_protocol_error_that_names_the_real_tools() {
    let req = json!({
        "jsonrpc":"2.0","id":61,"method":"tools/call",
        "params":{"name":"colab.previw","arguments":{}}
    });
    let resp = handle(&req, &registry()).unwrap();
    assert_eq!(resp["error"]["code"], -32602, "got: {resp}");
    let message = resp["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("did you mean `colab.preview`?"),
        "{message}"
    );
}

#[test]
fn list_rules_is_a_fraction_of_the_full_schema() {
    let call = |name: &str, args: Value| {
        let req = json!({
            "jsonrpc":"2.0","id":62,"method":"tools/call",
            "params":{"name": name, "arguments": args}
        });
        let resp = handle(&req, &registry()).unwrap();
        resp["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string()
    };

    let languages = call("colab.list_languages", json!({}));
    let one_lang = call("colab.list_rules", json!({"lang": "go"}));
    let filtered = call("colab.schema", json!({"lang": "go"}));
    let everything = call("colab.schema", json!({}));

    assert!(languages.contains("\"go\""));
    assert!(languages.len() < one_lang.len());
    assert!(one_lang.len() < everything.len());
    assert!(filtered.len() < everything.len());
}

#[test]
fn list_rules_rejects_an_unknown_language_with_a_suggestion() {
    let req = json!({
        "jsonrpc":"2.0","id":63,"method":"tools/call",
        "params":{"name":"colab.list_rules","arguments":{"lang":"rustt"}}
    });
    let resp = handle(&req, &registry()).unwrap();
    assert_eq!(resp["result"]["isError"], true, "got: {resp}");
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("did you mean `rust`?"), "{text}");
}

#[test]
fn lint_script_rejects_missing_script_argument() {
    let req = json!({
        "jsonrpc":"2.0","id":7,"method":"tools/call",
        "params":{"name":"colab.lint_script","arguments":{}}
    });
    let resp = handle(&req, &registry()).unwrap();
    // Tool-level error: returns an `error` field at JSON-RPC level
    assert!(resp.get("error").is_some(), "got: {resp}");
}

#[test]
fn preview_returns_diff_without_writing() {
    use std::fs;
    let dir = std::env::temp_dir().join(format!(
        "colab-mcp-preview-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    let go_path = dir.join("main.go");
    let original = "package main\nimport \"old.module\"\n";
    fs::write(&go_path, original).unwrap();

    let script = "refactor \"r\" { match go::import \"old.module\" { replace \"new.module\" } }";

    // Default detail: counts, no diff.
    let req = json!({
        "jsonrpc":"2.0","id":8,"method":"tools/call",
        "params":{"name":"colab.preview","arguments":{
            "script": script,
            "paths":[go_path.to_string_lossy()],
        }}
    });
    let resp = handle(&req, &registry()).unwrap();
    assert_eq!(resp["result"]["isError"], false);
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    let parsed: Value = serde_json::from_str(text).unwrap();

    assert_eq!(parsed["summary"]["scanned"], 1);
    assert_eq!(parsed["summary"]["changed"], 1);
    assert_eq!(parsed["rules"][0]["files"], 1);
    assert_eq!(parsed["changed"].as_array().unwrap().len(), 1);
    assert_eq!(parsed["applied"], false);
    assert!(parsed.get("diffs").is_none(), "counts must not carry diffs");
    assert!(parsed.get("warnings").is_none(), "got: {parsed}");
    // Compact, not pretty-printed.
    assert!(
        !text.contains("\n  \""),
        "response should be compact: {text}"
    );

    // Explicit diff detail adds hunks.
    let req = json!({
        "jsonrpc":"2.0","id":9,"method":"tools/call",
        "params":{"name":"colab.preview","arguments":{
            "script": script,
            "paths":[go_path.to_string_lossy()],
            "detail":"diff",
        }}
    });
    let resp = handle(&req, &registry()).unwrap();
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    let parsed: Value = serde_json::from_str(text).unwrap();
    assert!(
        parsed["diffs"][0]["diff"]
            .as_str()
            .unwrap()
            .contains("--- a/")
    );

    // File on disk untouched by either call.
    assert_eq!(fs::read_to_string(&go_path).unwrap(), original);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn streaming_and_synchronous_paths_report_identically() {
    // The two entry points share one `run_script` behind a
    // ProgressSink, but that is an implementation detail — this
    // asserts the observable guarantee: whether or not a client
    // supplies a progressToken changes only the notifications, never
    // the report. When these were two 61%-duplicated functions,
    // nothing enforced that.
    use std::fs;
    let dir = std::env::temp_dir().join(format!("colab-mcp-equiv-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("main.go"), "package main\nimport \"old/pkg\"\n").unwrap();

    let script = "refactor \"r\" { match go::import \"old/pkg\" { replace \"new/pkg\" } }";
    let arguments = json!({
        "script": script,
        "paths": ["."],
        "cwd": dir.to_string_lossy(),
    });

    // Synchronous: no sink.
    let sync = run_script(
        &arguments,
        &registry(),
        &NoTools,
        RunMode::Preview,
        &mut NoProgress,
    )
    .expect("synchronous run");

    // Streaming: notifications go to a buffer we then ignore.
    let mut buffer: Vec<u8> = Vec::new();
    let token = json!("tok-1");
    let mut sink = Notifications {
        writer: &mut buffer,
        token: &token,
        last_emitted: 0,
    };
    let streamed = run_script(
        &arguments,
        &registry(),
        &NoTools,
        RunMode::Preview,
        &mut sink,
    )
    .expect("streaming run");

    // `elapsed_ms` is wall-clock and will differ; everything else
    // must match exactly.
    let strip = |mut v: Value| {
        v["summary"]
            .as_object_mut()
            .expect("summary")
            .remove("elapsed_ms");
        v
    };
    assert_eq!(
        strip(sync),
        strip(streamed),
        "the progressToken changed the report, not just the notifications"
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn preview_flags_a_dead_rule_without_needing_a_diff() {
    use std::fs;
    let dir = std::env::temp_dir().join(format!("colab-mcp-dead-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("main.go"), "package main\nimport \"old.module\"\n").unwrap();

    let req = json!({
        "jsonrpc":"2.0","id":10,"method":"tools/call",
        "params":{"name":"colab.preview","arguments":{
            "script": "refactor \"r\" {\n\
                    match go::import \"old.module\" { replace \"new.module\" }\n\
                    match go::import \"absent.module\" { replace \"x\" }\n\
                  }",
            "paths":[dir.to_string_lossy()],
        }}
    });
    let resp = handle(&req, &registry()).unwrap();
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    let parsed: Value = serde_json::from_str(text).unwrap();

    assert_eq!(parsed["rules"][0]["files"], 1);
    assert_eq!(parsed["rules"][1]["files"], 0);
    let warnings = parsed["warnings"].as_array().expect("warnings");
    assert!(
        warnings[0].as_str().unwrap().contains("matched no files"),
        "got: {warnings:?}"
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn apply_writes_nothing_while_a_rule_reports_a_conflict() {
    use std::fs;
    let dir = std::env::temp_dir().join(format!("colab-mcp-conflict-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let input = "package main\n\nfunc load() {}\n\nfunc parse() {}\n";
    fs::write(dir.join("main.go"), input).unwrap();
    let call = |allow: bool| {
        json!({
            "jsonrpc":"2.0","id":12,"method":"tools/call",
            "params":{"name":"colab.apply","arguments":{
                "script":"refactor \"r\" { match go::symbol \"load\" { replace \"parse\" } }",
                "paths":[dir.to_string_lossy()],
                "allow_conflicts": allow,
            }}
        })
    };

    let resp = handle(&call(false), &registry()).unwrap();
    let parsed: Value =
        serde_json::from_str(resp["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(parsed["applied"], false, "{parsed}");
    assert_eq!(parsed["conflicts"][0]["rule"], 0, "{parsed}");
    assert_eq!(fs::read_to_string(dir.join("main.go")).unwrap(), input);

    let resp = handle(&call(true), &registry()).unwrap();
    let parsed: Value =
        serde_json::from_str(resp["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(parsed["applied"], true, "{parsed}");
    assert!(
        !fs::read_to_string(dir.join("main.go"))
            .unwrap()
            .contains("load")
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn workflow_tools_generate_the_script_and_preview_by_default() {
    use std::fs;
    let dir = std::env::temp_dir().join(format!("colab-mcp-workflow-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let go = "package main\n\nimport \"old/pkg\"\n";
    fs::write(dir.join("main.go"), go).unwrap();
    fs::write(dir.join("Cargo.toml"), "[dependencies]\nregex = \"1.9\"\n").unwrap();
    let call = |name: &str, args: Value| {
        let req = json!({"jsonrpc":"2.0","id":13,"method":"tools/call",
            "params":{"name": name, "arguments": args}});
        let resp = handle(&req, &registry()).unwrap();
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        serde_json::from_str::<Value>(&text).unwrap()
    };
    let paths = json!([dir.to_string_lossy()]);

    let preview = call(
        "colab.move_import",
        json!({"lang": "go", "from": "old/pkg", "to": "new/pkg", "paths": paths}),
    );
    assert_eq!(preview["applied"], false, "{preview}");
    assert_eq!(preview["rules"][0]["files"], 1, "{preview}");
    assert!(
        preview["script"]
            .as_str()
            .unwrap()
            .contains("match go::import \"old/pkg\" { replace \"new/pkg\" }")
    );
    assert_eq!(fs::read_to_string(dir.join("main.go")).unwrap(), go);

    let applied = call(
        "colab.move_import",
        json!({"lang": "go", "from": "old/pkg", "to": "new/pkg", "paths": paths, "write": true}),
    );
    assert_eq!(applied["applied"], true, "{applied}");
    assert!(
        fs::read_to_string(dir.join("main.go"))
            .unwrap()
            .contains("new/pkg")
    );

    let toml = dir.join("Cargo.toml");
    let set = call(
        "colab.set_config_key",
        json!({"key": "dependencies.regex", "value": "\"1.10\"", "paths": [toml.to_string_lossy()], "write": true}),
    );
    assert_eq!(set["applied"], true, "{set}");
    assert!(
        fs::read_to_string(&toml)
            .unwrap()
            .contains("regex = \"1.10\"")
    );

    let unknown = handle(&json!({"jsonrpc":"2.0","id":14,"method":"tools/call",
        "params":{"name":"colab.move_import","arguments":{"lang":"toml","from":"a","paths":paths}}}), &registry()).unwrap();
    assert_eq!(unknown["result"]["isError"], true, "{unknown}");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn relative_paths_resolve_against_cwd() {
    use std::fs;
    let dir = std::env::temp_dir().join(format!("colab-mcp-cwd-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("main.go"), "package main\nimport \"old.module\"\n").unwrap();

    let req = json!({
        "jsonrpc":"2.0","id":11,"method":"tools/call",
        "params":{"name":"colab.preview","arguments":{
            "script":"refactor \"r\" { match go::import \"old.module\" { replace \"new.module\" } }",
            "paths":["main.go"],
            "cwd": dir.to_string_lossy(),
        }}
    });
    let resp = handle(&req, &registry()).unwrap();
    assert_eq!(resp["result"]["isError"], false, "got: {resp}");
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    let parsed: Value = serde_json::from_str(text).unwrap();
    assert_eq!(parsed["summary"]["changed"], 1);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_empty_result_says_which_kind_of_empty_it_was() {
    use std::fs;
    let dir = std::env::temp_dir().join(format!("colab-mcp-empty-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    // A file of a language the script does not target.
    fs::write(dir.join("notes.txt"), "nothing to see\n").unwrap();

    let req = json!({
        "jsonrpc":"2.0","id":12,"method":"tools/call",
        "params":{"name":"colab.preview","arguments":{
            "script":"refactor \"r\" { match go::import \"old.module\" { replace \"new.module\" } }",
            "paths":[dir.to_string_lossy()],
        }}
    });
    let resp = handle(&req, &registry()).unwrap();
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    let parsed: Value = serde_json::from_str(text).unwrap();

    assert_eq!(parsed["summary"]["visited"], 1);
    assert_eq!(parsed["summary"]["scanned"], 0);
    let warnings = parsed["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap().contains("none belong to a language")),
        "got: {warnings:?}"
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn apply_writes_file() {
    use std::fs;
    let dir = std::env::temp_dir().join(format!(
        "colab-mcp-apply-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    let go_path = dir.join("main.go");
    fs::write(&go_path, "package main\nimport \"old.module\"\n").unwrap();

    let req = json!({
        "jsonrpc":"2.0","id":9,"method":"tools/call",
        "params":{"name":"colab.apply","arguments":{
            "script":"refactor \"r\" { match go::import \"old.module\" { replace \"new.module\" } }",
            "paths":[go_path.to_string_lossy()],
        }}
    });
    let resp = handle(&req, &registry()).unwrap();
    assert_eq!(resp["result"]["isError"], false);
    let on_disk = fs::read_to_string(&go_path).unwrap();
    assert!(on_disk.contains("new.module"));
    assert!(!on_disk.contains("old.module"));
    fs::remove_dir_all(&dir).ok();
}

/// Parse a buffer of newline-delimited JSON-RPC messages into a
/// `Vec<Value>`, asserting each line is exactly one message.
fn parse_messages(buf: &[u8]) -> Vec<Value> {
    let text = std::str::from_utf8(buf).unwrap();
    assert!(
        text.is_empty() || text.ends_with('\n'),
        "unterminated line: {text:?}"
    );
    text.lines()
        .map(|l| serde_json::from_str(l).expect("each line is one JSON message"))
        .collect()
}

#[test]
fn handle_streaming_emits_progress_notifications_when_token_present() {
    use std::fs;
    let dir = std::env::temp_dir().join(format!(
        "colab-mcp-progress-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    // Create > PROGRESS_BATCH (64) Go files so we get at least
    // one mid-walk progress notification.
    for i in 0..80 {
        let p = dir.join(format!("dir{}/{:03}.go", i % 4, i));
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, "package demo\nimport \"old.module\"\n").unwrap();
    }

    let req = json!({
        "jsonrpc": "2.0",
        "id": 42,
        "method": "tools/call",
        "params": {
            "name": "colab.preview",
            "arguments": {
                "script": "refactor \"r\" { match go::import \"old.module\" { replace \"new.module\" } }",
                "paths": [dir.to_string_lossy()],
            },
            "_meta": {
                "progressToken": "tok-1"
            }
        }
    });

    let mut buf: Vec<u8> = Vec::new();
    handle_streaming(&req, &registry(), &mut buf).unwrap();

    let messages = parse_messages(&buf);
    // At least: ≥1 mid-walk progress + 1 final progress + 1 response.
    let progress: Vec<&Value> = messages
        .iter()
        .filter(|m| {
            m.get("method")
                .and_then(|x| x.as_str())
                .is_some_and(|s| s == "notifications/progress")
        })
        .collect();
    let responses: Vec<&Value> = messages.iter().filter(|m| m.get("id").is_some()).collect();

    assert!(
        progress.len() >= 2,
        "got {} progress messages",
        progress.len()
    );
    assert_eq!(responses.len(), 1, "got {responses:?}");
    assert_eq!(responses[0]["id"], 42);
    // Each progress carries the original token.
    for p in &progress {
        assert_eq!(p["params"]["progressToken"], json!("tok-1"));
    }
    // The final progress carries `total == progress`.
    let last = progress.last().unwrap();
    assert_eq!(last["params"]["progress"], last["params"]["total"]);

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn handle_streaming_skips_progress_when_no_token() {
    let req = json!({
        "jsonrpc":"2.0","id":7,"method":"tools/call",
        "params":{"name":"colab.schema","arguments":{}}
    });
    let mut buf: Vec<u8> = Vec::new();
    handle_streaming(&req, &registry(), &mut buf).unwrap();
    let messages = parse_messages(&buf);
    // Exactly one response, no progress notifications.
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["id"], 7);
    assert!(messages[0].get("method").is_none());
}

#[test]
fn read_write_roundtrip() {
    let input = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n",
        "\n",
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n",
    );
    let mut output: Vec<u8> = Vec::new();
    serve(input.as_bytes(), &mut output, registry()).unwrap();

    // One response per request, each on its own line, no headers.
    let text = String::from_utf8(output).unwrap();
    assert!(!text.contains("Content-Length"), "{text}");
    let messages = parse_messages(text.as_bytes());
    assert_eq!(messages.len(), 2, "{text}");
    assert_eq!(messages[0]["id"], 1);
    assert_eq!(messages[0]["result"]["protocolVersion"], PROTOCOL_VERSION);
    assert_eq!(messages[1]["id"], 2);
    assert!(messages[1]["result"]["tools"].is_array());
}

#[test]
fn last_line_without_newline_is_still_read() {
    let input = "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/list\"}";
    let mut output: Vec<u8> = Vec::new();
    serve(input.as_bytes(), &mut output, registry()).unwrap();
    let messages = parse_messages(&output);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["id"], 3);
}

#[test]
fn invalid_json_line_gets_parse_error_and_server_continues() {
    let input = "not json\n{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"tools/list\"}\n";
    let mut output: Vec<u8> = Vec::new();
    serve(input.as_bytes(), &mut output, registry()).unwrap();
    let messages = parse_messages(&output);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["error"]["code"], PARSE_ERROR);
    assert!(messages[0]["id"].is_null());
    assert_eq!(messages[1]["id"], 4);
}

#[test]
fn newlines_inside_strings_stay_on_one_line() {
    let mut output: Vec<u8> = Vec::new();
    write_message(&mut output, &json!({"text": "a\nb\r\nc"})).unwrap();
    let text = String::from_utf8(output).unwrap();
    assert_eq!(text.matches('\n').count(), 1);
    assert!(text.ends_with('\n'));
    assert_eq!(parse_messages(text.as_bytes())[0]["text"], "a\nb\r\nc");
}

fn test_rule_call(arguments: Value) -> Value {
    let req = json!({
        "jsonrpc":"2.0","id":40,"method":"tools/call",
        "params":{"name":"colab.test_rule","arguments": arguments}
    });
    let resp = handle(&req, &registry()).unwrap();
    assert_eq!(resp["result"]["isError"], false, "{resp}");
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    serde_json::from_str(text).unwrap()
}

#[test]
fn an_in_glob_is_relative_to_the_calls_cwd() {
    let dir = std::env::temp_dir().join(format!("colab-mcp-in-glob-{}", std::process::id()));
    for sub in ["api", "app"] {
        std::fs::create_dir_all(dir.join(sub)).unwrap();
        std::fs::write(
            dir.join(sub).join("x.go"),
            "package x\n\nimport \"old/pkg\"\n",
        )
        .unwrap();
    }
    let script = r#"match go::import "old/pkg" in "api/**" { replace "new/pkg" }"#;
    let call = |tool: &str| {
        let req = json!({
            "jsonrpc":"2.0","id":41,"method":"tools/call",
            "params":{"name": tool, "arguments": {
                "script": script, "paths": ["."], "cwd": dir.to_string_lossy()
            }}
        });
        let resp = handle(&req, &registry()).unwrap();
        assert_eq!(resp["result"]["isError"], false, "{resp}");
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        serde_json::from_str::<Value>(text).unwrap()
    };
    let found = call("colab.find");
    assert_eq!(found["summary"]["matches"], 1, "{found}");
    assert!(found.to_string().contains("api/x.go"), "{found}");
    let preview = call("colab.preview");
    assert_eq!(preview["summary"]["changed"], 1, "{preview}");
    assert!(preview.to_string().contains("api/x.go"), "{preview}");
    assert!(!preview.to_string().contains("app/x.go"), "{preview}");
    std::fs::remove_dir_all(&dir).unwrap();
}

const TEST_RULE_SCRIPT: &str = "refactor \"x\" { match go::import \"a\" { replace \"b\" } }";
const TEST_RULE_SOURCE: &str = "package main\n\nimport \"a\"\n";
const TEST_RULE_OUTPUT: &str = "package main\n\nimport \"b\"\n";

#[test]
fn test_rule_transforms_a_snippet_in_memory() {
    let parsed = test_rule_call(json!({
        "script": TEST_RULE_SCRIPT,
        "source": TEST_RULE_SOURCE,
        "path": "main.go",
    }));
    assert_eq!(parsed["changed"], true);
    assert_eq!(parsed["output"], TEST_RULE_OUTPUT);
    assert_eq!(parsed["rules"][0]["files"], 1);
    assert!(parsed["diff"].as_str().unwrap().contains("+import \"b\""));
    assert!(parsed.get("warnings").is_none());
}

#[test]
fn test_rule_compares_against_expected() {
    let ok = test_rule_call(json!({
        "script": TEST_RULE_SCRIPT,
        "source": TEST_RULE_SOURCE,
        "path": "main.go",
        "expected": TEST_RULE_OUTPUT,
    }));
    assert_eq!(ok["matches_expected"], true);
    assert!(ok.get("expected_diff").is_none());

    let bad = test_rule_call(json!({
        "script": TEST_RULE_SCRIPT,
        "source": TEST_RULE_SOURCE,
        "path": "main.go",
        "expected": "package main\n\nimport \"c\"\n",
    }));
    assert_eq!(bad["matches_expected"], false);
    assert!(
        bad["expected_diff"]
            .as_str()
            .unwrap()
            .contains("+import \"c\"")
    );
}

#[test]
fn test_rule_warns_when_no_rule_applies_to_the_path() {
    let parsed = test_rule_call(json!({
        "script": TEST_RULE_SCRIPT,
        "source": TEST_RULE_SOURCE,
        "path": "main.rs",
    }));
    assert_eq!(parsed["changed"], false);
    assert_eq!(parsed["output"], TEST_RULE_SOURCE);
    let warnings = parsed["warnings"].to_string();
    assert!(warnings.contains("matched no files"), "{warnings}");
    assert!(
        warnings.contains("No rule applies to `main.rs`"),
        "{warnings}"
    );
}

#[test]
fn test_rule_reports_script_errors_as_tool_errors() {
    let req = json!({
        "jsonrpc":"2.0","id":41,"method":"tools/call",
        "params":{"name":"colab.test_rule","arguments":{
            "script": "refactor \"x\" { match go::import \"a\" { replac \"b\" } }",
            "source": "", "path": "main.go"
        }}
    });
    let resp = handle(&req, &registry()).unwrap();
    assert_eq!(resp["result"]["isError"], true);
}

#[test]
fn test_rule_requires_source_and_path() {
    let req = json!({
        "jsonrpc":"2.0","id":42,"method":"tools/call",
        "params":{"name":"colab.test_rule","arguments":{"script": TEST_RULE_SCRIPT}}
    });
    let resp = handle(&req, &registry()).unwrap();
    assert_eq!(resp["error"]["code"], INVALID_PARAMS);
}

fn call_ok(name: &str, arguments: Value) -> Value {
    let req = json!({
        "jsonrpc":"2.0","id":50,"method":"tools/call",
        "params":{"name": name, "arguments": arguments}
    });
    let resp = handle(&req, &registry()).unwrap();
    assert_eq!(resp["result"]["isError"], false, "{resp}");
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    serde_json::from_str(text).unwrap()
}

#[test]
fn find_reports_pattern_matches_with_captures_relative_to_cwd() {
    let dir = std::env::temp_dir().join(format!("colab-mcp-find-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("main.go"),
        "package main\n\nfunc f() error {\n\treturn errors.Wrap(err, \"x\")\n}\n",
    )
    .unwrap();
    let parsed = call_ok(
        "colab.find",
        json!({
            "script": "refactor \"f\" { match go::pattern 'errors.Wrap($E, $M)' { delete } }",
            "paths": ["."],
            "cwd": dir.to_str().unwrap(),
            "detail": "detailed",
        }),
    );
    std::fs::remove_dir_all(&dir).ok();
    let m = &parsed["matches"][0];
    assert_eq!(m["path"], "main.go");
    assert_eq!((m["line"].as_u64(), m["col"].as_u64()), (Some(4), Some(9)));
    assert_eq!(m["captures"]["E"], "err");
    assert_eq!(parsed["summary"]["matches"], 1);
}

#[test]
fn ast_shows_a_pattern_tree_and_its_context() {
    // A bare call reads as it is; a case clause only inside a switch.
    let parsed = call_ok("colab.ast", json!({"lang": "go", "pattern": "f($A)"}));
    assert!(
        parsed["tree"]
            .as_str()
            .unwrap()
            .starts_with("call_expression")
    );
    assert_eq!(parsed["prefilter"], "f");
    let case = call_ok(
        "colab.ast",
        json!({"lang": "go", "pattern": "case $X: $$$B"}),
    );
    assert!(
        case["tree"]
            .as_str()
            .unwrap()
            .starts_with("expression_case")
    );
    assert!(case["context"].as_str().unwrap().contains("switch"));
}

#[test]
fn ast_needs_exactly_one_input() {
    let req = json!({
        "jsonrpc":"2.0","id":51,"method":"tools/call",
        "params":{"name":"colab.ast","arguments":{"lang":"go"}}
    });
    let resp = handle(&req, &registry()).unwrap();
    assert_eq!(resp["error"]["code"], INVALID_PARAMS);
}

struct Extra;

impl ToolProvider for Extra {
    fn tools(&self) -> Vec<Value> {
        vec![
            json!({"name": "colab.extra", "inputSchema": {"type": "object"}, "annotations": {"readOnlyHint": true}}),
        ]
    }
    fn call(&self, name: &str, args: &Value) -> Option<colab_core::Result<Value>> {
        (name == "colab.extra").then(|| match args.get("fail") {
            Some(_) => Err(colab_core::Error::Config("extra failed".into())),
            None => Ok(json!({"echo": args})),
        })
    }
}

#[test]
fn a_host_provider_adds_tools_to_the_list_and_dispatch() {
    let list = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"});
    let resp = handle_with(&list, &registry(), &Extra).unwrap();
    let names: Vec<&str> = resp["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"colab.preview") && names.contains(&"colab.extra"));

    let call = |args: Value| {
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": "colab.extra", "arguments": args}})
    };
    let ok = handle_with(&call(json!({"x": 1})), &registry(), &Extra).unwrap();
    assert_eq!(ok["result"]["isError"], false);
    assert!(
        ok["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("\"x\":1")
    );
    let err = handle_with(&call(json!({"fail": true})), &registry(), &Extra).unwrap();
    assert_eq!(err["result"]["isError"], true);
    assert!(
        err["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("extra failed")
    );

    // Without the provider the tool does not exist.
    let plain = handle(&call(json!({})), &registry()).unwrap();
    assert_ne!(plain["result"]["isError"], false);
}
