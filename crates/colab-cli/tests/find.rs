//! End-to-end tests for the read-only commands: `colab find` and `colab ast`.

mod common;
use common::*;

const GO_WRAP: &str = "package main\n\nfunc f() error {\n\treturn errors.Wrap(err, \"read\")\n}\n";

#[test]
fn find_stdin_searches_one_text_named_by_path() {
    let run = |args: &[&str], text: &str| {
        let mut child = colab()
            .args(["find", "--stdin"])
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    };
    // The language comes from --path, and locations are in the text.
    let out = run(
        &["--path", "src/x.go", "fmt.Println($A)"],
        "package p\n\nfunc f() {\n\tfmt.Println(1)\n}\n",
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("src/x.go:4:2:"), "{stdout}");
    // Paths alongside --stdin are refused, and --stdin needs --path.
    let both = run(&["--path", "x.go", "fmt.Println($A)", "src"], "");
    assert!(String::from_utf8_lossy(&both.stderr).contains("takes no paths"));
    assert!(!run(&["fmt.Println($A)"], "").status.success());
}

#[test]
fn a_ts_field_pattern_may_end_in_its_semicolon() {
    let run = |args: &[&str]| {
        let mut child = colab()
            .args(args)
            .args(["--stdin", "--path", "a.ts"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"class A {\n  private count: number = 0;\n}\n")
            .unwrap();
        child.wait_with_output().unwrap()
    };
    // The `;` is the field's sibling in the class body, not part of it.
    let found = run(&["find", "private count: number = $V;"]);
    assert!(
        String::from_utf8_lossy(&found.stdout).contains("a.ts:2:3:"),
        "{found:?}"
    );
    // And a template's own `;` is not doubled.
    let out = run(&[
        "refactor",
        "-e",
        "match ts::pattern 'count: number = $V;' { replace 'count: number = $V + 1;' }",
    ]);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "class A {\n  private count: number = 0 + 1;\n}\n"
    );
}

#[test]
fn an_untyped_field_pattern_with_its_semicolon_finds_fields_and_statements() {
    let root = workspace_temp("ts-untyped-field");
    write(
        &root.join("g.ts"),
        "class A {\n  name = \"x\";\n}\nname = \"y\";\nf(name);\n",
    );
    let out = colab()
        .current_dir(&root)
        .args(["find", "--lang", "ts", "name = $V;", "g.ts"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("g.ts:2:3:") && stdout.contains("g.ts:4:1:"),
        "{stdout}"
    );
    // A call statement is not a field, and keeps reading as a statement.
    let call = colab()
        .current_dir(&root)
        .args(["find", "--lang", "ts", "f($A);", "g.ts"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&call.stdout).contains("1 match(es)"));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_bare_pattern_over_typescript_picks_ts_or_tsx_by_the_files() {
    // `js` claims `.ts` and `.tsx` too; the backend named after the files'
    // extension is the one meant, and a JSX pattern parses only in `tsx`.
    let root = workspace_temp("find-tsx");
    write(
        &root.join("app/button.tsx"),
        "export const B = (p: Props) => <Button onClick={p.go} />;\n",
    );
    write(
        &root.join("lib/store.ts"),
        "export function load(p: string): Promise<Config> { return get(p); }\n",
    );
    let find = |dir: &str, pattern: &str| {
        colab()
            .current_dir(root.join(dir))
            .args(["find", pattern, "."])
            .output()
            .unwrap()
    };
    let jsx = find("app", "<Button onClick={$H} />");
    assert!(
        jsx.status.success(),
        "{}",
        String::from_utf8_lossy(&jsx.stderr)
    );
    assert!(String::from_utf8_lossy(&jsx.stderr).contains("searching tsx files"));
    assert!(String::from_utf8_lossy(&jsx.stdout).contains("button.tsx:1:"));
    let typed = find("lib", "Promise<$T>");
    assert!(
        typed.status.success(),
        "{}",
        String::from_utf8_lossy(&typed.stderr)
    );
    assert!(String::from_utf8_lossy(&typed.stderr).contains("searching ts files"));
    assert!(String::from_utf8_lossy(&typed.stdout).contains("store.ts:1:"));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn find_takes_a_bare_pattern_as_grep_takes_a_regex() {
    let root = workspace_temp("find-bare");
    write(&root.join("main.go"), GO_WRAP);
    write(&root.join("notes.md"), "# Notes\n");
    let run = |args: &[&str]| {
        colab()
            .current_dir(&root)
            .args(["find"])
            .args(args)
            .output()
            .unwrap()
    };
    // Positional, the language inferred from the files (Go, not Markdown).
    let found = run(&["errors.Wrap($E, $M)", "."]);
    let stdout = String::from_utf8_lossy(&found.stdout);
    assert_eq!(
        found.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&found.stderr)
    );
    assert!(stdout.contains("main.go:4:9:"), "{stdout}");
    // --pattern with --lang says the same; quotes in it are fine.
    let explicit = run(&[
        "--pattern",
        "errors.Wrap($E, \"read\")",
        "--lang",
        "go",
        ".",
    ]);
    assert!(String::from_utf8_lossy(&explicit.stdout).contains("main.go:4:9:"));
    // A pattern that is not code in the files' language says what to do.
    let incomplete = run(&["errors.Wrap($E,", "."]);
    assert_ne!(incomplete.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&incomplete.stderr);
    assert!(
        stderr.contains("mostly go") && stderr.contains("colab ast --lang go"),
        "{stderr}"
    );
    // Nothing to find names the three ways to say it.
    let nothing = run(&["."]);
    assert!(String::from_utf8_lossy(&nothing.stderr).contains("give what to find"));
    fs::remove_dir_all(&root).ok();
}

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
fn find_jobs_sets_the_workers_and_leaves_the_hits_alone() {
    let root = workspace_temp("find-jobs");
    for i in 0..40 {
        write(
            &root.join(format!("pkg{i:02}/main.go")),
            &GO_WRAP.replace("\"read\"", &format!("\"read {i}\"")),
        );
    }
    let find = |jobs: &[&str], env: Option<&str>| {
        let mut command = colab();
        command.args([
            "find",
            "--table",
            "csv",
            "-e",
            "match go::pattern 'errors.Wrap($E, $M)' { delete }",
        ]);
        command.args(jobs).arg(&root);
        match env {
            Some(n) => command.env("COLAB_JOBS", n),
            None => command.env_remove("COLAB_JOBS"),
        };
        let output = command.output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    let default = find(&[], None);
    assert_eq!(
        default.lines().count(),
        41,
        "a header and 40 hits: {default}"
    );
    assert_eq!(find(&["--jobs", "1"], None), default);
    assert_eq!(find(&["--jobs", "4"], None), default);
    assert_eq!(
        find(&[], Some("1")),
        default,
        "COLAB_JOBS when --jobs is not given"
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
