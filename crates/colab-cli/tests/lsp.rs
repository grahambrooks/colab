//! End-to-end tests for the commands that ask a language server
//! (`rename`, `mv`, `action`, `references`, `safe-delete`,
//! `--verify-lsp`, `--tidy`, the MCP language-server tools), against the
//! fake server from `colab-lsp-client`'s examples.

mod common;
use common::*;

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

#[test]
fn an_action_whose_edit_adds_an_error_is_refused_with_verify_lsp() {
    // "Uppercase file" turns `broken` into `BROKEN`, on which the fake
    // server reports an error: --verify-lsp refuses the write.
    let text = "fn broken() {}\n";
    let root = git_workspace("action-verify-lsp", &[("a.rs", text)]);
    let run = |extra: &[&str]| {
        let mut args = vec![
            "action",
            "--at",
            "a.rs:1:4",
            "--title",
            "Uppercase file",
            "--write",
        ];
        args.extend_from_slice(extra);
        lsp_cmd(&root, &args)
    };
    let verify = format!("--verify-lsp={}", fake_lsp());
    let refused = run(&[verify.as_str()]);
    assert_eq!(
        refused.status.code(),
        Some(11),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert_eq!(fs::read_to_string(root.join("a.rs")).unwrap(), text);
    // Without it the edit is written.
    let written = run(&[]);
    assert_eq!(written.status.code(), Some(0));
    assert_eq!(
        fs::read_to_string(root.join("a.rs")).unwrap(),
        "FN BROKEN() {}\n"
    );
    fs::remove_dir_all(&root).ok();
}

#[test]
fn an_action_that_creates_a_file_writes_it() {
    // rust-analyzer's "Extract module to file" creates a file and fills it
    // in one edit; the file is not on disk when the write is checked, so
    // it must not count as one deleted since colab read it.
    let root = git_workspace("action-create", &[("a.rs", RS_ACT_A)]);
    let output = lsp_cmd(
        &root,
        &[
            "action",
            "--at",
            "a.rs:6:13",
            "--title",
            "Extract to new file",
            "--write",
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(root.join("extracted.txt")).unwrap(),
        "extracted\n"
    );
    fs::remove_dir_all(&root).ok();
}

#[test]
fn an_action_the_server_resolved_against_a_stale_file_is_listed_again() {
    // The fake answers the first resolve of this action as rust-analyzer
    // does when the file changed under it: "Mismatching assist …".
    let root = action_workspace("action-stale");
    let output = lsp_cmd(
        &root,
        &[
            "action",
            "--at",
            "a.rs:1:1",
            "--title",
            "Sort imports (flaky)",
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
            .starts_with("use alpha;\nuse zeta;\n")
    );
    fs::remove_dir_all(&root).ok();
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
            "Organize imports",
            "Sort imports (flaky)",
            "Extract to new file"
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
    // One rule changed the file, so the error is that rule's.
    assert_eq!(conflicts[0]["rule"], 0, "{json}");
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

#[test]
fn mcp_rename_symbol_offers_text_occurrences_and_includes_them_on_request() {
    let root = workspace_temp("mcp-rename-symbol");
    write(
        &root.join("a.rs"),
        "fn helper() {}\nfn main() { helper(); }\n",
    );
    write(&root.join("README.md"), "Call `helper` to help.\n");
    // Go is not the server's language: only the comment is prose.
    let go = "package p\n\n// helper is mirrored here\nfunc helper() {}\n";
    write(&root.join("tool.go"), go);
    let lsp = fake_lsp();
    let cwd = root.to_str().unwrap();
    let call = |id: u32, include: bool| {
        serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": "colab.rename_symbol", "arguments": {
                "symbol": "helper", "in": "a.rs", "to": "assist", "lsp": lsp,
                "root": ".", "cwd": cwd, "include_text": include, "write": include}}})
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

    let preview = text(0);
    assert_eq!(preview["changed"], serde_json::json!(["a.rs"]), "{preview}");
    assert_eq!(preview["text"]["files"], 2, "{preview}");
    assert_eq!(preview["text"]["included"], false);
    let sample: Vec<&str> = preview["text"]["sample"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["path"].as_str().unwrap())
        .collect();
    assert_eq!(sample, ["README.md", "tool.go"], "{preview}");

    let applied = text(1);
    assert_eq!(applied["written"], true, "{applied}");
    assert_eq!(applied["rules"][1]["files"], 2, "{applied}");
    assert_eq!(
        fs::read_to_string(root.join("README.md")).unwrap(),
        "Call `assist` to help.\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("tool.go")).unwrap(),
        go.replace("// helper", "// assist"),
        "the Go function is another symbol and keeps its name"
    );
    assert!(
        fs::read_to_string(root.join("a.rs"))
            .unwrap()
            .contains("fn assist()")
    );
    fs::remove_dir_all(&root).ok();
}
