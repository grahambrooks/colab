//! End-to-end tests for `colab agent install` / `uninstall`, the post-edit
//! hook, and telemetry.

mod common;
use common::*;

#[test]
fn agent_install_is_idempotent_and_uninstall_takes_back_only_its_own() {
    let project = workspace_temp("agent-project");
    let home = workspace_temp("agent-home");
    // Things of the user's that must survive both.
    let settings = "{\n  \"model\": \"opus\"\n}\n";
    write(&project.join(".claude/settings.json"), settings);
    let agents = "# Notes\n\nKeep this.\n";
    write(&project.join("AGENTS.md"), agents);
    let codex = "model = \"o4\"\n";
    write(&home.join(".codex/config.toml"), codex);
    let agent = |action: &str| {
        colab()
            .env("HOME", &home)
            .args([
                "agent", action, "--claude", "--cursor", "--codex", "--format", "json", "-C",
            ])
            .arg(&project)
            .output()
            .unwrap()
    };
    let statuses = |out: &std::process::Output| -> Vec<String> {
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        doc["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                format!(
                    "{} {}",
                    f["path"].as_str().unwrap(),
                    f["status"].as_str().unwrap()
                )
            })
            .collect()
    };
    let snapshot = |root: &std::path::Path| -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).unwrap().flatten() {
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p.clone());
                    out.push((
                        p.strip_prefix(root).unwrap().display().to_string() + "/",
                        String::new(),
                    ));
                } else {
                    out.push((
                        p.strip_prefix(root).unwrap().display().to_string(),
                        fs::read_to_string(&p).unwrap(),
                    ));
                }
            }
        }
        out.sort();
        out
    };
    let before = (snapshot(&project), snapshot(&home));

    let first = statuses(&agent("install"));
    assert!(first.iter().any(|s| s == ".mcp.json created"), "{first:?}");
    assert!(
        first.iter().any(|s| s == ".claude/settings.json updated"),
        "{first:?}"
    );
    let installed = (snapshot(&project), snapshot(&home));
    // Idempotent: a second install changes nothing, byte for byte.
    let second = statuses(&agent("install"));
    assert!(
        second.iter().all(|s| s.ends_with(" unchanged")),
        "{second:?}"
    );
    assert_eq!((snapshot(&project), snapshot(&home)), installed);
    let settings_now: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(project.join(".claude/settings.json")).unwrap())
            .unwrap();
    assert_eq!(settings_now["model"], "opus");
    assert_eq!(
        settings_now["hooks"]["PostToolUse"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // Uninstall: everything as it was, nothing of colab's left; twice is fine.
    statuses(&agent("uninstall"));
    assert_eq!((snapshot(&project), snapshot(&home)), before);
    let again = statuses(&agent("uninstall"));
    assert!(again.iter().all(|s| s.ends_with(" unchanged")), "{again:?}");
    for dir in [&project, &home] {
        fs::remove_dir_all(dir).ok();
    }
}

/// `--user`: Claude Code's MCP server through its own `claude mcp`
/// (faked here, keeping `~/.claude.json` as the real one does), the skill
/// in `~/.claude/skills/`, no hook; Cursor and Codex in their home files.
#[cfg(unix)]
#[test]
fn agent_install_user_scope_goes_through_claude_mcp_and_writes_no_hook() {
    use std::os::unix::fs::PermissionsExt;
    let home = workspace_temp("agent-user-home");
    let bin = workspace_temp("agent-user-bin");
    let log = bin.join("claude.log");
    let fake = format!(
        "#!/bin/sh\necho \"$*\" >> '{log}'\ncase \"$2\" in\n  add) printf '{{\"numStartups\": 3, \"mcpServers\": {{\"colab\": {{\"type\": \"stdio\", \"command\": \"colab\", \"args\": [\"mcp\"], \"env\": {{}}}}}}}}' > \"$HOME/.claude.json\" ;;\n  remove) printf '{{\"numStartups\": 3, \"mcpServers\": {{}}}}' > \"$HOME/.claude.json\" ;;\nesac\n",
        log = log.display()
    );
    write(&bin.join("claude"), &fake);
    fs::set_permissions(bin.join("claude"), fs::Permissions::from_mode(0o755)).unwrap();
    write(&home.join(".claude.json"), "{\"numStartups\": 3}");
    write(
        &home.join(".claude/settings.json"),
        "{\"model\": \"opus\"}\n",
    );
    let agent = |action: &str| {
        let out = colab()
            .env("HOME", &home)
            .env_remove("CLAUDE_CONFIG_DIR")
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .args([
                "agent", action, "--user", "--claude", "--cursor", "--codex", "--format", "json",
            ])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()
    };
    let calls = || fs::read_to_string(&log).unwrap_or_default();

    let first = agent("install");
    assert_eq!(first["scope"], "user");
    assert_eq!(first["commands"][0]["status"], "created");
    assert_eq!(calls(), "mcp add --scope user colab -- colab mcp\n");
    let paths: Vec<&str> = first["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert!(
        paths.contains(&"~/.claude/skills/colab-codemod/SKILL.md")
            && paths.contains(&"~/.cursor/mcp.json")
            && paths.contains(&"~/.codex/config.toml")
            && paths.contains(&"~/.codex/AGENTS.md"),
        "{paths:?}"
    );
    assert!(home.join(".claude/skills/colab-codemod/SKILL.md").is_file());
    assert_eq!(
        fs::read_to_string(home.join(".claude/settings.json")).unwrap(),
        "{\"model\": \"opus\"}\n",
        "no hook in user scope"
    );

    // A second install runs nothing and writes nothing.
    let second = agent("install");
    assert_eq!(second["commands"][0]["status"], "unchanged");
    assert!(second["commands"][0]["command"].is_null());
    assert!(
        second["files"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["status"] == "unchanged"),
        "{second}"
    );
    assert_eq!(calls().lines().count(), 1);

    // Uninstall: `claude mcp remove`, and colab's files go.
    let removed = agent("uninstall");
    assert_eq!(removed["commands"][0]["status"], "removed");
    assert_eq!(
        calls().lines().last(),
        Some("mcp remove --scope user colab")
    );
    assert!(!home.join(".claude/skills").exists());
    assert!(!home.join(".cursor").exists() && !home.join(".codex").exists());
    assert!(home.join(".claude/settings.json").is_file());
    for dir in [&home, &bin] {
        fs::remove_dir_all(dir).ok();
    }
}

#[test]
fn agent_user_scope_conflicts_with_a_project() {
    let out = colab()
        .args(["agent", "install", "--user", "-C", ".", "--dry-run"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot be used with"));
}

#[test]
fn the_post_edit_hook_reports_a_broken_convention_to_the_model() {
    let root = workspace_temp("post-edit");
    write(&root.join(".git/HEAD"), "ref: refs/heads/main\n");
    write(
        &root.join("colab.toml"),
        "[[packs]]\npath = \"packs/p.colab\"\n",
    );
    write(
        &root.join("packs/p.colab"),
        r#"refactor "p" { match go::import "github.com/pkg/errors" { replace "errors" message "use the standard errors package" id "std-errors" } }"#,
    );
    let file = root.join("svc/a.go");
    write(&file, "package svc\n\nimport \"github.com/pkg/errors\"\n");
    let hook = |path: &std::path::Path| {
        let mut child = colab()
            .args(["hook", "post-edit"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let call = serde_json::json!({"tool_name": "Write", "tool_input": {"file_path": path}});
        std::io::Write::write_all(child.stdin.as_mut().unwrap(), call.to_string().as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    };
    let out = hook(&file);
    assert!(out.status.success());
    let decision: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(decision["decision"], "block");
    let reason = decision["reason"].as_str().unwrap();
    assert!(
        reason.contains("svc/a.go:3: error: use the standard errors package [std-errors]"),
        "{reason}"
    );
    // Clean, or outside any colab.toml: nothing at all.
    write(&file, "package svc\n\nimport \"errors\"\n");
    assert!(hook(&file).stdout.is_empty());
    let outside = workspace_temp("post-edit-outside");
    write(
        &outside.join("b.go"),
        "package b\n\nimport \"github.com/pkg/errors\"\n",
    );
    assert!(hook(&outside.join("b.go")).stdout.is_empty());
    for dir in [&root, &outside] {
        fs::remove_dir_all(dir).ok();
    }
}

/// A collector on a free loopback port: answers every OTLP/HTTP POST with
/// 200 and hands back the bodies it received.
fn collector() -> (String, std::sync::mpsc::Receiver<serde_json::Value>) {
    use std::io::{Read, Write as _};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut seen = Vec::new();
            let mut buf = [0u8; 8192];
            let body = loop {
                let Ok(n) = stream.read(&mut buf) else {
                    break None;
                };
                if n == 0 {
                    break None;
                }
                seen.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&seen).into_owned();
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let len: usize = head
                        .lines()
                        .find_map(|l| l.strip_prefix("Content-Length: "))
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(0);
                    if body.len() >= len {
                        break Some(body.to_string());
                    }
                }
            };
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            if let Some(body) = body.and_then(|b| serde_json::from_str(&b).ok()) {
                let _ = send.send(body);
            }
        }
    });
    (endpoint, receive)
}

#[test]
fn a_command_is_one_otlp_trace_when_an_endpoint_is_set() {
    let root = workspace_temp("otel");
    write(&root.join("main.go"), GO_INPUT);
    let (endpoint, traces) = collector();
    let out = colab()
        .current_dir(&root)
        .env_remove("OTEL_SDK_DISABLED")
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", &endpoint)
        .env("OTEL_RESOURCE_ATTRIBUTES", "deployment.environment=test")
        .args([
            "find",
            "-e",
            r#"match go::import "old.module" { replace "new.module" }"#,
        ])
        .args(["--format", "json", "main.go"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let body = traces
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("the trace reached the collector");
    let resource = body["resourceSpans"][0]["resource"]["attributes"].to_string();
    assert!(resource.contains(r#""stringValue":"colab""#), "{resource}");
    assert!(resource.contains("deployment.environment"), "{resource}");
    let spans = body["resourceSpans"][0]["scopeSpans"][0]["spans"]
        .as_array()
        .unwrap()
        .clone();
    let named = |name: &str| {
        spans
            .iter()
            .find(|s| s["name"] == name)
            .unwrap_or_else(|| panic!("no `{name}` span in {spans:?}"))
    };
    let command = named("colab find");
    assert!(command.get("parentSpanId").is_none(), "the root: {command}");
    for child in ["compile", "walk"] {
        assert_eq!(named(child)["parentSpanId"], command["spanId"], "{child}");
        assert_eq!(named(child)["traceId"], command["traceId"], "{child}");
    }
    let attributes = command["attributes"].to_string();
    assert!(attributes.contains("process.exit.code"), "{attributes}");
    assert!(
        attributes.contains(r#""format""#),
        "flag names: {attributes}"
    );
    // Flag names only: no paths, rules or flag values.
    let whole = body.to_string();
    assert!(!whole.contains("main.go"), "a path in the trace: {whole}");
    assert!(
        !whole.contains("old.module"),
        "the rule in the trace: {whole}"
    );
    assert!(
        !attributes.contains(r#""json""#),
        "a flag's value: {attributes}"
    );

    // Without an endpoint, nothing is sent and the command is unaffected.
    let quiet = colab()
        .current_dir(&root)
        .env_remove("OTEL_SDK_DISABLED")
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .env_remove("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")
        .args([
            "find",
            "-e",
            r#"match go::import "old.module" { replace "new.module" }"#,
            "main.go",
        ])
        .output()
        .unwrap();
    assert!(quiet.status.success());
    assert!(
        traces
            .recv_timeout(std::time::Duration::from_millis(300))
            .is_err()
    );
    fs::remove_dir_all(&root).ok();
}
