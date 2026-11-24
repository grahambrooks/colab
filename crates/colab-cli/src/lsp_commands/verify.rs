//! `--verify-lsp`: a change's new error diagnostics, as conflicts.

use super::*;

/// The 1-based line of an `added_errors` entry (`line N: message`).
pub(crate) fn error_line(error: &str) -> Option<usize> {
    error
        .strip_prefix("line ")?
        .split(':')
        .next()?
        .trim()
        .parse()
        .ok()
}

/// The rule to credit with an error on `line` (1-based, in the changed
/// text) of `change`: the one whose own change holds that line — found by
/// applying each rule alone, as the CI formats do — or, when no change
/// holds it (an error the change caused elsewhere), the only rule that
/// changed the file. `None` when that cannot be told.
pub(crate) fn blame(
    change: &colab_core::FileChange,
    rules: &[Box<dyn colab_core::Operation>],
    report: &mut colab_core::RunReport,
    line: usize,
) -> Option<usize> {
    if !report.spans().contains_key(&change.path) {
        colab_core::report::attribute(rules, std::slice::from_ref(change), report);
    }
    let at = line.checked_sub(1)?;
    let spans = report.spans().get(&change.path).map(Vec::as_slice);
    let holding: Vec<usize> = colab_core::render::blocks(change, spans)
        .into_iter()
        .find(|b| b.new.contains(&at) || (b.new.is_empty() && b.new.start == at))
        .map_or_default(|b| b.rules);
    match (holding.as_slice(), change.rules_fired.as_slice()) {
        ([one], _) => Some(*one),
        ([], [only]) => Some(*only),
        _ => None,
    }
}

/// `--verify-lsp`: ask each changed file's language server whether the
/// change adds errors, before anything is written, and record each added
/// error as a conflict. With `rules` (the script), an error is credited to
/// the rule whose change holds its line ([`blame`]), or to the only rule
/// that changed the file; otherwise it is the whole file's. `command`
/// overrides the server chosen by extension. A file with no default
/// server is noted, not checked.
pub fn verify_lsp(
    command: Option<&str>,
    changes: &[colab_core::FileChange],
    report: &mut colab_core::RunReport,
    rules: Option<&[Box<dyn colab_core::Operation>]>,
) -> Result<()> {
    use colab_lsp_client::diagnostics::{Changed, added_errors};
    use std::collections::BTreeMap;

    let cwd = std::env::current_dir().map_err(io_to_error)?;
    // (server, root) -> files; and each canonical path's name as reported.
    let mut groups: BTreeMap<(String, PathBuf), (ServerCommand, Vec<Changed>)> = BTreeMap::new();
    let mut shown: std::collections::HashMap<PathBuf, PathBuf> = Default::default();
    for change in changes.iter().filter(|c| c.changed()) {
        let server = match command {
            Some(command) => ServerCommand::parse(command)?,
            None => match ServerCommand::default_for(&change.path) {
                Some(server) => server,
                None => {
                    report.warn(format!(
                        "--verify-lsp: no default language server for {}; not checked (pass --verify-lsp='<command>')",
                        change.path.display()
                    ));
                    continue;
                }
            },
        };
        let path = fs::canonicalize(&change.path).map_err(|e| Error::io_at(&change.path, e))?;
        let root = root_for(None, &path, &cwd)?;
        shown.insert(path.clone(), change.path.clone());
        groups
            .entry((server.display(), root))
            .or_insert_with(|| (server, Vec::new()))
            .1
            .push(Changed {
                path,
                before: change.before.clone(),
                after: change.after.clone(),
            });
    }
    for ((name, root), (server, files)) in groups {
        info!(
            "--verify-lsp: asking `{name}` about {} file(s) in {}",
            files.len(),
            root.display()
        );
        for added in added_errors(&server, &root, Duration::from_secs(120), SETTLE, &files)? {
            let path = shown.get(&added.path).cloned().unwrap_or(added.path);
            let change = changes.iter().find(|c| c.path == path);
            for error in added.errors {
                let rule = change.zip(rules).and_then(|(change, rules)| {
                    let line = error_line(&error)?;
                    blame(change, rules, report, line)
                });
                report.add_conflict(colab_core::Conflict {
                    path: path.clone(),
                    rule,
                    message: format!("`{name}` reports a new error at {error}"),
                });
            }
        }
    }
    Ok(())
}
