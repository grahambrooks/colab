//! `rename --include-text`: the old name where the language server does
//! not look (comments, strings, config and docs), as whole words.

use super::*;

/// Files a text occurrence may be in without a parser: config and docs.
pub(crate) const TEXT_EXTENSIONS: &[&str] = &[
    "toml", "json", "yaml", "yml", "md", "markdown", "txt", "rst", "adoc",
];

/// Whole-word occurrences of `old` the rename in `set` left behind, under
/// the git work tree holding `root` (else `root`), as edits to `new` on
/// top of it: in source files, only inside comments and
/// string contents (`LanguageBackend::prose_ranges`); in config and docs
/// files, anywhere. Each edit's `before` is the file as the rename leaves
/// it, at its final path, and is attributed to rule 1.
pub(crate) fn text_occurrences(
    old: &str,
    new: &str,
    root: &Path,
    set: &ChangeSet,
) -> Result<Vec<colab_core::FileChange>> {
    // The server's root is the nearest project marker, which in a
    // workspace is one crate or package; docs and config usually live at
    // the top of the work tree, so search that when there is one.
    let top = crate::git::Repo::discover(root).map(|repo| repo.top().to_path_buf());
    let root = top.as_deref().unwrap_or(root);
    let registry = colab_backends::registry();
    // A backend claims a path when its own `symbol` rule would.
    let claims: Vec<(
        &dyn colab_core::LanguageBackend,
        Box<dyn colab_core::Operation>,
    )> = registry
        .languages()
        .into_iter()
        .filter_map(|lang| {
            let backend = registry.get(lang)?;
            backend.prose_ranges("")?;
            let op = backend
                .build_rule(
                    "symbol",
                    colab_core::RuleSpec::Replace {
                        target: old.into(),
                        replacement: new.into(),
                    },
                )
                .ok()?;
            Some((backend, op))
        })
        .collect();
    let renamed: std::collections::HashMap<PathBuf, &colab_core::FileChange> = set
        .edits
        .iter()
        .map(|e| (set.origin_of(&e.path).unwrap_or_else(|| e.path.clone()), e))
        .collect();

    let mut out = Vec::new();
    let candidates = Candidates { claims: &claims };
    colab_core::walker::walk(&candidates, root, &mut |change| {
        let (path, base) = match renamed.get(&change.path) {
            Some(edit) => (edit.path.clone(), edit.after.clone()),
            None => (change.path.clone(), change.before),
        };
        let prose = match claims
            .iter()
            .find(|(_, op)| op.is_file_relevant(&change.path))
        {
            Some((backend, _)) => backend.prose_ranges(&base),
            None => Some(std::iter::once(0..base.len()).collect()),
        };
        let Some(prose) = prose else { return Ok(()) };
        let after = replace_words(&base, old, new, &prose);
        if after != base {
            out.push(colab_core::FileChange {
                path,
                before: base,
                after,
                rules_fired: vec![1],
                conflicts: Vec::new(),
                created: false,
            });
        }
        Ok(())
    })?;
    Ok(out)
}

/// Selects the files [`text_occurrences`] reads; it never changes them.
pub(crate) struct Candidates<'a> {
    pub(crate) claims: &'a [(
        &'a dyn colab_core::LanguageBackend,
        Box<dyn colab_core::Operation>,
    )],
}

impl colab_core::CodeTransformer for Candidates<'_> {
    fn is_file_relevant(&self, path: &Path) -> bool {
        let text = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| TEXT_EXTENSIONS.contains(&e));
        text || self.claims.iter().any(|(_, op)| op.is_file_relevant(path))
    }

    fn apply(&self, source: &str) -> String {
        source.to_string()
    }
}

/// `text` with every whole-word `old` inside one of `ranges` made `new`.
pub(crate) fn replace_words(
    text: &str,
    old: &str,
    new: &str,
    ranges: &[std::ops::Range<usize>],
) -> String {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (at, _) in text.match_indices(old) {
        let end = at + old.len();
        let bounded = !text[..at].chars().next_back().is_some_and(is_word)
            && !text[end..].chars().next().is_some_and(is_word);
        if bounded && at >= last && ranges.iter().any(|r| r.start <= at && end <= r.end) {
            out.push_str(&text[last..at]);
            out.push_str(new);
            last = end;
        }
    }
    out.push_str(&text[last..]);
    out
}

/// Fold text edits into the server's change set: a file both touched keeps
/// the server's `before` and takes the text edit's `after`.
pub(crate) fn merge_text(set: &mut ChangeSet, text: Vec<colab_core::FileChange>) {
    for change in text {
        match set.edits.iter_mut().find(|e| e.path == change.path) {
            Some(edit) => {
                edit.after = change.after;
                edit.rules_fired.push(1);
            }
            None => set.edits.push(change),
        }
    }
}

/// `{files, occurrences, included, sample: [{path, line, text}]}`, the
/// sample capped so a common word cannot flood the response.
pub(crate) fn text_summary(text: &[colab_core::FileChange], cwd: &Path, included: bool) -> Value {
    const SAMPLE: usize = 20;
    let mut sample = Vec::new();
    let mut occurrences = 0;
    for change in text {
        for (number, (old, new)) in change.before.lines().zip(change.after.lines()).enumerate() {
            if old == new {
                continue;
            }
            occurrences += 1;
            if sample.len() < SAMPLE {
                sample.push(json!({
                    "path": relative(&change.path, cwd).display().to_string(),
                    "line": number + 1,
                    "text": old.trim(),
                }));
            }
        }
    }
    json!({"files": text.len(), "lines": occurrences, "included": included, "sample": sample})
}
