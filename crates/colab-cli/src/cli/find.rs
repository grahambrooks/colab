//! Read-only commands: `colab find` (where each rule matches) and
//! `colab ast` (a pattern's or snippet's syntax tree).

use super::*;

pub(super) fn run_find(args: FindArgs) -> Result<u8> {
    use colab_core::find::{FindDetail, FindReport, Selector};

    change_working_dir(&args.change_dir)?;
    let backends = default_backends();
    let mut refactoring = args.script.compile(&backends)?;
    args.select.apply(&mut refactoring)?;
    let _oracle = OracleGuard::install(&refactoring, args.lsp.as_deref(), args.lsp_timeout)?;
    let source = Source::Walk {
        targets: if args.paths.is_empty() {
            vec![PathBuf::from(".")]
        } else {
            args.paths.clone()
        },
        options: WalkOptions {
            include: args.include.clone(),
            exclude: args.exclude.clone(),
            respect_gitignore: !args.no_ignore,
            follow_symlinks: false,
            jobs: resolve_jobs(args.jobs),
        },
    };
    // Scans first, over every file and every stage, so `in` reads what
    // `refactor` would; hits are still located in the files as they are.
    if refactoring.has_scans() {
        source.visit_phased(&refactoring, &mut |_| Ok(()))?;
    }
    let rules = &refactoring.rules;
    let detail = match args.detail {
        FindDetailArg::Concise => FindDetail::Concise,
        FindDetailArg::Detailed => FindDetail::Detailed,
    };
    let max_results = args.max_results.unwrap_or(if args.table.is_some() {
        usize::MAX
    } else {
        colab_core::find::DEFAULT_MAX_RESULTS
    });
    let mut report = FindReport::new(rules, max_results, detail);
    refactoring.annotate(&mut report.run);
    report.script = refactoring.name.clone();

    let selector = Selector::new(rules, &report.script);
    let outcome = source.visit(&selector, &mut |change: FileChange| {
        report.record_found(&change.path, selector.take(&change.path));
        Ok(())
    })?;
    report.run.record_walk(outcome);
    semantic_failure()?;
    colab_core::near_miss::probe_disk(rules, &mut report.run);

    if let Some(format) = args.table {
        let mut table = crate::table::hits(&report);
        if !args.group_by.is_empty() {
            table = crate::table::group(&table, &args.group_by)?;
        }
        // Buffered: a row per match, and stdout alone writes each line.
        let mut out = io::BufWriter::new(io::stdout().lock());
        match format {
            crate::table::TableFormat::Csv => crate::table::write_csv(&mut out, &table),
            crate::table::TableFormat::Ndjson => crate::table::write_ndjson(&mut out, &table),
        }
        .and_then(|()| out.flush())
        .map_err(io_to_error)?;
        if report.truncated() > 0 {
            log::warn!(
                "{} more match(es) not in the table (--max-results {max_results})",
                report.truncated()
            );
        }
        return Ok(0);
    }

    match args.format {
        FindFormat::Json => {
            let mut out = io::stdout().lock();
            writeln!(out, "{}", render::find_json(&report)).map_err(io_to_error)?;
        }
        FindFormat::Human => {
            let mut out = io::BufWriter::new(io::stdout().lock());
            render::write_find_human(&mut out, &report)
                .and_then(|()| out.flush())
                .map_err(io_to_error)?;
        }
    }
    Ok(0)
}

pub(super) fn run_ast(args: AstArgs) -> Result<u8> {
    let backends = default_backends();
    let value = match (&args.pattern, args.stdin) {
        (Some(pattern), _) => discover::ast_of_pattern(&backends, &args.lang, pattern)?,
        (None, true) => {
            let mut source = String::new();
            io::stdin()
                .read_to_string(&mut source)
                .map_err(io_to_error)?;
            discover::ast_of_source(&backends, &args.lang, &source)?
        }
        (None, false) => {
            return Err(Error::Config(
                "`colab ast` needs `--pattern '<code>'` or `--stdin`".into(),
            ));
        }
    };
    match args.format {
        FindFormat::Json => print_json(&value),
        FindFormat::Human => {
            let mut out = io::stdout().lock();
            for key in ["context", "prefilter"] {
                if let Some(text) = value.get(key).and_then(|v| v.as_str()) {
                    writeln!(out, "{key}: {}", text.replace('\n', "⏎")).map_err(io_to_error)?;
                }
            }
            if value.get("has_error").and_then(serde_json::Value::as_bool) == Some(true) {
                writeln!(
                    out,
                    "warning: the snippet has syntax errors (see ERROR / MISSING nodes)"
                )
                .map_err(io_to_error)?;
            }
            let tree = value.get("tree").and_then(|v| v.as_str()).unwrap_or("");
            write!(out, "{tree}").map_err(io_to_error)?;
            Ok(0)
        }
    }
}
