//! The tools that compile and run a script: `colab.preview`,
//! `colab.apply`, the workflow tools' generated scripts, `colab.test_rule`,
//! `colab.find`, and `colab.ast` (a pattern's tree).

use super::*;

#[derive(Clone, Copy)]
pub(crate) enum RunMode {
    Preview,
    Apply,
}

/// Compile a script, rooting `include "..."` resolution at `cwd` when
/// one was supplied.
///
/// Without a base path `include` is unusable, and the underlying error
/// names Rust API functions an MCP caller cannot reach — so `cwd` is what
/// makes includes work over this transport at all.
pub(crate) fn compile_script(
    script: &str,
    cwd: Option<&Path>,
    backends: &BackendRegistry,
) -> Result<colab_dsl::Refactoring, colab_core::Error> {
    // Bare `match …` rules parse as they are (the grammar takes them), so
    // a parse error points into the text the caller sent.
    let refactoring = match cwd {
        // `compile_at_path` resolves includes against the file's parent,
        // so point it at a notional script inside `cwd`.
        Some(dir) => colab_dsl::compile_at_source(script, &dir.join("<script>"), backends),
        None => compile(script, backends),
    }?;
    // The MCP server starts no language server, so it cannot answer them.
    if refactoring.needs_oracle() {
        return Err(colab_core::Error::UnsupportedOperation(
            "`type` and `resolves` constraints are answered by a language server, which the MCP \
             server does not run; run the script with the colab CLI (`colab refactor` / `colab find`)"
                .into(),
        ));
    }
    Ok(refactoring)
}

/// Run a script a workflow tool generated, previewing unless `write` is
/// true, and return the report with the `script` beside it.
pub(crate) fn run_generated(
    script: String,
    args: &Value,
    backends: &BackendRegistry,
    extra: &dyn ToolProvider,
) -> Result<Value, ToolError> {
    let mut run_args = args.clone();
    run_args["script"] = json!(script);
    let mode = if arg_bool(args, "write")? {
        RunMode::Apply
    } else {
        RunMode::Preview
    };
    let mut value = run_script(&run_args, backends, extra, mode, &mut NoProgress)?;
    value["script"] = json!(script);
    Ok(value)
}

/// Compile and run a script, reporting progress through `sink`.
///
/// The single implementation behind both `colab.preview`/`colab.apply`
/// entry points. Pass [`NoProgress`] for the synchronous path and
/// [`Notifications`] when the client supplied a `progressToken`.
pub(crate) fn run_script<P: ProgressSink>(
    args: &Value,
    backends: &BackendRegistry,
    extra: &dyn ToolProvider,
    mode: RunMode,
    sink: &mut P,
) -> Result<Value, ToolError> {
    use colab_dsl::pipeline::{Hooks, Pipeline, Source};

    /// Progress ticks per file the walk produces.
    struct Progress<'s, P>(&'s mut P, u64);
    impl<P: ProgressSink> Hooks for Progress<'_, P> {
        fn progress(&mut self, files: u64) {
            self.1 = files;
            self.0.tick(files);
        }
    }

    let script = arg_str(args, "script")?;
    let cwd = arg_opt_path(args, "cwd")?;
    let paths = arg_paths(args, "paths", cwd.as_deref())?;
    let options = arg_render_options(args)?;
    let allow_conflicts = arg_bool(args, "allow_conflicts")?;
    let checks = if arg_bool(args, "no_link_check")? {
        Vec::new()
    } else {
        extra.checks(&paths)
    };

    let refactoring =
        compile_script(&script, cwd.as_deref(), backends).map_err(ToolError::from_error)?;

    let mut progress = Progress(sink, 0);
    let outcome = Pipeline::new(&refactoring, Source::walk(paths))
        .write(matches!(mode, RunMode::Apply))
        .allow_conflicts(allow_conflicts)
        .refusal_hint("fix the rules, or call again with allow_conflicts: true")
        .checks(checks)
        .execute(&mut progress)
        .map_err(ToolError::from_error)?;
    let files_processed = progress.1;
    sink.finish(files_processed);

    let mut collector = ChangeCollector::new(options);
    for change in &outcome.changes {
        collector.push(change);
    }
    let mode = if outcome.blocked {
        RunMode::Preview
    } else {
        mode
    };
    Ok(finish_report(&collector, &outcome.report, mode))
}

/// `colab.test_rule`: run a script over one in-memory snippet.
///
/// The MCP twin of `colab refactor --stdin --path <hint>`. It goes
/// through the same `apply_at` the walker uses, so per-file gating
/// (`is_file_relevant`, `in "<glob>"`) and per-rule attribution behave
/// exactly as they would on disk — a rule that says `files: 0` here will
/// say it in `colab.preview` too.
pub(crate) fn test_rule(args: &Value, backends: &BackendRegistry) -> Result<Value, ToolError> {
    let script = arg_str(args, "script")?;
    let source = arg_str(args, "source")?;
    let path = PathBuf::from(arg_str(args, "path")?);
    let cwd = arg_opt_path(args, "cwd")?;
    let expected = match args.get("expected") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err(ToolError::protocol("argument `expected` must be a string")),
    };

    let refactoring =
        compile_script(&script, cwd.as_deref(), backends).map_err(ToolError::from_error)?;
    let (change, report) = colab_dsl::pipeline::run_text(&refactoring, path.clone(), source)
        .map_err(ToolError::from_error)?;

    let mut out = Map::new();
    out.insert("changed".into(), json!(change.changed()));
    out.insert("rules".into(), render::rules_json(&report));
    if let Some(conflicts) = render::conflicts_json(&report) {
        out.insert("conflicts".into(), conflicts);
    }
    if change.changed() {
        out.insert(
            "diff".into(),
            json!(render::unified_diff(&path, &change.before, &change.after)),
        );
    }
    if let Some(expected) = expected {
        let matched = expected == change.after;
        out.insert("matches_expected".into(), json!(matched));
        if !matched {
            out.insert(
                "expected_diff".into(),
                json!(render::unified_diff(&path, &change.after, &expected)),
            );
        }
    }

    let mut warnings = render::advisories(&report);
    if !refactoring.rules.iter().any(|r| r.is_file_relevant(&path)) {
        warnings.push(format!(
            "No rule applies to `{}`: check its extension against the languages the script targets, and any `in \"<glob>\"` scope.",
            path.display()
        ));
    }
    if !warnings.is_empty() {
        out.insert("warnings".into(), json!(warnings));
    }
    out.insert("output".into(), json!(change.after));
    Ok(Value::Object(out))
}

/// `colab.find`: read-only search, the twin of `colab find --format json`.
pub(crate) fn find(args: &Value, backends: &BackendRegistry) -> Result<Value, ToolError> {
    use colab_core::find::{FindDetail, FindReport, Hit, Selector};

    let script = arg_str(args, "script")?;
    let cwd = arg_opt_path(args, "cwd")?;
    let paths = arg_paths(args, "paths", cwd.as_deref())?;
    let detail = match args.get("detail") {
        None | Some(Value::Null) => FindDetail::Concise,
        Some(Value::String(s)) => FindDetail::parse(s).ok_or_else(|| {
            ToolError::protocol(format!(
                "argument `detail` must be `concise` or `detailed`, not `{s}`"
            ))
        })?,
        Some(_) => return Err(ToolError::protocol("argument `detail` must be a string")),
    };
    let max_results = arg_usize(args, "max_results", colab_core::find::DEFAULT_MAX_RESULTS)?;

    let refactoring =
        compile_script(&script, cwd.as_deref(), backends).map_err(ToolError::from_error)?;
    let source = colab_dsl::pipeline::Source::walk(paths);
    // Scans first, over every file and every stage, so `in` reads what
    // colab.preview would; hits are still located in the files as they are.
    if refactoring.has_scans() {
        source
            .visit_phased(&refactoring, &mut |_| Ok(()))
            .map_err(ToolError::from_error)?;
    }
    let rules = &refactoring.rules;
    let mut report = FindReport::new(rules, max_results, detail);
    refactoring.annotate(&mut report.run);
    report.script = refactoring.name.clone();
    let selector = Selector::new(rules, &report.script);
    let outcome = source
        .visit(&selector, &mut |change: colab_core::FileChange| {
            report.record_found(&change.path, selector.take(&change.path));
            Ok(())
        })
        .map_err(ToolError::from_error)?;
    report.run.record_walk(outcome);
    colab_dsl::pipeline::semantic_failure().map_err(ToolError::from_error)?;
    colab_core::near_miss::probe_disk(rules, &mut report.run);

    // Report paths as the caller named them: relative to `cwd`.
    if let Some(cwd) = &cwd {
        report.hits = std::mem::take(&mut report.hits)
            .into_iter()
            .map(|hit| Hit {
                path: hit
                    .path
                    .strip_prefix(cwd)
                    .map(Path::to_path_buf)
                    .unwrap_or(hit.path),
                ..hit
            })
            .collect();
    }
    Ok(render::find_json(&report))
}

/// `colab.ast`: a pattern's or a snippet's syntax tree.
pub(crate) fn ast(args: &Value, backends: &BackendRegistry) -> Result<Value, ToolError> {
    let lang = arg_str(args, "lang")?;
    let backend = backends.get(&lang).ok_or_else(|| {
        ToolError::from_error(colab_core::Error::UnsupportedOperation(format!(
            "unknown language `{}`; {}",
            lang,
            suggest::candidates_note(&lang, "languages", &backends.languages())
        )))
    })?;
    let pattern = args.get("pattern").and_then(|v| v.as_str());
    let source = args.get("source").and_then(|v| v.as_str());
    let (input, is_pattern) = match (pattern, source) {
        (Some(p), None) => (colab_core::AstInput::Pattern(p), true),
        (None, Some(s)) => (colab_core::AstInput::Source(s), false),
        _ => {
            return Err(ToolError::protocol(
                "give exactly one of `pattern` or `source`",
            ));
        }
    };
    let tree = backend.syntax_tree(input).map_err(ToolError::from_error)?;
    let mut out = json!({ "lang": lang, "tree": tree.tree });
    if is_pattern {
        out["context"] = json!(tree.context);
        out["prefilter"] = json!(tree.prefilter);
    } else {
        out["has_error"] = json!(tree.has_error);
    }
    Ok(out)
}

/// Attach the advisories every run should carry: dead rules, and why a
/// run that changed nothing changed nothing.
pub(crate) fn finish_report(
    collector: &ChangeCollector,
    report: &RunReport,
    mode: RunMode,
) -> Value {
    let mut value = collector.to_json(report);
    let obj = value.as_object_mut().expect("render produces an object");

    obj.insert("applied".into(), json!(matches!(mode, RunMode::Apply)));

    let warnings = render::advisories(report);
    if !warnings.is_empty() {
        obj.insert("warnings".into(), json!(warnings));
    }
    value
}
