//! The MCP tools backed by a language server (`colab.references`,
//! `colab.calls`, `colab.implementations`, `colab.rename`,
//! `colab.rename_symbol`, `colab.action`, `colab.mv`, `colab.edit_symbol`),
//! which `colab mcp` registers through `colab_mcp::ToolProvider`.

use super::*;
use colab_lsp_client::actions::Select;

/// The language-server tools, registered into `colab mcp` by the binary
/// (`colab-mcp` cannot depend on `colab-lsp-client`).
pub struct LspTools;

pub(crate) fn position_properties() -> Value {
    json!({
        "at": {"type": "string", "description": "`FILE:LINE:COL` (1-based, columns in characters), relative to `cwd`."},
        "symbol": {"type": "string", "description": "Instead of `at`: a name; its first whole-word occurrence in `in` is used."},
        "in": {"type": "string", "description": "The file `symbol` is looked up in — point it at the declaring file."},
        "lsp": {"type": "string", "description": "Language-server command; defaults by file extension (rust-analyzer, gopls, clangd, …)."},
        "root": {"type": "string", "description": "Project root for the server; defaults to the nearest project marker."},
        "cwd": {"type": "string", "description": "Directory paths are relative to; defaults to the server's."},
        "timeout": {"type": "integer", "description": "Seconds for start-up, indexing, and the request (default 120)."}
    })
}

pub(crate) fn write_properties(props: &mut Value) {
    props["write"] = json!({"type": "boolean", "default": false, "description": "Apply the change. Default: preview only. File operations (moves) need a git work tree with clean touched paths."});
    props["tidy"] = json!({"type": "boolean", "default": false, "description": "Organize imports and format changed files via the server, reported as its own rule."});
    props["detail"] =
        json!({"type": "string", "enum": ["summary", "counts", "diff"], "default": "counts"});
}

impl colab_mcp::ToolProvider for LspTools {
    fn tools(&self) -> Vec<Value> {
        let mut references = position_properties();
        references["include_declaration"] = json!({"type": "boolean", "default": false});
        references["max_results"] =
            json!({"type": "integer", "default": colab_core::find::DEFAULT_MAX_RESULTS});

        let mut calls = position_properties();
        calls["outgoing"] = json!({"type": "boolean", "default": false, "description": "List what the function calls, instead of who calls it."});
        calls["max_results"] =
            json!({"type": "integer", "default": colab_core::find::DEFAULT_MAX_RESULTS});

        let mut implementations = position_properties();
        implementations["max_results"] =
            json!({"type": "integer", "default": colab_core::find::DEFAULT_MAX_RESULTS});

        let mut rename = position_properties();
        rename["to"] = json!({"type": "string", "description": "The new name."});
        write_properties(&mut rename);

        let mut rename_symbol = rename.clone();
        rename_symbol["include_text"] = json!({"type": "boolean", "default": false, "description": "Also change the old name where the server does not: whole-word matches in comments and string literals of source files, and in config and docs files (TOML, JSON, YAML, Markdown, text) under the root. Without it they are only listed, under `text`."});

        let mut action = position_properties();
        action["at"] = json!({"type": "string", "description": "`FILE:LINE:COL` or a selection `FILE:LINE:COL-LINE:COL`, relative to `cwd`. Extract-style actions need a selection."});
        action
            .as_object_mut()
            .expect("position_properties builds an object")
            .remove("symbol");
        action
            .as_object_mut()
            .expect("position_properties builds an object")
            .remove("in");
        action["list"] = json!({"type": "boolean", "default": false, "description": "List the actions offered at `at` instead of running one."});
        action["kind"] = json!({"type": "string", "description": "Dotted-prefix kind, e.g. `refactor.extract` or `refactor.inline`."});
        action["title"] = json!({"type": "string", "description": "Case-insensitive substring of the action's title."});
        write_properties(&mut action);

        let mut mv = json!({
            "from": {"type": "string", "description": "The file or directory to move, relative to `cwd`."},
            "to": {"type": "string", "description": "Where it goes; an existing directory means \"into it\"."},
            "lsp": {"type": "string", "description": "Language-server command that updates what refers to the file; defaults by extension."},
            "no_lsp": {"type": "boolean", "default": false, "description": "Move with git only; update no references."},
            "root": {"type": "string", "description": "Project root for the server; defaults to the nearest project marker."},
            "cwd": {"type": "string", "description": "Directory paths are relative to; defaults to the server's."},
            "timeout": {"type": "integer", "description": "Seconds for start-up, indexing, and the answer (default 120)."}
        });
        write_properties(&mut mv);

        let mut edit = json!({
            "symbol": {"type": "string", "description": "The declaration's name path: `load`, or `Config/load` (`/` between a container and what it holds)."},
            "in": {"type": "string", "description": "The file it is declared in, relative to `cwd`."},
            "edit": {"type": "string", "enum": ["replace_body", "insert_before", "insert_after"], "description": "Replace the whole declaration (doc comments and attributes too), or insert beside it."},
            "text": {"type": "string", "description": "The new text; it is re-indented to the declaration's indentation."},
            "lsp": {"type": "string", "description": "Find the declaration with this language server's documentSymbol instead of colab's parser."},
            "list": {"type": "boolean", "default": false, "description": "List the file's declarations (name paths and lines) instead of editing."},
            "cwd": {"type": "string", "description": "Directory paths are relative to; defaults to the server's."},
            "timeout": {"type": "integer", "description": "Seconds for a language server's start-up (default 120)."}
        });
        write_properties(&mut edit);

        vec![
            json!({
                "name": "colab.edit_symbol",
                "description": "Edit one declaration by name path instead of by exact text: replace a function, method or class whole, or insert code before or after it. Cheaper and safer than a string match: the declaration is found by colab's parser (or the language server with `lsp`), an ambiguous name lists the candidates, and the result is a colab report (diff, conflicts). Call with `list: true` to see a file's name paths. Previews unless `write` is true.",
                "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false},
                "inputSchema": {"type": "object", "properties": edit, "required": ["in"], "additionalProperties": false}
            }),
            json!({
                "name": "colab.mv",
                "description": "Move or rename a file the way an IDE does: the language server updates what refers to it (`mod` declarations, imports, paths — workspace/willRenameFiles) and the move itself is made with `git mv`, so history follows. Never a plain filesystem move. Previews unless `write` is true; writing needs a git work tree with the touched paths clean.",
                "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false},
                "inputSchema": {"type": "object", "properties": mv, "required": ["from", "to"], "additionalProperties": false}
            }),
            json!({
                "name": "colab.references",
                "description": "Find every reference to the symbol at a position, as resolved by the language's own server (scope-aware, unlike text search). Read-only. Use before changing or deleting a symbol to see its reach.",
                "annotations": {"readOnlyHint": true},
                "inputSchema": {"type": "object", "properties": references, "additionalProperties": false}
            }),
            json!({
                "name": "colab.calls",
                "description": "Who calls a function (or, with `outgoing`, what it calls), from the language server's call hierarchy: each caller's or callee's declaration, as `{path, line, col, text}`. Read-only. Use to see a function's reach across files before changing its signature, or to find the code paths into it.",
                "annotations": {"readOnlyHint": true},
                "inputSchema": {"type": "object", "properties": calls, "additionalProperties": false}
            }),
            json!({
                "name": "colab.implementations",
                "description": "What implements an interface, trait or abstract method, as the language server resolves it (`textDocument/implementation`): each implementation's declaration as `{path, line, col, text}`. Read-only. Use before changing an interface to see every type that must follow.",
                "annotations": {"readOnlyHint": true},
                "inputSchema": {"type": "object", "properties": implementations, "additionalProperties": false}
            }),
            json!({
                "name": "colab.rename",
                "description": "Scope-aware rename via the language's server (rust-analyzer, gopls, clangd, …): renames the symbol at a position and every reference the server resolves to it — not other symbols that share the name. Returns the same report as colab.preview; set `write` to apply.",
                "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false},
                "inputSchema": {"type": "object", "properties": rename, "required": ["to"], "additionalProperties": false}
            }),
            json!({
                "name": "colab.rename_symbol",
                "description": "Rename a symbol the way an IDE does: the language server renames the symbol and every reference it resolves, and colab also finds the old name in comments, strings, and config/docs files, which the server skips. Those text matches are listed under `text` and only changed with `include_text: true`. Give `symbol` + `in` (the declaring file) or `at`. Previews unless `write` is true.",
                "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false},
                "inputSchema": {"type": "object", "properties": rename_symbol, "required": ["to"], "additionalProperties": false}
            }),
            json!({
                "name": "colab.action",
                "description": "Run one of the language server's code actions — extract function/variable, inline, change signature, organize imports, … — at a position or selection. Call with `list: true` first to see what is offered, then pick with `kind` and/or `title`. Returns a colab report; set `write` to apply.",
                "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false},
                "inputSchema": {"type": "object", "properties": action, "required": ["at"], "additionalProperties": false}
            }),
        ]
    }

    fn call(&self, name: &str, args: &Value) -> Option<Result<Value>> {
        let run = match name {
            "colab.references" => mcp_references,
            "colab.calls" => mcp_calls,
            "colab.implementations" => mcp_implementations,
            "colab.rename" => mcp_rename,
            "colab.rename_symbol" => mcp_rename_symbol,
            "colab.action" => mcp_action,
            "colab.mv" => mcp_mv,
            "colab.edit_symbol" => mcp_edit_symbol,
            _ => return None,
        };
        Some(run(args))
    }
}

/// `colab.mv`: `colab mv` over MCP.
pub(crate) fn mcp_mv(args: &Value) -> Result<Value> {
    let cwd = match arg(args, "cwd") {
        Some(dir) => fs::canonicalize(dir).map_err(|e| Error::io_at(Path::new(dir), e))?,
        None => std::env::current_dir().map_err(io_to_error)?,
    };
    let from = arg(args, "from").ok_or_else(|| Error::Config("`from` is required".into()))?;
    let to = arg(args, "to").ok_or_else(|| Error::Config("`to` is required".into()))?;
    let timeout = args.get("timeout").and_then(Value::as_u64).unwrap_or(120);
    let plan = moves::plan_move(&moves::MoveSpec {
        cwd: &cwd,
        from: Path::new(from),
        to: Path::new(to),
        lsp: arg(args, "lsp"),
        no_lsp: flag(args, "no_lsp"),
        root: arg(args, "root").map(Path::new),
        timeout,
    })?;
    let tidy = plan.server.as_ref().map(|server| tidy::Tidy {
        server,
        root: &plan.root,
        timeout,
    });
    change_set_json(
        vec![plan.label],
        plan.changes,
        plan.warnings,
        args,
        &cwd,
        tidy,
    )
}

pub(crate) fn mcp_edit_symbol(args: &Value) -> Result<Value> {
    use colab_core::symbols::SymbolEdit;
    let cwd = match arg(args, "cwd") {
        Some(dir) => fs::canonicalize(dir).map_err(|e| Error::io_at(Path::new(dir), e))?,
        None => std::env::current_dir().map_err(io_to_error)?,
    };
    let file = cwd.join(arg(args, "in").ok_or_else(|| Error::Config("`in` is required".into()))?);
    let timeout = args.get("timeout").and_then(Value::as_u64).unwrap_or(120);
    let backends = colab_backends::registry();
    if flag(args, "list") {
        let source = fs::read_to_string(&file).map_err(|e| Error::io_at(&file, e))?;
        let (decls, by) = crate::symbol_edit::declarations_of(
            &backends,
            &file,
            &source,
            arg(args, "lsp"),
            timeout,
        )?;
        let rows: Vec<Value> = colab_core::symbols::flatten(&decls)
            .iter()
            .map(|f| {
                let line = source[..f.declaration.range.start].matches('\n').count() + 1;
                json!({"path": f.path.join("/"), "kind": f.declaration.kind, "line": line})
            })
            .collect();
        return Ok(json!({"declarations": rows, "by": by}));
    }
    let symbol = arg(args, "symbol")
        .ok_or_else(|| Error::Config("`symbol` is required (or `list: true`)".into()))?;
    let text = arg(args, "text").ok_or_else(|| Error::Config("`text` is required".into()))?;
    let edit = match arg(args, "edit") {
        Some("replace_body") => SymbolEdit::ReplaceBody,
        Some("insert_before") => SymbolEdit::InsertBefore,
        Some("insert_after") => SymbolEdit::InsertAfter,
        other => {
            return Err(Error::Config(format!(
                "`edit` is `replace_body`, `insert_before` or `insert_after`, not {other:?}"
            )));
        }
    };
    let (change, label) = crate::symbol_edit::plan(
        &backends,
        &file,
        symbol,
        edit,
        text,
        arg(args, "lsp"),
        timeout,
    )?;
    change_set_json(
        vec![label],
        ChangeSet::from_edits(vec![change]),
        Vec::new(),
        args,
        &cwd,
        None,
    )
}

pub(crate) fn arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

pub(crate) fn flag(args: &Value, key: &str) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// `cwd`, place, server, root, and timeout from tool arguments.
pub(crate) fn mcp_context(args: &Value) -> Result<(PathBuf, Place, ServerCommand, PathBuf, u64)> {
    let cwd = match arg(args, "cwd") {
        Some(dir) => fs::canonicalize(dir).map_err(|e| Error::io_at(Path::new(dir), e))?,
        None => std::env::current_dir().map_err(io_to_error)?,
    };
    let place = resolve_place(
        arg(args, "at"),
        arg(args, "symbol"),
        arg(args, "in").map(Path::new),
        &cwd,
    )?;
    let server = server_for(arg(args, "lsp"), &place.file)?;
    let root = root_for(
        arg(args, "root")
            .map(Path::new)
            .map(|r| cwd.join(r))
            .as_deref(),
        &place.file,
        &cwd,
    )?;
    let timeout = args.get("timeout").and_then(Value::as_u64).unwrap_or(120);
    Ok((cwd, place, server, root, timeout))
}

pub(crate) fn mcp_references(args: &Value) -> Result<Value> {
    let (cwd, place, server, root, timeout) = mcp_context(args)?;
    let found = actions::references(
        &server,
        &target(&place, root, timeout),
        flag(args, "include_declaration"),
    )?;
    let max = args
        .get("max_results")
        .and_then(Value::as_u64)
        .map_or(colab_core::find::DEFAULT_MAX_RESULTS, |n| {
            usize::try_from(n).unwrap_or(usize::MAX)
        });
    Ok(references_json(&found, &cwd, max))
}

pub(crate) fn mcp_max(args: &Value) -> usize {
    args.get("max_results")
        .and_then(Value::as_u64)
        .map_or(colab_core::find::DEFAULT_MAX_RESULTS, |n| {
            usize::try_from(n).unwrap_or(usize::MAX)
        })
}

pub(crate) fn mcp_calls(args: &Value) -> Result<Value> {
    let (cwd, place, server, root, timeout) = mcp_context(args)?;
    let outgoing = flag(args, "outgoing");
    let found = actions::calls(&server, &target(&place, root, timeout), outgoing)?;
    let key = if outgoing { "callees" } else { "callers" };
    Ok(locations_json(key, &found, &cwd, mcp_max(args)))
}

pub(crate) fn mcp_implementations(args: &Value) -> Result<Value> {
    let (cwd, place, server, root, timeout) = mcp_context(args)?;
    let found = actions::implementations(&server, &target(&place, root, timeout))?;
    Ok(locations_json(
        "implementations",
        &found,
        &cwd,
        mcp_max(args),
    ))
}

pub(crate) fn mcp_rename(args: &Value) -> Result<Value> {
    let (cwd, place, server, root, timeout) = mcp_context(args)?;
    let to = arg(args, "to").ok_or_else(|| Error::Config("`to` is required".into()))?;
    let outcome = colab_lsp_client::rename(
        &server,
        &colab_lsp_client::RenameRequest {
            file_operations: crate::git::Repo::discover(&root).is_some(),
            root: root.clone(),
            file: place.file.clone(),
            offset: place.start,
            new_name: to.to_string(),
            timeout: Duration::from_secs(timeout),
            settle: SETTLE,
        },
    )?;
    let label = format!(
        "lsp rename `{}` -> `{to}` via {}",
        outcome.old_name.as_deref().unwrap_or("?"),
        server.program
    );
    let tidy = tidy::Tidy {
        server: &server,
        root: &root,
        timeout,
    };
    change_set_json(
        vec![label],
        outcome.changes,
        Vec::new(),
        args,
        &cwd,
        Some(tidy),
    )
}

/// `colab.rename_symbol`: the server's rename, plus the old name's text
/// occurrences it does not touch — listed, or included on request.
pub(crate) fn mcp_rename_symbol(args: &Value) -> Result<Value> {
    let (cwd, place, server, root, timeout) = mcp_context(args)?;
    let to = arg(args, "to").ok_or_else(|| Error::Config("`to` is required".into()))?;
    let outcome = colab_lsp_client::rename(
        &server,
        &colab_lsp_client::RenameRequest {
            file_operations: crate::git::Repo::discover(&root).is_some(),
            root: root.clone(),
            file: place.file.clone(),
            offset: place.start,
            new_name: to.to_string(),
            timeout: Duration::from_secs(timeout),
            settle: SETTLE,
        },
    )?;
    let old = outcome
        .old_name
        .clone()
        .ok_or_else(|| Error::Config("the server did not say what the old name was".into()))?;
    let mut set = outcome.changes;
    let text = text::text_occurrences(&old, to, &root, &set)?;
    let include = flag(args, "include_text");

    let mut labels = vec![format!(
        "lsp rename `{old}` -> `{to}` via {}",
        server.program
    )];
    let summary = text::text_summary(&text, &cwd, include);
    if include {
        labels.push(format!("text occurrences of `{old}` -> `{to}`"));
        text::merge_text(&mut set, text);
    }
    let tidy = tidy::Tidy {
        server: &server,
        root: &root,
        timeout,
    };
    let mut value = change_set_json(labels, set, Vec::new(), args, &cwd, Some(tidy))?;
    value["text"] = summary;
    Ok(value)
}

pub(crate) fn mcp_action(args: &Value) -> Result<Value> {
    let (cwd, place, server, root, timeout) = mcp_context(args)?;
    let target = target(&place, root.clone(), timeout);
    if flag(args, "list") {
        let list = actions::list_actions(&server, &target, arg(args, "kind"))?;
        return Ok(json!({"actions": list.iter().map(action_json).collect::<Vec<_>>()}));
    }
    let select = Select {
        kind: arg(args, "kind").map(str::to_string),
        title: arg(args, "title").map(str::to_string),
    };
    let outcome = actions::run_action(&server, &target, &select)?;
    let label = format!(
        "lsp action `{}` [{}] via {}",
        outcome.title,
        outcome.kind.as_deref().unwrap_or("command"),
        server.program
    );
    let tidy = tidy::Tidy {
        server: &server,
        root: &root,
        timeout,
    };
    change_set_json(
        vec![label],
        outcome.changes,
        Vec::new(),
        args,
        &cwd,
        Some(tidy),
    )
}

/// The `colab.preview`-shaped report for a change set — the same JSON as
/// `--format json` — applying it first when `write` is set. Nothing is
/// printed: stdout is the MCP transport.
pub(crate) fn change_set_json(
    mut labels: Vec<String>,
    mut set: ChangeSet,
    mut warnings: Vec<String>,
    args: &Value,
    cwd: &Path,
    tidy: Option<tidy::Tidy<'_>>,
) -> Result<Value> {
    use colab_core::render::{self, ChangeCollector, Detail, RenderOptions};

    if flag(args, "tidy")
        && let Some(tidy) = &tidy
    {
        let rule = labels.len();
        labels.push(format!("tidy via {}", tidy.server.program));
        warnings.extend(tidy::tidy_change_set(tidy, &mut set, rule)?);
    }
    let options = crate::apply::ApplyOptions::default();
    let prepared = crate::apply::prepare(&set, &options)?;
    warnings.extend(prepared.warnings.iter().cloned());
    let write = flag(args, "write");
    if write {
        crate::apply::apply(&set, &prepared, &options, |_, _| Ok(()))?;
    }

    let detail = arg(args, "detail")
        .and_then(Detail::parse)
        .unwrap_or(Detail::Counts);
    let mut collector = ChangeCollector::new(RenderOptions {
        detail,
        max_files: render::DEFAULT_MAX_FILES,
        max_diff_bytes: render::DEFAULT_MAX_DIFF_BYTES,
    });
    let mut report = colab_core::RunReport::with_rules(labels);
    let rel = |p: &Path| relative(p, cwd).to_path_buf();
    report.set_counts(set.edits.len() as u64, 0, 0);
    report.set_file_ops(set.ops.iter().map(|op| op.map_paths(rel)).collect());
    report.warn_all(warnings);
    for edit in &set.edits {
        let mut change = edit.clone();
        change.path = rel(&change.path);
        report.record(&change);
        collector.push(&change);
    }
    if report.rules()[0].files_matched == 0 && !set.ops.is_empty() {
        report.rules_mut()[0].files_matched = set.ops.len() as u64;
    }
    let mut value = collector.to_json(&report);
    let advisories = render::advisories(&report);
    if !advisories.is_empty() {
        value["warnings"] = json!(advisories);
    }
    value["written"] = json!(write);
    Ok(value)
}
