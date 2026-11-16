//! The workspace edits behind `colab server`'s code actions: a run's
//! changes as minimal hunks, the rule annotations an editor's refactor
//! preview groups them by, and the inverse that `colab: undo` applies.

use std::collections::HashMap;
use std::path::PathBuf;

use colab_core::merge::{Merge, three_way};
use similar::{DiffOp, TextDiff};
use tower_lsp_server::ls_types::{
    AnnotatedTextEdit, ChangeAnnotation, DocumentChanges, OneOf,
    OptionalVersionedTextDocumentIdentifier, Range, TextDocumentEdit, TextEdit, Uri, WorkspaceEdit,
};

use super::position_at;

/// One file a run would change.
#[derive(Debug, Clone)]
pub(super) struct Planned {
    pub uri: Uri,
    pub path: PathBuf,
    pub before: String,
    pub after: String,
    /// What each rule that changed the file does, as the report names it.
    pub rules: Vec<String>,
}

/// An applied run, kept so `colab: undo` can put it back.
#[derive(Debug, Clone)]
pub(super) struct Applied {
    pub label: String,
    pub files: Vec<Planned>,
}

/// The line hunks that turn `before` into `after`, as edits of `before`.
pub(super) fn hunks(before: &str, after: &str) -> Vec<TextEdit> {
    let old: Vec<&str> = before.split_inclusive('\n').collect();
    let new: Vec<&str> = after.split_inclusive('\n').collect();
    let mut starts = Vec::with_capacity(old.len() + 1);
    let mut at = 0;
    starts.push(0);
    for line in &old {
        at += line.len();
        starts.push(at);
    }
    TextDiff::from_slices(&old, &new)
        .ops()
        .iter()
        .filter(|op| !matches!(op, DiffOp::Equal { .. }))
        .map(|op| {
            let lines = op.old_range();
            let range = Range::new(
                position_at(before, starts[lines.start]),
                position_at(before, starts[lines.end]),
            );
            TextEdit::new(range, new[op.new_range()].concat())
        })
        .collect()
}

/// The label a file's hunks are annotated with: the rules that changed it.
fn annotation_label(file: &Planned) -> String {
    if file.rules.is_empty() {
        "colab".to_string()
    } else {
        file.rules.join("; ")
    }
}

/// `files` as one workspace edit. A `preview` edit annotates each hunk
/// with the rules that made it, `needsConfirmation` set, so an editor
/// that supports change annotations shows its refactor preview, grouped
/// by rule, and applies only what the user keeps.
pub(super) fn to_edit(files: &[Planned], preview: bool) -> WorkspaceEdit {
    if !preview {
        let changes = files
            .iter()
            .map(|f| (f.uri.clone(), hunks(&f.before, &f.after)))
            .collect();
        return WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        };
    }
    let mut annotations = HashMap::new();
    let documents = files
        .iter()
        .map(|file| {
            let label = annotation_label(file);
            annotations
                .entry(label.clone())
                .or_insert_with(|| ChangeAnnotation {
                    label: label.clone(),
                    needs_confirmation: Some(true),
                    description: Some(format!("changed by colab: {label}")),
                });
            TextDocumentEdit {
                text_document: OptionalVersionedTextDocumentIdentifier {
                    uri: file.uri.clone(),
                    version: None,
                },
                edits: hunks(&file.before, &file.after)
                    .into_iter()
                    .map(|text_edit| {
                        OneOf::Right(AnnotatedTextEdit {
                            text_edit,
                            annotation_id: label.clone(),
                        })
                    })
                    .collect(),
            }
        })
        .collect();
    WorkspaceEdit {
        changes: None,
        document_changes: Some(DocumentChanges::Edits(documents)),
        change_annotations: Some(annotations),
    }
}

/// The edit that puts each file of `applied` back, given its text now
/// (`None`: gone). Edits made since the apply survive when they do not
/// overlap the run's: the undo is a three-way merge of the file now with
/// the file before the run, based on the file as the run left it. Files
/// it cannot put back are returned, untouched.
pub(super) fn undo_edit(
    applied: &Applied,
    now: impl Fn(&Planned) -> Option<String>,
) -> (WorkspaceEdit, Vec<PathBuf>) {
    let mut changes = HashMap::new();
    let mut skipped = Vec::new();
    for file in &applied.files {
        let Some(current) = now(file) else {
            skipped.push(file.path.clone());
            continue;
        };
        match three_way(&file.after, &file.before, &current) {
            Merge::Clean(text) => {
                if text != current {
                    changes.insert(file.uri.clone(), hunks(&current, &text));
                }
            }
            Merge::Conflicted(_) => skipped.push(file.path.clone()),
        }
    }
    (
        WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        },
        skipped,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp_server::ls_types::Position;

    fn planned(before: &str, after: &str) -> Planned {
        Planned {
            uri: "file:///w/a.go".parse().unwrap(),
            path: PathBuf::from("/w/a.go"),
            before: before.into(),
            after: after.into(),
            rules: vec!["go::import \"a\" -> \"b\"".into()],
        }
    }

    #[test]
    fn hunks_touch_only_the_changed_lines() {
        let edits = hunks(
            "package p\n\nimport \"a\"\n\nfunc f() {}\n",
            "package p\n\nimport \"b\"\n\nfunc f() {}\n",
        );
        assert_eq!(
            edits,
            [TextEdit::new(
                Range::new(Position::new(2, 0), Position::new(3, 0)),
                "import \"b\"\n".into()
            )]
        );
    }

    #[test]
    fn a_preview_annotates_each_hunk_with_its_rule_and_asks_first() {
        let edit = to_edit(&[planned("import \"a\"\n", "import \"b\"\n")], true);
        let annotations = edit.change_annotations.unwrap();
        let note = &annotations["go::import \"a\" -> \"b\""];
        assert_eq!(note.needs_confirmation, Some(true));
        let Some(DocumentChanges::Edits(docs)) = edit.document_changes else {
            panic!("a preview is document changes");
        };
        assert!(matches!(&docs[0].edits[0], OneOf::Right(e) if e.annotation_id == note.label));
    }

    #[test]
    fn undo_keeps_later_edits_that_do_not_overlap_and_skips_ones_that_do() {
        let applied = Applied {
            label: "r".into(),
            files: vec![planned(
                "import \"a\"\n\nfunc f() {}\n",
                "import \"b\"\n\nfunc f() {}\n",
            )],
        };
        let (edit, skipped) = undo_edit(&applied, |_| {
            Some("import \"b\"\n\nfunc f() { g() }\n".into())
        });
        assert!(skipped.is_empty());
        let changes = edit.changes.unwrap();
        assert_eq!(
            changes.values().next().unwrap()[0].new_text,
            "import \"a\"\n"
        );
        let (_, skipped) = undo_edit(&applied, |_| Some("import \"c\"\n\nfunc f() {}\n".into()));
        assert_eq!(skipped, [PathBuf::from("/w/a.go")]);
    }
}
