//! End-to-end tests for the workflow commands built on `refactor`: the
//! Markdown commands, `colab fix` and packs, the CI formats, `--split-pr`,
//! `colab review`, `--baseline`, semantic constraints, fleets, `colab edit`,
//! and scan phases.

mod common;
use common::*;

#[test]
fn outline_and_section_read_markdown_by_structure() {
    let root = workspace_temp("md-outline");
    let doc = "# Plan\n\n## Order\n\n```sh\n# not a heading\n```\n\n## Risks\n\nrisky\n";
    write(&root.join("docs/plan.md"), doc);
    let out = colab()
        .current_dir(&root)
        .args(["outline", "docs", "--format", "json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let hs = v["files"][0]["headings"].as_array().unwrap();
    assert_eq!(hs.len(), 3, "the fenced `#` is not a heading: {v}");
    assert_eq!(hs[1]["address"], "Plan > Order");
    assert_eq!(hs[1]["fences"], 1);

    let out = colab()
        .current_dir(&root)
        .args(["section", "docs/plan.md", "#risks", "--body"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "risky\n");
    let miss = colab()
        .current_dir(&root)
        .args(["section", "docs/plan.md", "Risk"])
        .output()
        .unwrap();
    assert!(!miss.status.success());
    assert!(String::from_utf8_lossy(&miss.stderr).contains("did you mean `Plan > Risks`"));

    // The same over MCP.
    let call = |id: u32, name: &str, args: Value| {
        serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": name, "arguments": args}})
        .to_string()
    };
    let cwd = root.to_str().unwrap();
    let input = format!(
        "{}\n{}\n",
        call(
            1,
            "colab.outline",
            serde_json::json!({"paths": ["docs"], "cwd": cwd, "depth": 1})
        ),
        call(
            2,
            "colab.section",
            serde_json::json!({"path": "docs/plan.md", "section": "Order", "cwd": cwd})
        ),
    );
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
    let text = |i: usize| -> Value {
        serde_json::from_str(replies[i]["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
    };
    assert_eq!(text(0)["files"][0]["path"], "docs/plan.md");
    assert_eq!(text(0)["files"][0]["headings"].as_array().unwrap().len(), 1);
    assert!(text(1)["text"].as_str().unwrap().starts_with("## Order\n"));
    fs::remove_dir_all(&root).ok();
}

#[test]
fn a_run_that_would_break_a_link_elsewhere_writes_nothing() {
    let root = workspace_temp("md-link-check");
    let guide = "# Guide\n\n## Usage\n\nText.\n";
    write(&root.join("docs/guide.md"), guide);
    write(
        &root.join("README.md"),
        "# Home\n\nRead [usage](docs/guide.md#usage).\n",
    );
    let rename = r#"match markdown::section "Usage" in "docs/guide.md" { replace "How to use" }"#;

    let out = colab()
        .current_dir(&root)
        .args(["refactor", "--write", "--format", "json", "-e", rename, "."])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(11),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["conflicts"][0]["path"], "README.md", "{v}");
    assert!(
        v["conflicts"][0]["message"]
            .as_str()
            .unwrap()
            .contains("`docs/guide.md#usage` on line 3")
    );
    assert_eq!(
        fs::read_to_string(root.join("docs/guide.md")).unwrap(),
        guide
    );

    // Fixing the link in the same run clears it.
    let fix = r#"match markdown::link "guide.md#usage" { replace "guide.md#how-to-use" }"#;
    let status = colab()
        .current_dir(&root)
        .args(["refactor", "--write", "-e", rename, "-e", fix, "."])
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));
    assert!(
        fs::read_to_string(root.join("README.md"))
            .unwrap()
            .contains("#how-to-use")
    );

    // `colab links` agrees, and exits 10 on a broken one.
    let ok = colab()
        .current_dir(&root)
        .args(["links", "."])
        .output()
        .unwrap();
    assert_eq!(ok.status.code(), Some(0));
    write(&root.join("extra.md"), "[x](gone.md)\n");
    let bad = colab()
        .current_dir(&root)
        .args(["links", ".", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(10));
    let v: Value = serde_json::from_slice(&bad.stdout).unwrap();
    assert_eq!(v["broken"][0]["dest"], "gone.md");
    fs::remove_dir_all(&root).ok();
}

#[test]
fn verify_failing_on_rule_one_suggests_bisect() {
    let root = workspace_temp("verify-bisect-hint");
    let target = root.join("main.go");
    write(&target, "package main\n\nimport \"a\"\nimport \"b\"\n");
    let out = colab()
        .args(["refactor", "--verify", "false"])
        .args(["-e", r#"match go::import "a" { replace "x" }"#])
        .args(["-e", r#"match go::import "b" { replace "y" }"#])
        .arg(target.to_str().unwrap())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("use `--bisect`"));
    fs::remove_dir_all(&root).ok();
}

#[test]
fn refactor_tidy_runs_the_server_after_the_rules_as_its_own_rule() {
    let root = workspace_temp("refactor-tidy");
    write(
        &root.join("a.rs"),
        "use zeta;\nuse alpha;\nfn old() {}   \n",
    );
    let out = colab()
        .current_dir(&root)
        .args(["refactor", "--write", "--format", "json"])
        .arg(format!("--tidy={}", fake_lsp()))
        .args([
            "-e",
            r#"match rust::symbol "old" { replace "new" }"#,
            "a.rs",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["rules"][0]["files"], 1, "{v}");
    assert!(
        v["rules"][1]["rule"]
            .as_str()
            .unwrap()
            .starts_with("tidy via"),
        "{v}"
    );
    assert_eq!(v["rules"][1]["files"], 1, "{v}");
    // Renamed, imports sorted, trailing whitespace gone.
    assert_eq!(
        fs::read_to_string(root.join("a.rs")).unwrap(),
        "use alpha;\nuse zeta;\nfn new() {}\n"
    );
    fs::remove_dir_all(&root).ok();
}

#[test]
fn kept_server_is_the_default() {
    let root = rename_workspace("kept-default");
    let lsp = fake_lsp();
    let run = |to: &str, from: &str| {
        colab()
            .env_remove("COLAB_KEEP_SERVER")
            .current_dir(&root)
            .args([
                "rename", "--symbol", from, "--in", "a.rs", "--to", to, "--write",
            ])
            .args(["--lsp", lsp.as_str(), "--root", ".", "--format", "json"])
            .output()
            .unwrap()
    };
    let first = run("middle", "old");
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    // The second command reaches the same server, and sees the first's
    // edit on disk.
    let second = run("newest", "middle");
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert!(
        fs::read_to_string(root.join("a.rs"))
            .unwrap()
            .contains("fn newest()")
    );
    let stop = colab()
        .args(["lsp-daemon", "--stop", "--root"])
        .arg(&root)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&stop.stdout).trim(),
        "stopped 1 kept language server(s)",
        "the commands shared one daemon"
    );
    fs::remove_dir_all(&root).ok();
}

/// A repository with a `colab.toml`, run from a subdirectory.
fn fix_workspace(label: &str, config: &str) -> PathBuf {
    let root = workspace_temp(label);
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::create_dir_all(root.join("sub")).unwrap();
    write(&root.join("colab.toml"), config);
    write(
        &root.join("app.py"),
        "import typing\n\ndef f(a: Optional[str]) -> List[int]:\n    return old(a)\n",
    );
    write(
        &root.join(".colab/house.colab"),
        "refactor \"house\" {\n    match python::pattern 'old($A)' { replace 'new($A)' }\n}\n",
    );
    root
}

#[test]
fn fix_runs_the_packs_colab_toml_declares() {
    let root = fix_workspace(
        "fix",
        "[run]\ntidy = true\n\n[formatters]\npy = \"sed 's/^import typing$/import typing  # tidy/'\"\n\n\
         [[packs]]\npack = \"python/3.12-typing\"\n\n[[packs]]\npath = \".colab/house.colab\"\n",
    );
    let fix = |flag: &str| {
        colab()
            .current_dir(root.join("sub"))
            .args(["fix", flag, "--format", "json"])
            .output()
            .unwrap()
    };
    let out = fix("--check");
    assert_eq!(
        out.status.code(),
        Some(10),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = fix("--write");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let rules = v["rules"].as_array().unwrap();
    // Both packs' rules, then the formatter as its own rule.
    assert!(
        rules
            .iter()
            .any(|r| r["rule"].as_str().unwrap().contains("old($A)")),
        "{v}"
    );
    let last = rules.last().unwrap();
    assert!(
        last["rule"].as_str().unwrap().starts_with("tidy via sed"),
        "{v}"
    );
    assert_eq!(last["files"], 1, "{v}");
    assert_eq!(
        fs::read_to_string(root.join("app.py")).unwrap(),
        "import typing  # tidy\n\ndef f(a: str | None) -> list[int]:\n    return new(a)\n"
    );
    assert_eq!(fix("--check").status.code(), Some(0));
    fs::remove_dir_all(&root).ok();
}

#[test]
fn fix_says_what_is_missing() {
    let root = workspace_temp("fix-missing");
    fs::create_dir_all(root.join(".git")).unwrap();
    let out = colab()
        .current_dir(&root)
        .args(["fix", "--check"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("no colab.toml at or above"), "{err}");

    write(
        &root.join("colab.toml"),
        "[[packs]]\npack = \"go/1.18-modren\"\n",
    );
    let out = colab()
        .current_dir(&root)
        .args(["fix", "--check"])
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("did you mean `go/1.18-modern`"), "{err}");

    fs::remove_dir_all(&root).ok();
}

#[test]
fn pack_list_names_the_builtin_packs() {
    let out = colab().args(["pack", "list"]).output().unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let names: Vec<&str> = v["packs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["source"] == "builtin")
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"go/1.18-modern") && names.contains(&"rust/edition-2021-to-2024"),
        "{v}"
    );
}

#[test]
fn ci_formats_annotate_each_block_with_the_rule_that_changed_it() {
    let root = workspace_temp("ci-formats");
    write(
        &root.join("app.py"),
        "def f(a: Optional[str]) -> None:\n    pass\n\n\ndef g() -> List[int]:\n    return []\n",
    );
    let rules = [
        "-e",
        "match python::pattern 'Optional[$T]' { replace '$T | None' message \"PEP 604\" severity info }",
        "-e",
        "match python::pattern 'List[$$$A]' { replace 'list[$$$A]' }",
    ];
    let run = |format: &str| {
        colab()
            .current_dir(&root)
            .args(["refactor", "--check", "--format", format])
            .args(rules)
            .arg(".")
            .output()
            .unwrap()
    };
    let out = run("github");
    assert_eq!(out.status.code(), Some(10));
    let text = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = text.lines().collect();
    assert!(
        lines[0].starts_with("::notice file=app.py,line=1,endLine=1,"),
        "{text}"
    );
    assert!(
        lines[0].contains("::PEP 604. Replace with:%0Adef f(a: str | None) -> None:"),
        "{text}"
    );
    assert!(
        lines[1].starts_with("::error file=app.py,line=5,endLine=5,"),
        "{text}"
    );

    let out = run("sarif");
    assert_eq!(out.status.code(), Some(10));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let results = v["runs"][0]["results"].as_array().unwrap();
    let ids: Vec<(&str, &str)> = results
        .iter()
        .map(|r| (r["ruleId"].as_str().unwrap(), r["level"].as_str().unwrap()))
        .collect();
    assert_eq!(ids, [("colab/rule-1", "note"), ("colab/rule-2", "error")]);
    fs::remove_dir_all(&root).ok();
}

/// A committed Go repository with one import and two calls to rewrite.
fn split_workspace(label: &str) -> PathBuf {
    let root = workspace_temp(label);
    write(
        &root.join("a.go"),
        "package p\n\nimport \"old/log\"\n\nfunc F() { log.Old(1) }\n",
    );
    write(&root.join("b.go"), "package p\n\nfunc G() { log.Old(2) }\n");
    write(&root.join("notes.txt"), "notes\n");
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", "-A"]);
    git(
        &root,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            "base",
        ],
    );
    root
}

pub(crate) const SPLIT_RULES: [&str; 6] = [
    "-e",
    r#"match go::import "old/log" { replace "new/log" }"#,
    "-e",
    r#"match go::call "log.Old" { replace_call "log.New($args)" }"#,
    "-e",
    r#"match go::import "never/there" { delete }"#,
];

#[test]
fn split_pr_makes_one_branch_per_rule_and_leaves_the_tree_alone() {
    let root = split_workspace("split-pr");
    write(&root.join("notes.txt"), "notes\nlocal edit\n");
    let out = colab()
        .current_dir(&root)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .args(["refactor", "--split-pr=review", "--format", "json"])
        .args(SPLIT_RULES)
        .arg(".")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let branches: Vec<(&str, u64)> = v["branches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| (b["branch"].as_str().unwrap(), b["files"].as_u64().unwrap()))
        .collect();
    // The dead rule gets no branch.
    assert_eq!(
        branches,
        [
            ("review/01-go-import-old-log-new-log", 1),
            ("review/02-go-call-log-old-replace-call-log-new", 2)
        ]
    );
    // Each branch is HEAD plus that rule alone.
    let first = git(
        &root,
        &[
            "diff",
            "--name-only",
            "main",
            "review/01-go-import-old-log-new-log",
        ],
    );
    assert_eq!(first.trim(), "a.go");
    let second = git(
        &root,
        &[
            "show",
            "review/02-go-call-log-old-replace-call-log-new:a.go",
        ],
    );
    assert!(
        second.contains("log.New(1)") && second.contains("\"old/log\""),
        "{second}"
    );
    // Nothing on disk, in the index or on main moved.
    assert_eq!(git(&root, &["status", "--short"]).trim(), "M notes.txt");
    assert_eq!(git(&root, &["branch", "--show-current"]).trim(), "main");

    // Again: the branches exist, so nothing is made.
    let again = colab()
        .current_dir(&root)
        .args(["refactor", "--split-pr=review"])
        .args(SPLIT_RULES)
        .arg(".")
        .output()
        .unwrap();
    assert_eq!(again.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&again.stderr).contains("already exists"));
    fs::remove_dir_all(&root).ok();
}

#[test]
fn split_pr_refuses_uncommitted_changes_in_the_files_it_would_change() {
    let root = split_workspace("split-dirty");
    write(
        &root.join("b.go"),
        "package p\n\nfunc G() { log.Old(2) } // wip\n",
    );
    let out = colab()
        .current_dir(&root)
        .args(["refactor", "--split-pr"])
        .args(SPLIT_RULES)
        .arg(".")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("uncommitted changes") && err.contains("b.go"),
        "{err}"
    );
    assert!(
        git(&root, &["branch", "--list", "colab/*"])
            .trim()
            .is_empty()
    );
    fs::remove_dir_all(&root).ok();
}

#[test]
fn review_groups_a_recorded_run_by_rule() {
    let root = split_workspace("review");
    let log = colab()
        .current_dir(&root)
        .args(["refactor", "--format", "ndjson", "--detail", "diff"])
        .args(SPLIT_RULES)
        .arg(".")
        .output()
        .unwrap();
    write(
        &root.join("run.ndjson"),
        &String::from_utf8_lossy(&log.stdout),
    );
    let out = colab()
        .current_dir(&root)
        .args([
            "review",
            "run.ndjson",
            "--format",
            "json",
            "--rule",
            "2",
            "--per-page",
            "1",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["files"], 2);
    assert_eq!(v["rules"][1]["files"], 2);
    // a.go was changed by rules 1 and 2; the log splits its lines, so
    // neither is flagged as shared, and each counts only its own lines.
    assert!(v["rules"][1].get("shared").is_none(), "{v}");
    let lines = |i: usize| {
        (
            v["rules"][i]["added"].as_u64(),
            v["rules"][i]["removed"].as_u64(),
        )
    };
    assert_eq!(lines(0), (Some(1), Some(1)), "{v}");
    assert_eq!(lines(1), (Some(2), Some(2)), "{v}");
    assert_eq!(v["rules"][2]["files"], 0);
    assert_eq!(v["page"]["pages"], 2);
    assert!(
        v["page"]["files"][0]["diff"]
            .as_str()
            .unwrap()
            .contains("log.New")
    );

    let human = colab()
        .current_dir(&root)
        .args(["review", "run.ndjson"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&human.stdout);
    assert!(
        text.contains("rule 3: 0 file(s)")
            && text.contains("next: colab review run.ndjson --rule 1"),
        "{text}"
    );
    fs::remove_dir_all(&root).ok();
}

#[test]
fn bisect_says_when_the_starting_tree_already_fails() {
    let root = workspace_temp("bisect-start");
    write(&root.join("a.go"), "package p\n\nimport \"a\"\n");
    let out = colab()
        .current_dir(&root)
        .args([
            "refactor",
            "-e",
            r#"match go::import "a" { replace "b" }"#,
            "--bisect",
            "false",
            ".",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("starting tree already fails"), "{err}");
    assert!(!err.contains("breaks"), "no rule is blamed: {err}");
    assert_eq!(
        fs::read_to_string(root.join("a.go")).unwrap(),
        "package p\n\nimport \"a\"\n"
    );
    fs::remove_dir_all(&root).ok();
}

#[test]
fn a_rule_that_changed_some_files_says_which_it_skipped() {
    let root = workspace_temp("partial-skip");
    write(&root.join("a.go"), "package p\n\nfunc a() { f(1) }\n");
    // Here `$$$A,` doubles the trailing comma, so the rewrite would not parse.
    write(
        &root.join("b.go"),
        "package p\n\nfunc b() { f(\n\t1,\n\t2,\n) }\n",
    );
    let out = colab()
        .current_dir(&root)
        .args([
            "refactor",
            "--format",
            "json",
            "-e",
            "match go::pattern 'f($$$A)' { replace 'g($$$A,)' }",
            ".",
        ])
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["rules"][0]["files"], 1, "{v}");
    assert_eq!(v["rules"][0]["unchanged"]["files"], 1, "{v}");
    let warning = v["warnings"][0].as_str().unwrap();
    assert!(
        warning.starts_with("changed 1 file(s) but left its matches unchanged in 1 more"),
        "{warning}"
    );
    fs::remove_dir_all(&root).ok();
}

#[test]
fn script_builtin_runs_a_pack_built_into_colab() {
    let root = workspace_temp("builtin-pack");
    write(&root.join("a.go"), "package p\n\nvar x interface{}\n");
    let out = colab()
        .current_dir(&root)
        .args([
            "refactor",
            "--script",
            "builtin:go/1.18-modern",
            "--format",
            "diff",
            ".",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("+var x any"));
    let typo = colab()
        .current_dir(&root)
        .args(["refactor", "--script", "builtin:go/1.18-modren", "."])
        .output()
        .unwrap();
    assert_eq!(typo.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&typo.stderr).contains("did you mean `go/1.18-modern`"));
    fs::remove_dir_all(&root).ok();
}

#[test]
fn mcp_mv_moves_a_file_and_updates_its_references() {
    let root = git_workspace("mcp-mv", &[("main.rs", RS_MOD), ("util.rs", RS_UTIL)]);
    let lsp = fake_lsp();
    let cwd = root.to_str().unwrap();
    let call = |id: u32, write: bool| {
        serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": "colab.mv", "arguments": {
                "from": "util.rs", "to": "helpers.rs", "lsp": lsp, "root": ".", "cwd": cwd, "write": write}}})
        .to_string()
    };
    let input = format!("{}\n{}\n", call(1, false), call(2, true));
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
    let text = |i: usize| -> Value {
        serde_json::from_str(replies[i]["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
    };
    // The preview names the move and the reference edit, and writes nothing.
    assert_eq!(text(0)["written"], false, "{}", text(0));
    assert!(text(0).to_string().contains("helpers.rs"), "{}", text(0));
    assert_eq!(text(1)["written"], true, "{}", text(1));
    assert_eq!(
        fs::read_to_string(root.join("main.rs")).unwrap(),
        "mod helpers;\nfn main() { helpers::go(); }\n"
    );
    assert!(git(&root, &["status", "--porcelain"]).contains("R  util.rs -> helpers.rs"));
    fs::remove_dir_all(&root).ok();
}

#[test]
fn split_pr_stacked_chains_rules_that_build_on_each_other() {
    let root = split_workspace("split-stacked");
    let rules = [
        "-e",
        r#"match go::import "old/log" { replace "new/log" }"#,
        "-e",
        r#"match go::import "new/log" { replace "newer/log" }"#,
    ];
    let out = colab()
        .current_dir(&root)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .args([
            "refactor",
            "--split-pr=stk",
            "--stacked",
            "--format",
            "json",
        ])
        .args(rules)
        .arg(".")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let second = v["branches"][1]["branch"].as_str().unwrap().to_string();
    // Rule 2 only matches rule 1's output, yet has its branch, on rule 1's.
    let first = v["branches"][0]["commit"].as_str().unwrap();
    assert_eq!(
        git(&root, &["rev-parse", &format!("{second}~1")]).trim(),
        first
    );
    assert!(git(&root, &["show", &format!("{second}:a.go")]).contains("\"newer/log\""));
    assert!(
        git(&root, &["status", "--short"]).trim().is_empty(),
        "tree untouched"
    );
    fs::remove_dir_all(&root).ok();
}

#[test]
fn verify_lsp_credits_an_error_to_the_rule_whose_change_holds_it() {
    let root = workspace_temp("verify-lsp-blame");
    write(
        &root.join("a.rs"),
        "fn old() {}\n\nfn main() {\n    let quiet = 1;\n}\n",
    );
    let out = colab()
        .current_dir(&root)
        .args(["refactor", "--write", "--format", "json"])
        .arg(format!("--verify-lsp={}", fake_lsp()))
        .args(["-e", r#"match rust::symbol "old" { replace "fresh" }"#])
        .args(["-e", r#"match rust::symbol "quiet" { replace "BROKEN" }"#])
        .arg("a.rs")
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(11),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: Value = serde_json::from_slice(&out.stdout).unwrap();
    let conflicts = json["conflicts"].as_array().unwrap();
    assert_eq!(conflicts.len(), 1, "{json}");
    // The fake server flags line 4, which only rule 2's rename touched.
    assert_eq!(conflicts[0]["rule"], 1, "{json}");
    fs::remove_dir_all(&root).ok();
}

/// Two docs and a README linking into the first.
fn mv_section_workspace(label: &str, target_dir: &str) -> PathBuf {
    let root = workspace_temp(label);
    write(
        &root.join("docs/plan.md"),
        "# Plan\n\nSee [the item](#old-item).\n\n## Planned\n\n### Old item\n\nText, as [kept](#keep); [detail](#detail).\n\n#### Detail\n\nMore.\n\n## Keep\n\nstays\n",
    );
    write(
        &root.join(target_dir).join("done.md"),
        "# Done\n\n## Shipped\n\n### Earlier\n\ndone\n",
    );
    write(
        &root.join("README.md"),
        "# Readme\n\n[item](docs/plan.md#old-item)\n",
    );
    root
}

#[test]
fn mv_section_moves_a_section_and_its_links() {
    let root = mv_section_workspace("mv-section", "docs");
    let out = colab()
        .current_dir(&root)
        .args([
            "mv-section",
            "docs/plan.md",
            "Old item",
            "--to",
            "docs/done.md",
            "--under",
            "Shipped",
            "--write",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let plan = fs::read_to_string(root.join("docs/plan.md")).unwrap();
    let done = fs::read_to_string(root.join("docs/done.md")).unwrap();
    assert!(
        !plan.contains("Old item") && plan.contains("[the item](done.md#old-item)"),
        "{plan}"
    );
    // Its own links now start from done.md: `#keep` stayed behind, so it
    // gains the path back; `#detail` moved with it.
    assert!(
        done.ends_with(
            "### Old item\n\nText, as [kept](plan.md#keep); [detail](#detail).\n\n#### Detail\n\nMore.\n"
        ),
        "{done}"
    );
    assert_eq!(
        fs::read_to_string(root.join("README.md")).unwrap(),
        "# Readme\n\n[item](docs/done.md#old-item)\n"
    );
    fs::remove_dir_all(&root).ok();
}

#[test]
fn mv_section_refuses_to_break_a_link_it_cannot_repoint() {
    // Across directories the README link's path would differ per file, so
    // it is not rewritten: the link check stops the move.
    let root = mv_section_workspace("mv-section-dirs", "archive");
    let out = colab()
        .current_dir(&root)
        .args([
            "mv-section",
            "docs/plan.md",
            "Old item",
            "--to",
            "archive/done.md",
            "--write",
        ])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(11),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("README.md"));
    assert!(
        fs::read_to_string(root.join("docs/plan.md"))
            .unwrap()
            .contains("### Old item")
    );
    // The script is there to read.
    let script = colab()
        .current_dir(&root)
        .args([
            "mv-section",
            "docs/plan.md",
            "Old item",
            "--to",
            "archive/done.md",
            "--show-script",
        ])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&script.stdout);
    assert!(
        text.contains(r#"replace "../archive/done.md#old-item""#),
        "{text}"
    );
    fs::remove_dir_all(&root).ok();
}

#[test]
fn baseline_reports_only_findings_new_since_the_ref() {
    let root = split_workspace("baseline");
    let rule = r#"match go::call "log.Old" { replace_call "log.New($args)" }"#;
    let check = |root: &std::path::Path| {
        colab()
            .current_dir(root)
            .args([
                "refactor",
                "-e",
                rule,
                "--baseline",
                "HEAD",
                "--check",
                "--format",
                "json",
                ".",
            ])
            .output()
            .unwrap()
    };
    // Both calls were there at HEAD: nothing is new, and the rule, which
    // matched them, is not reported as matching nothing.
    let clean = check(&root);
    assert_eq!(
        clean.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&clean.stderr)
    );
    let doc: serde_json::Value = serde_json::from_slice(&clean.stdout).unwrap();
    let warnings = doc["warnings"].to_string();
    assert!(warnings.contains("already at HEAD"), "{doc}");
    assert!(!warnings.contains("matched no files"), "{doc}");
    assert!(!warnings.contains("no rule matched"), "{doc}");
    // A new call, and the old ones moved down a line.
    write(
        &root.join("b.go"),
        "package p\n\n// moved\nfunc G() { log.Old(2) }\n\nfunc H() { log.Old(3) }\n",
    );
    let out = check(&root);
    assert_eq!(out.status.code(), Some(10));
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let changed = doc["changed"].as_array().unwrap();
    assert_eq!(changed.len(), 1, "{doc}");
    assert_eq!(changed[0], "./b.go");
    assert!(
        doc["warnings"].to_string().contains("already at HEAD"),
        "{doc}"
    );
    // No worktree is left behind.
    assert_eq!(git(&root, &["worktree", "list"]).lines().count(), 1);
    let refused = colab()
        .current_dir(&root)
        .args(["refactor", "-e", rule, "--baseline", "HEAD", "--write", "."])
        .output()
        .unwrap();
    assert!(!refused.status.success());
    fs::remove_dir_all(&root).ok();
}

#[test]
fn the_renovate_preset_runs_colab_fix_once_per_branch() {
    let text = fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../renovate/colab.json"),
    )
    .unwrap();
    let preset: serde_json::Value = serde_json::from_str(&text).unwrap();
    let tasks = &preset["postUpgradeTasks"];
    assert_eq!(tasks["commands"], serde_json::json!(["colab fix --write"]));
    assert_eq!(tasks["executionMode"], "branch");
    // The command must parse as colab's own.
    let help = colab().args(["fix", "--write", "--help"]).output().unwrap();
    assert!(help.status.success());
}

/// Two packages with a `Print` each, and a file using both.
fn semantic_workspace(label: &str) -> PathBuf {
    let root = workspace_temp(label);
    write(
        &root.join("alog/log.go"),
        "package alog\n\nfunc Print(s string) {}\n",
    );
    write(
        &root.join("blog/log.go"),
        "package blog\n\nfunc Print(s string) {}\n",
    );
    write(
        &root.join("main.go"),
        "package main\n\nvar r io.Reader\nvar f *os.File\n\nfunc main() {\n\talog.Print(\"a\")\n\tblog.Print(\"b\")\n\tr.Close()\n\tf.Close()\n}\n",
    );
    root
}

#[test]
fn type_and_resolves_constraints_are_answered_by_the_server() {
    let root = semantic_workspace("semantic");
    let rules = [
        "-e",
        r#"match go::pattern '$F($$$A)' where $F resolves "alog.Print" { replace 'logx.Print($$$A)' }"#,
        "-e",
        r#"match go::pattern '$X.Close()' where $X type "*os.File" { replace 'closeFile($X)' }"#,
    ];
    let run = |extra: &[&str]| {
        colab()
            .current_dir(&root)
            .args(["refactor"])
            .args(rules)
            .args(["--lsp", &fake_lsp(), "--format", "diff"])
            .args(extra)
            .arg("main.go")
            .output()
            .unwrap()
    };
    let out = run(&[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let diff = String::from_utf8_lossy(&out.stdout);
    assert!(diff.contains("+\tlogx.Print(\"a\")"), "{diff}");
    assert!(
        !diff.contains("-\tblog.Print"),
        "blog.Print is not alog's: {diff}"
    );
    assert!(diff.contains("+\tcloseFile(f)"), "{diff}");
    assert!(
        !diff.contains("-\tr.Close()"),
        "an io.Reader is not an *os.File: {diff}"
    );
    // `find` agrees.
    let found = colab()
        .current_dir(&root)
        .args(["find"])
        .args(rules)
        .args(["--lsp", &fake_lsp(), "main.go"])
        .output()
        .unwrap();
    let found = String::from_utf8_lossy(&found.stdout);
    assert!(
        found.contains("main.go:7:2") && found.contains("main.go:10:2"),
        "{found}"
    );
    assert!(
        !found.contains("main.go:8:") && !found.contains("main.go:9:"),
        "{found}"
    );
    // No server: the run fails, and says why; nothing is written.
    let failed = colab()
        .current_dir(&root)
        .args(["refactor"])
        .args(rules)
        .args(["--lsp", "no-such-language-server", "--write", "main.go"])
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert!(
        String::from_utf8_lossy(&failed.stderr).contains("no-such-language-server"),
        "{}",
        String::from_utf8_lossy(&failed.stderr)
    );
    assert!(
        fs::read_to_string(root.join("main.go"))
            .unwrap()
            .contains("alog.Print")
    );
    fs::remove_dir_all(&root).ok();
}

#[test]
fn calls_called_by_and_implements_are_answered_by_the_server_hierarchies() {
    let root = workspace_temp("hierarchy");
    write(
        &root.join("alog/log.go"),
        "package alog\n\ntype Writer interface{}\n\nfunc Print(s string) {}\n",
    );
    write(
        &root.join("blog/log.go"),
        "package blog\n\nfunc Print(s string) {}\n",
    );
    write(
        &root.join("main.go"),
        "package main\n\ntype T struct{} // implements: alog.Writer\n\ntype U struct{}\n\nfunc a() {\n\talog.Print(\"a\")\n}\n\nfunc b() {\n\tblog.Print(\"b\")\n}\n\nfunc c() {\n\ta()\n}\n",
    );
    let find = |rule: &str| {
        let out = colab()
            .current_dir(&root)
            .args(["find", "-e", rule, "--lsp", &fake_lsp(), "main.go"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let lines: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| l.split(": ").next().filter(|p| p.starts_with("main.go:")))
            .map(str::to_string)
            .collect();
        lines
    };
    // `a` calls alog's Print; `b` calls blog's, a different function.
    assert_eq!(
        find(r#"match go::pattern 'func $F() { $$$B }' where $F calls "alog.Print" { delete }"#),
        ["main.go:7:1"]
    );
    // Only `c` calls `a`.
    assert_eq!(
        find(r#"match go::pattern 'func $F() { $$$B }' where $F called_by "main.c" { delete }"#),
        ["main.go:7:1"]
    );
    assert_eq!(
        find(
            r#"match go::pattern 'func $F() { $$$B }' where not $F called_by "main.c" { delete }"#
        ),
        ["main.go:11:1", "main.go:15:1"]
    );
    // `T` implements alog.Writer; `U` implements nothing.
    assert_eq!(
        find(
            r#"match go::pattern 'type $T struct{}' where $T implements "alog.Writer" { delete }"#
        ),
        ["main.go:3:1"]
    );

    // The same answers as navigation commands.
    let list = |args: &[&str]| {
        let out = colab()
            .current_dir(&root)
            .args(args)
            .args(["--lsp", &fake_lsp(), "--format", "json"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()
    };
    let callers = list(&["calls", "--symbol", "Print", "--in", "alog/log.go"]);
    assert_eq!(callers["callers"][0]["path"], "main.go");
    assert_eq!(callers["callers"][0]["text"], "func a() {");
    assert_eq!(
        callers["summary"]["callers"], 1,
        "blog.Print's caller is not alog's: {callers}"
    );
    let callees = list(&["calls", "--symbol", "c", "--in", "main.go", "--outgoing"]);
    assert_eq!(callees["callees"][0]["text"], "func a() {");
    let implementations = list(&[
        "implementations",
        "--symbol",
        "Writer",
        "--in",
        "alog/log.go",
    ]);
    assert_eq!(implementations["implementations"][0]["path"], "main.go");
    assert_eq!(implementations["implementations"][0]["line"], 3);
    assert_eq!(implementations["summary"]["implementations"], 1);
    fs::remove_dir_all(&root).ok();
}

#[test]
fn a_git_pack_is_fetched_at_its_pin_and_its_own_cases_must_pass() {
    let shared = workspace_temp("git-pack-shared");
    write(
        &shared.join("go/errors.colab"),
        r#"refactor "errors" { match go::import "github.com/pkg/errors" { replace "errors" } }"#,
    );
    write(
        &shared.join("go/errors.tests/renames/input/a.go"),
        "package p\n\nimport \"github.com/pkg/errors\"\n",
    );
    write(
        &shared.join("go/errors.tests/renames/expected/a.go"),
        "package p\n\nimport \"errors\"\n",
    );
    let commit = |message: &str| {
        git(&shared, &["add", "-A"]);
        git(
            &shared,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "-m",
                message,
            ],
        );
    };
    git(&shared, &["init", "-q", "-b", "main"]);
    commit("v1");
    git(&shared, &["tag", "v1"]);
    let v1 = git(&shared, &["rev-parse", "HEAD"]).trim().to_string();
    // v2's case expects the wrong thing.
    write(
        &shared.join("go/errors.tests/renames/expected/a.go"),
        "package p\n\nimport \"wrong\"\n",
    );
    commit("v2");
    git(&shared, &["tag", "v2"]);

    let app = workspace_temp("git-pack-app");
    let cache = workspace_temp("git-pack-cache");
    write(
        &app.join("main.go"),
        "package main\n\nimport \"github.com/pkg/errors\"\n",
    );
    let url = format!("file://{}", shared.display());
    let fix = |pin: &str| {
        write(
            &app.join("colab.toml"),
            &format!("[[packs]]\ngit = \"{url}\"\n{pin}\npath = \"go/errors.colab\"\n"),
        );
        colab()
            .current_dir(&app)
            .env("COLAB_CACHE_DIR", &cache)
            .args(["fix", "--check", "-C", "."])
            .output()
            .unwrap()
    };
    git(&app, &["init", "-q"]);
    assert_eq!(fix("tag = \"v1\"").status.code(), Some(10));
    assert_eq!(fix(&format!("rev = \"{v1}\"")).status.code(), Some(10));
    let refused = fix("tag = \"v2\"");
    assert!(!refused.status.success());
    let err = String::from_utf8_lossy(&refused.stderr);
    assert!(
        err.contains("fails its own tests") && err.contains("renames"),
        "{err}"
    );
    // Cached: the shared repository can go, the pinned pack still runs.
    fs::remove_dir_all(&shared).unwrap();
    assert_eq!(fix("tag = \"v1\"").status.code(), Some(10));
    for dir in [&app, &cache] {
        fs::remove_dir_all(dir).ok();
    }
}

#[test]
fn a_fleet_runs_each_repository_in_a_worktree_canary_first() {
    let root = workspace_temp("fleet");
    for name in ["api", "billing"] {
        let repo = root.join(name);
        write(
            &repo.join("a.go"),
            "package p\n\nimport \"github.com/pkg/errors\"\n",
        );
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["add", "-A"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "-m",
                "init",
            ],
        );
    }
    write(
        &root.join("m.colab"),
        r#"refactor "m" { match go::import "github.com/pkg/errors" { replace "errors" } }"#,
    );
    write(
        &root.join("fleet.toml"),
        "[fleet]\nscript = \"m.colab\"\nbranch = \"colab/errors\"\ntitle = \"Use errors\"\ncanary = \"billing\"\n\n[[repos]]\npath = \"api\"\n\n[[repos]]\npath = \"billing\"\n",
    );
    let fleet = |args: &[&str]| {
        colab()
            .current_dir(&root)
            .args(["fleet", "fleet.toml", "--format", "json"])
            .args(args)
            // CI runners have no git identity; the fleet commits.
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap()
    };
    let doc = |out: &std::process::Output| -> serde_json::Value {
        serde_json::from_slice(&out.stdout).unwrap()
    };
    // A dry run: both would change, the canary listed first, nothing written.
    let dry = fleet(&[]);
    assert!(
        dry.status.success(),
        "{}",
        String::from_utf8_lossy(&dry.stderr)
    );
    let d = doc(&dry);
    assert_eq!(d["repos"][0]["repo"], "billing");
    assert_eq!(d["repos"][1]["status"], "changed");
    assert!(
        fs::read_to_string(root.join("api/a.go"))
            .unwrap()
            .contains("pkg/errors")
    );
    // --write: a commit on the branch each; the checkout itself untouched.
    let wrote = fleet(&["--write"]);
    assert!(
        wrote.status.success(),
        "{}{}",
        String::from_utf8_lossy(&wrote.stdout),
        String::from_utf8_lossy(&wrote.stderr)
    );
    for name in ["api", "billing"] {
        let repo = root.join(name);
        assert!(git(&repo, &["show", "colab/errors:a.go"]).contains("import \"errors\""));
        assert_eq!(git(&repo, &["branch", "--show-current"]).trim(), "main");
        assert!(git(&repo, &["status", "--short"]).is_empty());
        assert_eq!(git(&repo, &["worktree", "list"]).lines().count(), 1);
    }
    // Again: the branch exists, the canary fails, the rest is skipped.
    let again = fleet(&["--write"]);
    assert_eq!(again.status.code(), Some(1));
    let a = doc(&again);
    assert_eq!(a["repos"][0]["status"], "failed");
    assert!(
        a["repos"][0]["error"]
            .as_str()
            .unwrap()
            .contains("already exists")
    );
    assert_eq!(a["repos"][1]["status"], "skipped");
    fs::remove_dir_all(&root).ok();
}

#[test]
fn edit_finds_a_declaration_by_name_path_and_previews_like_any_run() {
    let root = workspace_temp("edit-symbol");
    let file = root.join("lib.rs");
    write(
        &file,
        "struct A;\nimpl A {\n    /// Loads.\n    fn load(&self) {}\n}\nstruct B;\nimpl B {\n    fn load(&self) {}\n}\n",
    );
    let edit = |args: &[&str]| {
        colab()
            .current_dir(&root)
            .args(["edit", "--in", "lib.rs"])
            .args(args)
            .output()
            .unwrap()
    };
    // The file's name paths.
    let listed = edit(&["--list", "--format", "json"]);
    let doc: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    let paths: Vec<&str> = doc["declarations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["A", "A", "A/load", "B", "B", "B/load"]);
    // `load` alone is ambiguous: the candidates are listed.
    let ambiguous = edit(&[
        "--symbol",
        "load",
        "--replace-body",
        "--text",
        "fn load() {}",
    ]);
    assert!(!ambiguous.status.success());
    let err = String::from_utf8_lossy(&ambiguous.stderr);
    assert!(
        err.contains("`A/load` (line 3)") && err.contains("`B/load` (line 8)"),
        "{err}"
    );
    // A preview changes nothing on disk.
    let preview = edit(&[
        "--symbol",
        "B/load",
        "--replace-body",
        "--text",
        "fn load(&self) {\n    todo!()\n}",
        "--format",
        "diff",
    ]);
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stderr)
    );
    let diff = String::from_utf8_lossy(&preview.stdout);
    assert!(diff.contains("+        todo!()"), "{diff}");
    assert!(!fs::read_to_string(&file).unwrap().contains("todo!"));
    // Written: the doc comment goes with A/load when it is replaced.
    let wrote = edit(&[
        "--symbol",
        "A/load",
        "--replace-body",
        "--text",
        "fn load(&self) -> u8 { 1 }",
        "--write",
    ]);
    assert!(wrote.status.success());
    let after = fs::read_to_string(&file).unwrap();
    assert!(
        after.contains("impl A {\n    fn load(&self) -> u8 { 1 }\n}"),
        "{after}"
    );
    let inserted = edit(&[
        "--symbol",
        "B/load",
        "--insert-after",
        "--text",
        "fn save(&self) {}",
        "--write",
    ]);
    assert!(inserted.status.success());
    let after = fs::read_to_string(&file).unwrap();
    assert!(
        after.contains("    fn load(&self) {}\n\n    fn save(&self) {}\n}"),
        "{after}"
    );
    fs::remove_dir_all(&root).ok();
}

#[test]
fn colab_toml_lists_the_projects_expression_macros() {
    let root = workspace_temp("project-macros");
    write(
        &root.join("lib.rs"),
        "fn t(p: &Path) {\n    my_assert!(is_relevant(p));\n}\n",
    );
    let run = || {
        colab()
            .current_dir(&root)
            .args([
                "refactor",
                "-e",
                "match rust::pattern 'is_relevant($P)' { replace 'LANG.is_source($P)' }",
                "--format",
                "diff",
                ".",
            ])
            .output()
            .unwrap()
    };
    // Unlisted, `my_assert!`'s contents are a token tree no pattern reads.
    assert!(!String::from_utf8_lossy(&run().stdout).contains("LANG.is_source"));
    write(
        &root.join("colab.toml"),
        "[patterns]\nmacros = [\"my_assert\"]\n",
    );
    let out = run();
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("+    my_assert!(LANG.is_source(p));"),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    fs::remove_dir_all(&root).ok();
}

#[test]
fn a_script_file_of_bare_rules_runs_named_after_the_file() {
    let root = workspace_temp("bare-script");
    write(
        &root.join("main.go"),
        "package main\n\nimport \"old/pkg\"\n\nfunc main() {}\n",
    );
    write(
        &root.join("move.colab"),
        "// no refactor block around these\nmatch go::import \"old/pkg\" { replace \"new/pkg\" }\n",
    );
    let out = colab()
        .current_dir(&root)
        .args([
            "refactor",
            "--script",
            "move.colab",
            "--format",
            "diff",
            ".",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("+import \"new/pkg\""));
    assert!(String::from_utf8_lossy(&out.stderr).contains("Running `move`"));
    // A parse error points into the file as written: line 2.
    write(
        &root.join("bad.colab"),
        "match go::import \"a\" { replace \"b\" }\nmatch go::import \"a\" { nope }\n",
    );
    let bad = colab()
        .current_dir(&root)
        .args(["refactor", "--script", "bad.colab", "."])
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&bad.stderr)
            .contains("parse error in bad.colab at line 2, column 24"),
        "{}",
        String::from_utf8_lossy(&bad.stderr)
    );
    fs::remove_dir_all(&root).ok();
}

#[test]
fn a_scan_reads_the_whole_tree_when_the_run_changes_only_what_git_names() {
    let root = git_workspace(
        "scan-changed-since",
        &[
            ("api/api.go", "package api\n\nfunc Exported() {}\n"),
            ("app/main.go", "package main\n\nfunc main() {}\n"),
        ],
    );
    // Only main.go changes since HEAD; the scan must still read api.go.
    write(
        &root.join("app/main.go"),
        "package main\n\nfunc main() {\n\tapi.Exported()\n}\n",
    );
    write(
        &root.join("s.colab"),
        r#"scan go::pattern 'func $F() {}' in "api/**" where $F matches /^[A-Z]/ as exported
match go::pattern 'api.$F()' where $F in exported { replace 'api.$F(nil)' }
"#,
    );
    let out = colab()
        .current_dir(&root)
        .args([
            "refactor",
            "--script",
            "s.colab",
            "--changed-since",
            "HEAD",
            "--format",
            "json",
            ".",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["changed"], serde_json::json!(["app/main.go"]), "{doc}");
    fs::remove_dir_all(&root).ok();
}

#[test]
fn a_scan_reads_every_file_before_a_rule_edits_one() {
    let root = workspace_temp("scan-phases");
    write(
        &root.join("api/api.go"),
        "package api\n\nfunc Exported() {}\n\nfunc internal() {}\n",
    );
    write(
        &root.join("app/main.go"),
        "package main\n\nfunc main() {\n\tapi.Exported()\n\tapi.internal()\n}\n",
    );
    let script = root.join("s.colab");
    write(
        &script,
        r#"refactor "s" {
    scan go::pattern 'func $F() {}' in "api/**" where $F matches /^[A-Z]/ as exported
    match go::pattern 'api.$F()' where $F in exported { replace 'api.$F(nil)' }
}"#,
    );
    let run = |args: &[&str]| {
        colab()
            .current_dir(&root)
            .args(["refactor", "--script", "s.colab", "--format", "diff"])
            .args(args)
            .arg(".")
            .output()
            .unwrap()
    };
    let out = run(&[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let diff = String::from_utf8_lossy(&out.stdout);
    assert!(diff.contains("+\tapi.Exported(nil)"), "{diff}");
    assert!(!diff.contains("api.internal(nil)"), "{diff}");
    // `find` reads the same facts.
    let found = colab()
        .current_dir(&root)
        .args(["find", "--script", "s.colab", "."])
        .output()
        .unwrap();
    let found = String::from_utf8_lossy(&found.stdout);
    assert!(
        found.contains("main.go:4:2") && !found.contains("main.go:5:"),
        "{found}"
    );
    // The runs that apply one rule at a time read the same facts: the
    // scans run once, over the whole tree, first.
    let verified = run(&["--verify", "true"]);
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    let main = fs::read_to_string(root.join("app/main.go")).unwrap();
    assert!(
        main.contains("api.Exported(nil)") && !main.contains("api.internal(nil)"),
        "{main}"
    );
    write(
        &root.join("app/main.go"),
        "package main\n\nfunc main() {\n\tapi.Exported()\n\tapi.internal()\n}\n",
    );
    // `--stdin` runs it over the one text: its scans read that text.
    let mut child = colab()
        .current_dir(&root)
        .args([
            "refactor", "--script", "s.colab", "--stdin", "--path", "api/x.go",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"package api\n\nfunc Exported() {}\n\nfunc f() { api.Exported() }\n")
            .unwrap();
    }
    let piped = child.wait_with_output().unwrap();
    assert!(
        piped.status.success(),
        "{}",
        String::from_utf8_lossy(&piped.stderr)
    );
    assert!(String::from_utf8_lossy(&piped.stdout).contains("api.Exported(nil)"));
    fs::remove_dir_all(&root).ok();
}
