//! `colab agent install | uninstall`: put colab in front of an agent (M20).
//!
//! For each tool asked for (or found on the machine), install writes only
//! what colab owns, and uninstall removes only that:
//!
//! | Tool | What | Where |
//! | ---- | ---- | ----- |
//! | Claude Code | MCP server `colab` | `.mcp.json`, key `mcpServers.colab` |
//! | Claude Code | post-edit hook | `.claude/settings.json`, the `PostToolUse` hook running `colab hook post-edit` |
//! | Claude Code | the skill | `.claude/skills/colab-codemod/` |
//! | Cursor | MCP server `colab` | `.cursor/mcp.json`, key `mcpServers.colab` |
//! | Cursor | the skill, as a rule | `.cursor/rules/colab.mdc` |
//! | Codex | MCP server `colab` | `~/.codex/config.toml`, table `[mcp_servers.colab]` |
//! | Codex | how to use colab | `AGENTS.md`, the managed block `colab` |
//!
//! With `--user`, the same for every project of the user's instead of
//! one: Claude Code's MCP server through `claude mcp add --scope user`
//! (Claude Code owns `~/.claude.json`, so colab only reads it), the skill
//! in `~/.claude/skills/`, no hook; Cursor's MCP server in
//! `~/.cursor/mcp.json`; Codex's in `~/.codex/config.toml`, with the
//! block in `~/.codex/AGENTS.md`.
//!
//! Install compares by meaning, not bytes, so a second run changes
//! nothing and says so. Uninstall removes those keys, entries, blocks and
//! files, keeps everything else in a file as it was, and deletes a file
//! that is left with nothing else in it. A file colab would edit that does
//! not parse (a broken `settings.json`) is an error, never overwritten.
use std::fmt::Write as _;

use std::io::Write;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand, ValueEnum};
use colab_core::{Error, Result};
use serde_json::{Value, json};

#[derive(Parser, Debug)]
pub struct AgentArgs {
    #[command(subcommand)]
    action: AgentAction,
}

#[derive(Subcommand, Debug)]
enum AgentAction {
    /// Wire colab into the agents asked for, or the ones found: its MCP
    /// server, its skill, and (Claude Code) the post-edit hook.
    Install(Targets),
    /// Remove what `install` wrote, and nothing else.
    Uninstall(Targets),
}

#[derive(clap::Args, Debug)]
struct Targets {
    /// Claude Code (`.mcp.json`, `.claude/settings.json`, `.claude/skills/`).
    #[arg(long)]
    claude: bool,
    /// Cursor (`.cursor/mcp.json`, `.cursor/rules/colab.mdc`).
    #[arg(long)]
    cursor: bool,
    /// Codex (`~/.codex/config.toml`, `AGENTS.md`).
    #[arg(long)]
    codex: bool,
    /// Without the Claude Code post-edit hook.
    #[arg(long = "no-hook")]
    no_hook: bool,
    /// The project to install into (default: the working directory).
    #[arg(short = 'C', long = "change-dir", default_value = ".")]
    project: PathBuf,
    /// For every project of the user's, not one: user-level MCP servers,
    /// the skill in `~/.claude/skills/`, and no post-edit hook. Rerun it
    /// after upgrading colab to refresh the skill.
    #[arg(long, conflicts_with = "project")]
    user: bool,
    /// Show what would change, as diffs, and write nothing.
    #[arg(long = "dry-run")]
    dry_run: bool,
    /// `human`: a line per file. `json`: one document.
    #[arg(long, value_enum, default_value_t = AgentFormat::Human)]
    format: AgentFormat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum AgentFormat {
    Human,
    Json,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tool {
    Claude,
    Cursor,
    Codex,
}

impl Tool {
    fn name(self) -> &'static str {
        match self {
            Tool::Claude => "claude",
            Tool::Cursor => "cursor",
            Tool::Codex => "codex",
        }
    }
}

/// One file's change: its text before and after (`None`: absent).
#[derive(Debug, Clone, PartialEq, Eq)]
struct FileEdit {
    tool: Tool,
    path: PathBuf,
    before: Option<String>,
    after: Option<String>,
}

const SKILL: &[(&str, &str)] = &[
    (
        "SKILL.md",
        include_str!("../../../plugins/claude/colab/skills/colab-codemod/SKILL.md"),
    ),
    (
        "references/recipes.md",
        include_str!("../../../plugins/claude/colab/skills/colab-codemod/references/recipes.md"),
    ),
    (
        "references/troubleshooting.md",
        include_str!(
            "../../../plugins/claude/colab/skills/colab-codemod/references/troubleshooting.md"
        ),
    ),
];

/// A change made by running the agent's own CLI, for files colab must
/// not rewrite itself (`~/.claude.json`, which Claude Code keeps
/// rewriting).
#[derive(Debug, Clone, PartialEq, Eq)]
struct CommandStep {
    tool: Tool,
    /// What it changes, for the report.
    what: &'static str,
    /// The command; `None` when nothing needs doing.
    argv: Option<Vec<&'static str>>,
    status: &'static str,
}

const CLAUDE_MCP_ADD: &[&str] = &[
    "claude", "mcp", "add", "--scope", "user", "colab", "--", "colab", "mcp",
];
const CLAUDE_MCP_REMOVE: &[&str] = &["claude", "mcp", "remove", "--scope", "user", "colab"];

/// Claude Code's user config: `$CLAUDE_CONFIG_DIR/.claude.json`, else
/// `~/.claude.json`.
fn claude_user_config(home: &Path) -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map_or_else(|| home.to_path_buf(), PathBuf::from)
        .join(".claude.json")
}

/// Claude Code's user directory (skills): `$CLAUDE_CONFIG_DIR`, else
/// `~/.claude`.
fn claude_user_dir(home: &Path) -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR").map_or_else(|| home.join(".claude"), PathBuf::from)
}

/// The user-scope MCP server, added or removed with `claude mcp`, judged
/// by what `~/.claude.json` already holds.
fn claude_user_mcp(config: &Path, install: bool) -> Result<CommandStep> {
    let text = read(config)?;
    let doc = parse_json(config, text.as_deref())?;
    let entry = &doc["mcpServers"]["colab"];
    let ours = entry["command"] == "colab" && entry["args"] == json!(["mcp"]);
    let (argv, status) = match (install, entry.is_null(), ours) {
        (true, _, true) | (false, true, _) => (None, "unchanged"),
        // Someone else's `colab` entry: replace it, as `.mcp.json` does.
        (true, false, false) => (
            Some([CLAUDE_MCP_REMOVE, &["&&"], CLAUDE_MCP_ADD].concat()),
            "updated",
        ),
        (true, true, _) => (Some(CLAUDE_MCP_ADD.to_vec()), "created"),
        (false, false, _) => (Some(CLAUDE_MCP_REMOVE.to_vec()), "removed"),
    };
    Ok(CommandStep {
        tool: Tool::Claude,
        what: "user MCP server `colab`",
        argv,
        status,
    })
}

/// Run a step's command(s), `&&`-separated; a failure names the command.
fn run_step(step: &CommandStep) -> Result<()> {
    let Some(argv) = &step.argv else {
        return Ok(());
    };
    for cmd in argv.split(|a| *a == "&&") {
        let shown = cmd.join(" ");
        let out = std::process::Command::new(cmd[0])
            .args(&cmd[1..])
            .output()
            .map_err(|e| {
                Error::Config(format!(
                    "could not run `{shown}` ({e}); is `{}` on PATH?",
                    cmd[0]
                ))
            })?;
        if !out.status.success() {
            return Err(Error::Config(format!(
                "`{shown}` failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
    }
    Ok(())
}

/// The post-edit hook's command; uninstall finds the hook by it.
const HOOK_COMMAND: &str = "colab hook post-edit 2>/dev/null || true";
const HOOK_MARK: &str = "colab hook post-edit";

const AGENTS_BLOCK: &str = "## colab

For a mechanical change at many sites (an import path, a renamed symbol,
a call shape, a config key), use colab rather than editing each file:
`colab find -e '<rule>' .` to see where it applies, `colab refactor -e
'<rule>' --format diff .` to preview, `--write` to apply. Its MCP server
(`colab mcp`) offers the same as `colab.preview` / `colab.apply`. A
repository's own conventions are in `colab.toml`: `colab fix --check`.";

fn mcp_entry() -> Value {
    json!({"command": "colab", "args": ["mcp"]})
}

fn hook_entry() -> Value {
    json!({
        "matcher": "Edit|Write|MultiEdit",
        "hooks": [{"type": "command", "command": HOOK_COMMAND, "timeout": 30}]
    })
}

fn read(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::io_at(path, e)),
    }
}

fn parse_json(path: &Path, text: Option<&str>) -> Result<Value> {
    match text {
        None => Ok(json!({})),
        Some(t) if t.trim().is_empty() => Ok(json!({})),
        Some(t) => {
            let value: Value = serde_json::from_str(t).map_err(|e| {
                Error::Config(format!(
                    "{}: not valid JSON ({e}); colab leaves it alone, fix it and rerun",
                    path.display()
                ))
            })?;
            if !value.is_object() {
                return Err(Error::Config(format!(
                    "{}: not a JSON object; colab leaves it alone",
                    path.display()
                )));
            }
            Ok(value)
        }
    }
}

/// `value` as the file's new text: `None` when it is left with nothing.
fn json_text(before: Option<&str>, old: &Value, new: &Value) -> Option<String> {
    if new == old {
        return before.map(str::to_string);
    }
    if new.as_object().is_some_and(serde_json::Map::is_empty) {
        return None;
    }
    Some(serde_json::to_string_pretty(new).unwrap_or_default() + "\n")
}

/// `.mcp.json` / `.cursor/mcp.json`: `mcpServers.colab` set or removed.
fn mcp_json(tool: Tool, path: PathBuf, install: bool) -> Result<FileEdit> {
    let before = read(&path)?;
    let old = parse_json(&path, before.as_deref())?;
    let mut new = old.clone();
    let root = new.as_object_mut().expect("parse_json gives an object");
    if install {
        let servers = root.entry("mcpServers").or_insert_with(|| json!({}));
        let Some(servers) = servers.as_object_mut() else {
            return Err(Error::Config(format!(
                "{}: `mcpServers` is not an object; colab leaves it alone",
                path.display()
            )));
        };
        servers.insert("colab".into(), mcp_entry());
    } else if let Some(servers) = root.get_mut("mcpServers").and_then(Value::as_object_mut) {
        servers.remove("colab");
        if servers.is_empty() {
            root.remove("mcpServers");
        }
    }
    let after = json_text(before.as_deref(), &old, &new);
    Ok(FileEdit {
        tool,
        path,
        before,
        after,
    })
}

/// `.claude/settings.json`: the post-edit hook added once, or removed.
fn claude_settings(path: PathBuf, install: bool) -> Result<FileEdit> {
    let before = read(&path)?;
    let old = parse_json(&path, before.as_deref())?;
    let mut new = old.clone();
    let ours = |hook: &Value| {
        hook["command"]
            .as_str()
            .is_some_and(|c| c.contains(HOOK_MARK))
    };
    let root = new.as_object_mut().expect("parse_json gives an object");
    if install {
        let hooks = root.entry("hooks").or_insert_with(|| json!({}));
        let post = hooks
            .as_object_mut()
            .ok_or_else(|| Error::Config(format!("{}: `hooks` is not an object", path.display())))?
            .entry("PostToolUse")
            .or_insert_with(|| json!([]));
        let list = post.as_array_mut().ok_or_else(|| {
            Error::Config(format!(
                "{}: `hooks.PostToolUse` is not a list",
                path.display()
            ))
        })?;
        let present = list.iter().any(|entry| {
            entry["hooks"]
                .as_array()
                .is_some_and(|hs| hs.iter().any(ours))
        });
        if !present {
            list.push(hook_entry());
        }
    } else if let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) {
        if let Some(list) = hooks.get_mut("PostToolUse").and_then(Value::as_array_mut) {
            for entry in list.iter_mut() {
                if let Some(hs) = entry.get_mut("hooks").and_then(Value::as_array_mut) {
                    hs.retain(|h| !ours(h));
                }
            }
            list.retain(|entry| entry["hooks"].as_array().is_none_or(|hs| !hs.is_empty()));
            if list.is_empty() {
                hooks.remove("PostToolUse");
            }
        }
        if hooks.is_empty() {
            root.remove("hooks");
        }
    }
    let after = json_text(before.as_deref(), &old, &new);
    Ok(FileEdit {
        tool: Tool::Claude,
        path,
        before,
        after,
    })
}

/// A file colab owns outright: written whole, or removed.
fn owned(tool: Tool, path: PathBuf, content: &str, install: bool) -> Result<FileEdit> {
    let before = read(&path)?;
    let after = install.then(|| content.to_string());
    Ok(FileEdit {
        tool,
        path,
        before,
        after,
    })
}

/// `~/.codex/config.toml`: `[mcp_servers.colab]` set or removed.
fn codex_config(path: PathBuf, install: bool) -> Result<FileEdit> {
    let before = read(&path)?;
    let mut doc: toml_edit::DocumentMut = before.as_deref().unwrap_or("").parse().map_err(|e| {
        Error::Config(format!(
            "{}: not valid TOML ({e}); colab leaves it alone",
            path.display()
        ))
    })?;
    let has = doc
        .get("mcp_servers")
        .and_then(|s| s.get("colab"))
        .and_then(|c| c.get("command"))
        .and_then(|c| c.as_str())
        == Some("colab");
    let after = if install {
        if has {
            before.clone()
        } else {
            if doc.get("mcp_servers").is_none() {
                let mut servers = toml_edit::Table::new();
                servers.set_implicit(true);
                doc.insert("mcp_servers", toml_edit::Item::Table(servers));
            }
            let mut colab = toml_edit::Table::new();
            colab.insert("command", toml_edit::value("colab"));
            let mut args = toml_edit::Array::new();
            args.push("mcp");
            colab.insert("args", toml_edit::value(args));
            doc["mcp_servers"]["colab"] = toml_edit::Item::Table(colab);
            Some(doc.to_string())
        }
    } else {
        let removed = doc
            .get_mut("mcp_servers")
            .and_then(|s| s.as_table_like_mut())
            .and_then(|t| t.remove("colab"))
            .is_some();
        if removed {
            if doc
                .get("mcp_servers")
                .and_then(|s| s.as_table_like())
                .is_some_and(toml_edit::TableLike::is_empty)
            {
                doc.remove("mcp_servers");
            }
            let text = doc.to_string();
            (!text.trim().is_empty()).then_some(text)
        } else {
            before.clone()
        }
    };
    Ok(FileEdit {
        tool: Tool::Codex,
        path,
        before,
        after,
    })
}

/// `AGENTS.md`: the managed block `colab`, through colab's own
/// `markdown::block` rule.
fn agents_md(path: PathBuf, install: bool) -> Result<FileEdit> {
    let before = read(&path)?;
    let rule = if install {
        format!("match markdown::block \"colab\" {{ set r##\"{AGENTS_BLOCK}\"## }}")
    } else {
        "match markdown::block \"colab\" { delete }".to_string()
    };
    let script = format!("refactor \"agent\" {{ {rule} }}");
    let refactoring = colab_dsl::compile(&script, &colab_backends::registry())?;
    let text = before.clone().unwrap_or_default();
    let out =
        colab_core::CodeTransformer::apply_at(&refactoring, Path::new("AGENTS.md"), &text).output;
    let after = if out == text {
        before.clone()
    } else if out.trim().is_empty() {
        None
    } else {
        Some(out)
    };
    Ok(FileEdit {
        tool: Tool::Codex,
        path,
        before,
        after,
    })
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| Error::Config("HOME is not set, so Codex's config cannot be found".into()))
}

/// The tools asked for, else those found: on `PATH`, or with their
/// directory in the project or the home directory.
fn tools(t: &Targets, project: &Path) -> Result<Vec<Tool>> {
    // In user scope `project` is the home directory, so its `.claude`
    // and `.cursor` are the user's.
    let asked: Vec<Tool> = [
        (t.claude, Tool::Claude),
        (t.cursor, Tool::Cursor),
        (t.codex, Tool::Codex),
    ]
    .into_iter()
    .filter_map(|(on, tool)| on.then_some(tool))
    .collect();
    if !asked.is_empty() {
        return Ok(asked);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let found: Vec<Tool> = [
        (
            on_path("claude") || project.join(".claude").is_dir(),
            Tool::Claude,
        ),
        (
            on_path("cursor") || project.join(".cursor").is_dir() || home.join(".cursor").is_dir(),
            Tool::Cursor,
        ),
        (
            on_path("codex") || home.join(".codex").is_dir(),
            Tool::Codex,
        ),
    ]
    .into_iter()
    .filter_map(|(on, tool)| on.then_some(tool))
    .collect();
    if found.is_empty() {
        return Err(Error::Config(
            "no agent found (claude, cursor or codex on PATH, or their directories); name one with --claude, --cursor or --codex".into(),
        ));
    }
    Ok(found)
}

/// The files to change and the commands to run, in user scope (`--user`)
/// with `base` the home directory, else with `base` the project.
fn plan(
    tools: &[Tool],
    t: &Targets,
    base: &Path,
    install: bool,
) -> Result<(Vec<FileEdit>, Vec<CommandStep>)> {
    if t.user {
        return plan_user(tools, base, install);
    }
    let project = base;
    let mut edits = Vec::new();
    for &tool in tools {
        match tool {
            Tool::Claude => {
                edits.push(mcp_json(tool, project.join(".mcp.json"), install)?);
                if !t.no_hook || !install {
                    edits.push(claude_settings(
                        project.join(".claude/settings.json"),
                        install,
                    )?);
                }
                for (rel, content) in SKILL {
                    let path = project.join(".claude/skills/colab-codemod").join(rel);
                    edits.push(owned(tool, path, content, install)?);
                }
            }
            Tool::Cursor => {
                edits.push(mcp_json(tool, project.join(".cursor/mcp.json"), install)?);
                let body = SKILL[0]
                    .1
                    .splitn(3, "---")
                    .nth(2)
                    .unwrap_or(SKILL[0].1)
                    .trim_start();
                let rule = format!(
                    "---\ndescription: Repo-wide, mechanical code and config changes with colab (codemods); use instead of editing many files by hand\nalwaysApply: false\n---\n\n{body}"
                );
                edits.push(owned(
                    tool,
                    project.join(".cursor/rules/colab.mdc"),
                    &rule,
                    install,
                )?);
            }
            Tool::Codex => {
                edits.push(codex_config(home()?.join(".codex/config.toml"), install)?);
                edits.push(agents_md(project.join("AGENTS.md"), install)?);
            }
        }
    }
    Ok((edits, Vec::new()))
}

fn plan_user(
    tools: &[Tool],
    home: &Path,
    install: bool,
) -> Result<(Vec<FileEdit>, Vec<CommandStep>)> {
    let mut edits = Vec::new();
    let mut commands = Vec::new();
    for &tool in tools {
        match tool {
            Tool::Claude => {
                commands.push(claude_user_mcp(&claude_user_config(home), install)?);
                for (rel, content) in SKILL {
                    let path = claude_user_dir(home).join("skills/colab-codemod").join(rel);
                    edits.push(owned(tool, path, content, install)?);
                }
            }
            // Cursor keeps user rules in its settings, not in files, so
            // only the MCP server is user-wide.
            Tool::Cursor => edits.push(mcp_json(tool, home.join(".cursor/mcp.json"), install)?),
            Tool::Codex => {
                edits.push(codex_config(home.join(".codex/config.toml"), install)?);
                edits.push(agents_md(home.join(".codex/AGENTS.md"), install)?);
            }
        }
    }
    Ok((edits, commands))
}

/// Write each changed file, or remove it and the directories above it
/// that are left empty, up to `stop` (the project and home directories).
fn apply_within(edits: &[FileEdit], stop: &[PathBuf]) -> Result<()> {
    for e in edits {
        if e.before == e.after {
            continue;
        }
        match &e.after {
            Some(text) => {
                if let Some(parent) = e.path.parent() {
                    std::fs::create_dir_all(parent).map_err(|err| Error::io_at(parent, err))?;
                }
                std::fs::write(&e.path, text).map_err(|err| Error::io_at(&e.path, err))?;
            }
            None => {
                std::fs::remove_file(&e.path).map_err(|err| Error::io_at(&e.path, err))?;
                // `remove_dir` only removes an empty directory.
                let mut dir = e.path.parent();
                while let Some(d) = dir {
                    if stop.iter().any(|s| s == d) || std::fs::remove_dir(d).is_err() {
                        break;
                    }
                    dir = d.parent();
                }
            }
        }
    }
    Ok(())
}

fn status(e: &FileEdit) -> &'static str {
    match (&e.before, &e.after) {
        (b, a) if b == a => "unchanged",
        (None, Some(_)) => "created",
        (Some(_), None) => "removed",
        _ => "updated",
    }
}

pub fn run(args: AgentArgs) -> Result<u8> {
    let (t, install) = match &args.action {
        AgentAction::Install(t) => (t, true),
        AgentAction::Uninstall(t) => (t, false),
    };
    let dir = if t.user { home()? } else { t.project.clone() };
    let project = dir.canonicalize().map_err(|e| Error::io_at(&dir, e))?;
    let tools = tools(t, &project)?;
    let (edits, commands) = plan(&tools, t, &project, install)?;
    if !t.dry_run {
        let mut stop = vec![project.clone()];
        stop.extend(home().ok().and_then(|h| h.canonicalize().ok()));
        if let Some(claude) = std::env::var_os("CLAUDE_CONFIG_DIR") {
            stop.push(PathBuf::from(claude));
        }
        for step in &commands {
            run_step(step)?;
        }
        apply_within(&edits, &stop)?;
    }
    // Paths under the project are shown relative to it; in user scope,
    // as `~/…`.
    let shown = |p: &Path| match p.strip_prefix(&project) {
        Ok(rel) if t.user => Path::new("~").join(rel),
        Ok(rel) => rel.to_path_buf(),
        Err(_) => p.to_path_buf(),
    };
    let mut out = std::io::stdout().lock();
    match t.format {
        AgentFormat::Json => {
            let files: Vec<Value> = edits
                .iter()
                .map(|e| json!({"tool": e.tool.name(), "path": shown(&e.path), "status": status(e)}))
                .collect();
            let mut doc = json!({"action": if install { "install" } else { "uninstall" }, "dry_run": t.dry_run, "files": files});
            if t.user {
                doc["scope"] = json!("user");
                doc["commands"] = commands
                    .iter()
                    .map(|c| json!({"tool": c.tool.name(), "what": c.what, "status": c.status, "command": c.argv.as_ref().map(|a| a.join(" "))}))
                    .collect();
            }
            writeln!(out, "{doc}")
        }
        AgentFormat::Human => {
            let mut text = String::new();
            for c in &commands {
                let _ = write!(text, "{:<7} {:<10} {}", c.tool.name(), c.status, c.what);
                match &c.argv {
                    Some(argv) => {
                        let _ = writeln!(text, " (`{}`)", argv.join(" "));
                    }
                    None => text.push('\n'),
                }
            }
            for e in &edits {
                let _ = writeln!(text, "{:<7} {:<10} {}", e.tool.name(), status(e), shown(&e.path).display());
                if t.dry_run && e.before != e.after {
                    let diff = colab_core::render::unified_diff(
                        &shown(&e.path),
                        e.before.as_deref().unwrap_or(""),
                        e.after.as_deref().unwrap_or(""),
                    );
                    text.push_str(&diff);
                }
            }
            if install && !t.user && tools.contains(&Tool::Claude) {
                text.push_str("For every project instead: /plugin marketplace add grahambrooks/colab, then /plugin install colab@colab\n");
            }
            write!(out, "{text}")
        }
    }
    .map_err(crate::cli::io_to_error)?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(edits: &[FileEdit]) -> Result<()> {
        apply_within(edits, &[])
    }

    fn temp(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "colab-agent-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn json_entries_are_added_once_and_removed_alone() {
        let dir = temp("json");
        let settings = dir.join("settings.json");
        let theirs = "{\n  \"hooks\": {\n    \"PostToolUse\": [\n      {\"matcher\": \"Bash\", \"hooks\": [{\"type\": \"command\", \"command\": \"mine\"}]}\n    ]\n  },\n  \"model\": \"x\"\n}\n";
        std::fs::write(&settings, theirs).unwrap();
        let first = claude_settings(settings.clone(), true).unwrap();
        apply(std::slice::from_ref(&first)).unwrap();
        let again = claude_settings(settings.clone(), true).unwrap();
        assert_eq!(status(&again), "unchanged");
        let installed: Value =
            serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(
            installed["hooks"]["PostToolUse"].as_array().unwrap().len(),
            2
        );
        let removed = claude_settings(settings.clone(), false).unwrap();
        apply(std::slice::from_ref(&removed)).unwrap();
        let back: Value =
            serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(
            back,
            serde_json::from_str::<Value>(theirs).unwrap(),
            "their hook and keys stay"
        );
        // A file colab created goes again with uninstall.
        let mcp = dir.join(".mcp.json");
        apply(&[mcp_json(Tool::Claude, mcp.clone(), true).unwrap()]).unwrap();
        assert!(mcp.exists());
        apply(&[mcp_json(Tool::Claude, mcp.clone(), false).unwrap()]).unwrap();
        assert!(!mcp.exists());
        // A broken file is not overwritten.
        std::fs::write(&settings, "{ nope").unwrap();
        assert!(
            claude_settings(settings, true)
                .unwrap_err()
                .to_string()
                .contains("not valid JSON")
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn codex_config_and_agents_md_round_trip() {
        let dir = temp("codex");
        let config = dir.join("config.toml");
        let theirs = "model = \"o4\"\n\n[mcp_servers.other]\ncommand = \"x\"\n";
        std::fs::write(&config, theirs).unwrap();
        apply(&[codex_config(config.clone(), true).unwrap()]).unwrap();
        assert!(
            std::fs::read_to_string(&config)
                .unwrap()
                .contains("[mcp_servers.colab]")
        );
        assert_eq!(
            status(&codex_config(config.clone(), true).unwrap()),
            "unchanged"
        );
        apply(&[codex_config(config.clone(), false).unwrap()]).unwrap();
        assert_eq!(std::fs::read_to_string(&config).unwrap(), theirs);
        let agents = dir.join("AGENTS.md");
        let notes = "# Notes\n\nOurs.\n";
        std::fs::write(&agents, notes).unwrap();
        apply(&[agents_md(agents.clone(), true).unwrap()]).unwrap();
        assert!(
            std::fs::read_to_string(&agents)
                .unwrap()
                .contains("colab mcp")
        );
        assert_eq!(
            status(&agents_md(agents.clone(), true).unwrap()),
            "unchanged"
        );
        apply(&[agents_md(agents.clone(), false).unwrap()]).unwrap();
        assert_eq!(
            std::fs::read_to_string(&agents).unwrap().trim_end(),
            notes.trim_end()
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
