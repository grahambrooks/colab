//! Read-only commands: `colab find` (where each rule matches) and
//! `colab ast` (a pattern's or snippet's syntax tree).

use super::*;

#[derive(Parser, Debug)]
pub(super) struct FindArgs {
    #[command(flatten)]
    pub(super) select: SelectArgs,

    /// The language server that answers `type` / `resolves` constraints,
    /// instead of each language's usual one (`gopls`, `rust-analyzer`, …).
    #[arg(long = "lsp", value_name = "COMMAND")]
    pub(super) lsp: Option<String>,

    /// Seconds to wait for that server to start and index.
    #[arg(long = "lsp-timeout", value_name = "SECS", default_value_t = 120)]
    pub(super) lsp_timeout: u64,

    #[command(flatten)]
    pub(super) script: ScriptArgs,

    /// Change to this working directory before resolving paths.
    #[arg(short = 'C', long = "change-dir", default_value = ".")]
    pub(super) change_dir: PathBuf,

    /// `human`: `path:line:col: text` lines. `json`: one document.
    #[arg(long, value_enum, default_value_t = FindFormat::Human)]
    pub(super) format: FindFormat,

    /// `concise`: the first line of each match. `detailed`: adds
    /// metavariable captures and the full (capped) match text.
    #[arg(long, value_enum, default_value_t = FindDetailArg::Concise)]
    pub(super) detail: FindDetailArg,

    /// List at most this many matches; the rest are counted. Default 50,
    /// or every match with `--table`.
    #[arg(long = "max-results", value_name = "N")]
    pub(super) max_results: Option<usize>,

    /// Every match as a row: `path,line,column,rule,id`, a column per
    /// metavariable (`$X`), and the match `text`. `csv` or `ndjson`.
    #[arg(long, value_enum, value_name = "FORMAT")]
    pub(super) table: Option<crate::table::TableFormat>,

    /// With `--table`: one row per distinct value of these columns
    /// (`$X`, `rule`, `id`, `path`), with a `count`, most common first.
    /// Repeatable.
    #[arg(long = "group-by", value_name = "COLUMN", requires = "table")]
    pub(super) group_by: Vec<String>,

    /// Whitelist files via gitignore-syntax glob. Repeatable.
    #[arg(long = "include", value_name = "GLOB")]
    pub(super) include: Vec<String>,

    /// Blacklist files via gitignore-syntax glob. Repeatable.
    #[arg(long = "exclude", value_name = "GLOB")]
    pub(super) exclude: Vec<String>,

    /// Don't honour `.gitignore` or hidden-file rules.
    #[arg(long = "no-ignore")]
    pub(super) no_ignore: bool,

    /// Worker thread count. Defaults to the available parallelism. Falls back to
    /// the `COLAB_JOBS` env var when unset. Set to 1 to search one file at a
    /// time. The results are the same, in the same order, for any count.
    #[arg(long, value_name = "N")]
    pub(super) jobs: Option<usize>,

    /// Files or directories to search. Defaults to `.`.
    pub(super) paths: Vec<PathBuf>,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FindFormat {
    Human,
    Json,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FindDetailArg {
    Concise,
    Detailed,
}

#[derive(Parser, Debug)]
pub(super) struct AstArgs {
    /// Language backend whose grammar to use, e.g. `go`.
    #[arg(long)]
    pub(super) lang: String,

    /// Show how this pattern parses (metavariables included), instead
    /// of a snippet.
    #[arg(long, conflicts_with = "stdin")]
    pub(super) pattern: Option<String>,

    /// Read the snippet to parse from stdin.
    #[arg(long)]
    pub(super) stdin: bool,

    /// `human`: the tree as indented text. `json`: one document.
    #[arg(long, value_enum, default_value_t = FindFormat::Human)]
    pub(super) format: FindFormat,
}

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
