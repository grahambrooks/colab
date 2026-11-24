//! `--tidy`: organize imports and format each changed file, through the
//! language server or `colab.toml`'s `[formatters]`.

use super::*;

/// The server `--tidy` asks.
pub struct Tidy<'a> {
    pub server: &'a ServerCommand,
    pub root: &'a Path,
    pub timeout: u64,
}

/// Organize imports and format every edited file in `set`, in place,
/// recording `rule` on each file tidying changed. Returns warnings for
/// steps the server could not do.
pub fn tidy_change_set(tidy: &Tidy<'_>, set: &mut ChangeSet, rule: usize) -> Result<Vec<String>> {
    if set.edits.is_empty() {
        return Ok(Vec::new());
    }
    info!(
        "Tidying {} file(s) via `{}`",
        set.edits.len(),
        tidy.server.display()
    );
    let files: Vec<(PathBuf, String)> = set
        .edits
        .iter()
        .map(|e| (e.path.clone(), e.after.clone()))
        .collect();
    let tidied = actions::tidy(
        tidy.server,
        tidy.root,
        &files,
        Duration::from_secs(tidy.timeout),
        SETTLE,
    )?;
    let mut skipped: Vec<String> = Vec::new();
    for (edit, result) in set.edits.iter_mut().zip(tidied) {
        if result.text != edit.after {
            edit.after = result.text;
            edit.rules_fired.push(rule);
        }
        for step in result.skipped {
            if !skipped.contains(&step) {
                skipped.push(step);
            }
        }
    }
    Ok(skipped
        .into_iter()
        .map(|step| format!("--tidy skipped {step} via `{}`", tidy.server.program))
        .collect())
}

/// `true` when `program` is a path that exists or a name found on `PATH`.
pub(crate) fn on_path(program: &str) -> bool {
    let program = Path::new(program);
    if program.components().count() > 1 {
        return program.is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

/// The report's entry for a tidy step.
pub(crate) fn tidy_rule(index: usize, by: &str) -> colab_core::RuleStat {
    colab_core::RuleStat {
        index,
        rule: format!("{}{by}", colab_core::report::TIDY_RULE),
        files_matched: 0,
        near_misses: Vec::new(),
        unchanged: None,
        message: None,
        severity: colab_core::Severity::Info,
        id: None,
        url: None,
    }
}

/// `source` through the formatter `command` (run by `sh -c`, `{path}`
/// replaced by the file's quoted path), or why it failed.
pub(crate) fn run_formatter(
    command: &str,
    path: &Path,
    source: &str,
) -> std::result::Result<String, String> {
    use std::io::Write;
    use std::process::Stdio;
    #[cfg(windows)]
    let quoted = format!("\"{}\"", path.display());
    #[cfg(not(windows))]
    let quoted = format!("'{}'", path.display().to_string().replace('\'', r"'\''"));
    let mut child = crate::cli::shell(&command.replace("{path}", &quoted))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let input = source.to_string();
    // Written from a thread, so a formatter that fills its stdout pipe
    // before reading all of stdin cannot deadlock.
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let _ = writer.join();
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("{}: {}", out.status, err.trim()));
    }
    String::from_utf8(out.stdout).map_err(|e| e.to_string())
}

/// `refactor --tidy`: tidy each changed file through its language server,
/// grouped by server and project root, in memory. Each server becomes a
/// rule after the script's, so the report shows what tidying changed.
/// A file with no default server is left as the rules made it, with a
/// warning.
///
/// Without an explicit `command`, a file whose extension `formatters`
/// (`colab.toml`'s `[formatters]`) names goes through that command
/// instead: it reads the file on stdin and writes it on stdout.
pub fn tidy_pending(
    command: Option<&str>,
    formatters: &std::collections::BTreeMap<String, String>,
    changes: &mut [colab_core::FileChange],
    report: &mut colab_core::RunReport,
) -> Result<()> {
    use std::collections::BTreeMap;
    let cwd = std::env::current_dir().map_err(io_to_error)?;
    // (server, root) -> indices into `changes`.
    let mut groups: BTreeMap<(String, PathBuf), (ServerCommand, Vec<usize>)> = BTreeMap::new();
    let mut formatter_rules: BTreeMap<String, usize> = BTreeMap::new();
    for (i, change) in changes.iter_mut().enumerate() {
        let formatter = command
            .is_none()
            .then(|| change.path.extension().and_then(|e| e.to_str()))
            .flatten()
            .and_then(|e| formatters.get(e));
        if let Some(formatter) = formatter {
            let rule = *formatter_rules.entry(formatter.clone()).or_insert_with(|| {
                let index = report.rules().len();
                report.add_rule(tidy_rule(index, formatter));
                index
            });
            match run_formatter(formatter, &change.path, &change.after) {
                Ok(out) if out != change.after => {
                    change.after = out;
                    if !change.rules_fired.contains(&rule) {
                        change.rules_fired.push(rule);
                    }
                }
                Ok(_) => {}
                Err(why) => report.warn(format!(
                    "--tidy: `{formatter}` failed on {}, so it is left as the rules made it: {why}",
                    change.path.display()
                )),
            }
            continue;
        }
        let chosen = match command {
            Some(command) => Some(ServerCommand::parse(command)?),
            None => ServerCommand::default_for(&change.path),
        };
        let Some(server) = chosen else {
            report.warn(format!(
                "--tidy: no default language server for {}; left as the rules made it",
                change.path.display()
            ));
            continue;
        };
        // A default the machine lacks is skipped, not fatal: one missing
        // server should not stop the other languages being tidied.
        if command.is_none() && !on_path(&server.program) {
            report.warn(format!(
                "--tidy: `{}` is not on PATH, so {} is left as the rules made it \
                 (install it, or name a formatter for it in colab.toml's [formatters])",
                server.program,
                change.path.display()
            ));
            continue;
        }
        let path = fs::canonicalize(&change.path).map_err(|e| Error::io_at(&change.path, e))?;
        let root = root_for(None, &path, &cwd)?;
        groups
            .entry((server.display(), root))
            .or_insert_with(|| (server, Vec::new()))
            .1
            .push(i);
    }
    let mut rules: BTreeMap<String, usize> = BTreeMap::new();
    for ((name, root), (server, indices)) in groups {
        let rule = *rules.entry(name.clone()).or_insert_with(|| {
            let index = report.rules().len();
            report.add_rule(tidy_rule(index, &server.program));
            index
        });
        let mut set = ChangeSet {
            ops: Vec::new(),
            edits: indices
                .iter()
                .map(|&i| {
                    let mut edit = changes[i].clone();
                    edit.path = fs::canonicalize(&edit.path).unwrap_or(edit.path);
                    edit
                })
                .collect(),
        };
        let tidy = Tidy {
            server: &server,
            root: &root,
            timeout: 120,
        };
        report.warn_all(tidy_change_set(&tidy, &mut set, rule)?);
        for (&i, edit) in indices.iter().zip(set.edits) {
            changes[i].after = edit.after;
            changes[i].rules_fired = edit.rules_fired;
        }
    }
    Ok(())
}
