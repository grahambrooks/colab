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
    pub(super) script: FindScript,

    /// The language of a bare `--pattern` (`rust`, `go`, …). Without it,
    /// the language is the one whose `pattern` compiles and applies to the
    /// most of the files searched.
    #[arg(long, value_name = "LANG")]
    pub(super) lang: Option<String>,

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

/// What `find` searches for: a script, inline rules, or a bare pattern
/// (`colab find 'fn $F($$$A)' src/`, as grep takes a regex). One of them;
/// a first path that is not on disk is taken as the pattern.
#[derive(clap::Args, Debug)]
#[group(multiple = false)]
pub(super) struct FindScript {
    /// Path to the codemod script. `-` reads the script from stdin;
    /// `builtin:<pack>` runs a pack built into colab (`colab pack list`).
    #[arg(long = "script", value_name = "PATH")]
    script_path: Option<PathBuf>,

    /// An inline rule, e.g. `match go::import "a" { replace "b" }`.
    /// Repeatable.
    #[arg(short = 'e', long = "rule", value_name = "RULE")]
    rules: Vec<String>,

    /// A structural pattern to find, e.g. `errors.Wrap($E, $M)`: the
    /// rule `match <lang>::pattern '<it>'`, with `--lang` or the language
    /// inferred from the files.
    #[arg(short = 'p', long = "pattern", value_name = "CODE")]
    pattern: Option<String>,
}

impl FindArgs {
    /// The script to run: the one given, or a bare pattern's rule.
    fn compile(&mut self, backends: &BackendRegistry) -> Result<codemod::Refactoring> {
        let given = ScriptArgs {
            script_path: self.script.script_path.take(),
            rules: std::mem::take(&mut self.script.rules),
        };
        if given.script_path.is_some() || !given.rules.is_empty() {
            return given.compile(backends);
        }
        // `colab find '<pattern>' <paths>`: grep's shape. The pattern is
        // the first path that does not exist.
        let pattern = match self.script.pattern.take() {
            Some(pattern) => pattern,
            None => match self.paths.first() {
                Some(first) if !first.exists() => {
                    self.paths.remove(0).to_string_lossy().into_owned()
                }
                _ => {
                    return Err(Error::Config(
                        "give what to find: a pattern (`colab find 'fn $F($$$A)' src/`, or \
                         --pattern with --lang), -e '<rule>', or --script <file>"
                            .into(),
                    ));
                }
            },
        };
        let quoted = quote(&pattern);
        let lang = match self.lang.clone() {
            Some(lang) => lang,
            None => self.infer_lang(&quoted, backends)?,
        };
        let rule = format!("match {lang}::pattern {quoted} {{ delete }}");
        ScriptArgs {
            script_path: None,
            rules: vec![rule],
        }
        .compile(backends)
    }

    /// The language a bare pattern is in: the language most of the files
    /// under the paths are in (looking at the first few thousand), when the
    /// pattern parses in it.
    fn infer_lang(&self, quoted: &str, backends: &BackendRegistry) -> Result<String> {
        const SAMPLE: usize = 5_000;
        // Every language with patterns, by a rule that compiles in each:
        // its relevance is the language's files, whatever the pattern.
        let candidates: Vec<(String, codemod::Refactoring)> = backends
            .languages()
            .into_iter()
            .filter(|lang| {
                backends
                    .get(lang)
                    .is_some_and(|b| b.capabilities().iter().any(|c| c.module == "pattern"))
            })
            .filter_map(|lang| {
                let rule = format!("match {lang}::pattern '$X' {{ delete }}");
                codemod::compile(&rule, backends)
                    .ok()
                    .map(|r| (lang.to_string(), r))
            })
            .collect();
        let parses = |lang: &str| {
            codemod::compile(
                &format!("match {lang}::pattern {quoted} {{ delete }}"),
                backends,
            )
            .is_ok()
        };
        // A census, not a search: the walk asks each path's relevance, which
        // counts it per language and says no, so no file is read.
        struct Census<'a> {
            candidates: &'a [(String, codemod::Refactoring)],
            counts: Vec<std::sync::atomic::AtomicUsize>,
            seen: std::sync::atomic::AtomicUsize,
        }
        impl colab_core::CodeTransformer for Census<'_> {
            fn is_file_relevant(&self, path: &Path) -> bool {
                use std::sync::atomic::Ordering::Relaxed;
                if self.seen.fetch_add(1, Relaxed) < SAMPLE {
                    for (i, (_, rules)) in self.candidates.iter().enumerate() {
                        if rules.is_file_relevant(path) {
                            self.counts[i].fetch_add(1, Relaxed);
                        }
                    }
                }
                false
            }
            fn apply(&self, source: &str) -> String {
                source.to_string()
            }
        }
        let census = Census {
            candidates: &candidates,
            counts: candidates.iter().map(|_| Default::default()).collect(),
            seen: Default::default(),
        };
        Source::Walk {
            targets: if self.paths.is_empty() {
                vec![PathBuf::from(".")]
            } else {
                self.paths.clone()
            },
            options: WalkOptions {
                include: self.include.clone(),
                exclude: self.exclude.clone(),
                respect_gitignore: !self.no_ignore,
                follow_symlinks: false,
                jobs: resolve_jobs(self.jobs),
            },
        }
        .visit(&census, &mut |_| Ok(()))?;
        // The languages present, most files first.
        let mut present: Vec<(&str, usize)> = candidates
            .iter()
            .zip(&census.counts)
            .map(|((lang, _), n)| (lang.as_str(), n.load(std::sync::atomic::Ordering::Relaxed)))
            .filter(|(_, n)| *n > 0)
            .collect();
        present.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        let Some(&(top, files)) = present.first() else {
            return Err(Error::Config(
                "no file under the paths is in a language colab has patterns for; pass --lang \
                 and the files to search"
                    .into(),
            ));
        };
        // A tie between languages the pattern parses in cannot be settled.
        let tied: Vec<&str> = present
            .iter()
            .filter(|(lang, n)| *n == files && parses(lang))
            .map(|(lang, _)| *lang)
            .collect();
        if tied.len() > 1 {
            return Err(Error::Config(format!(
                "{quoted} could be {} ({files} file(s) each); pass --lang",
                tied.join(" or ")
            )));
        }
        if parses(top) {
            log::info!("searching {top} files ({files} here; --lang chooses another language)");
            return Ok(top.to_string());
        }
        // The files are mostly `top`, and the pattern is not `top` code.
        let elsewhere: Vec<&str> = present
            .iter()
            .filter(|(lang, _)| parses(lang))
            .map(|(lang, _)| *lang)
            .collect();
        let others = if elsewhere.is_empty() {
            String::new()
        } else {
            format!(
                " (it does parse as {}; pass --lang for that)",
                elsewhere.join(", ")
            )
        };
        Err(Error::Config(format!(
            "the files here are mostly {top} ({files}), and {quoted} does not parse as a {top} \
             pattern{others}. A pattern is complete code with metavariables: a function needs its \
             body (`fn $F($$$A) -> $R {{ $$$B }}`). `colab ast --lang {top} --pattern …` shows how \
             it parses"
        )))
    }
}

/// `text` as a DSL string literal: single quotes, else double, else raw.
fn quote(text: &str) -> String {
    if !text.contains('\'') {
        format!("'{text}'")
    } else if !text.contains('"') {
        format!("\"{text}\"")
    } else {
        let mut hashes = String::from("#");
        while text.contains(&format!("\"{hashes}")) {
            hashes.push('#');
        }
        format!("r{hashes}\"{text}\"{hashes}")
    }
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

pub(super) fn run_find(mut args: FindArgs) -> Result<u8> {
    use colab_core::find::{FindDetail, FindReport, Selector};

    change_working_dir(&args.change_dir)?;
    let backends = default_backends();
    let mut refactoring = args.compile(&backends)?;
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
