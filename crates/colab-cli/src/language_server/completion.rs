//! Completion in `.colab` files: languages, modules, clauses and actions,
//! from the same registry `colab refactor` uses.

use super::*;

impl Backend {
    pub(crate) fn completions_for(&self, prefix: &str) -> Vec<CompletionItem> {
        let trimmed = prefix.trim_start();

        // `match … { <cursor>` — we're inside an action block.
        // Check this *before* the bare `match ` prefix because that
        // line also starts with `match `.
        if trimmed.starts_with("match ") && trimmed.contains('{') {
            return action_completions();
        }

        // After `match <ns> "<target>" ` → the clauses that come before
        // the braces: where the rule applies, and `where`.
        if let Some(after_match) = trimmed.strip_prefix("match ")
            && target_closed(after_match)
        {
            return CLAUSES
                .iter()
                .map(|(name, snippet, detail)| CompletionItem {
                    label: (*name).into(),
                    kind: Some(CompletionItemKind::KEYWORD),
                    insert_text: Some((*snippet).into()),
                    insert_text_format: Some(InsertTextFormat::SNIPPET),
                    detail: Some((*detail).into()),
                    ..Default::default()
                })
                .collect();
        }

        // After `match ` but no `{` yet → namespace / module.
        if let Some(after_match) = trimmed.strip_prefix("match ") {
            return self.namespace_or_module_completions(after_match.trim_start());
        }

        // Inside a `refactor … { <cursor>` block — suggest `match`.
        if trimmed.contains('{') {
            return vec![CompletionItem {
                label: "match".into(),
                kind: Some(CompletionItemKind::KEYWORD),
                insert_text: Some("match $1::$2 \"$3\" { $0 }".into()),
                insert_text_format: Some(InsertTextFormat::SNIPPET),
                detail: Some("Match block".into()),
                ..Default::default()
            }];
        }

        // Top-level: only `refactor` is meaningful.
        if "refactor".starts_with(trimmed) && !trimmed.is_empty() {
            return vec![CompletionItem {
                label: "refactor".into(),
                kind: Some(CompletionItemKind::KEYWORD),
                insert_text: Some("refactor \"$1\" {\n    $0\n}".into()),
                insert_text_format: Some(InsertTextFormat::SNIPPET),
                detail: Some("Top-level codemod block".into()),
                ..Default::default()
            }];
        }
        Vec::new()
    }

    /// Suggest a namespace (`<lang>`) when nothing follows yet, or
    /// the available modules for that backend once `<lang>::` is
    /// typed.
    pub(crate) fn namespace_or_module_completions(&self, fragment: &str) -> Vec<CompletionItem> {
        if let Some((lang, _module_prefix)) = fragment.split_once("::") {
            let Some(backend) = self.backends.get(lang) else {
                return Vec::new();
            };
            return backend
                .capabilities()
                .iter()
                .map(|cap| CompletionItem {
                    label: cap.module.into(),
                    kind: Some(CompletionItemKind::MODULE),
                    detail: Some(cap.description.into()),
                    ..Default::default()
                })
                .collect();
        }

        // No `::` typed yet — suggest a language. Each entry inserts
        // `<lang>::` so the next completion can offer modules.
        self.backends
            .languages()
            .iter()
            .map(|lang| CompletionItem {
                label: (*lang).into(),
                kind: Some(CompletionItemKind::MODULE),
                insert_text: Some(format!("{lang}::")),
                insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
                detail: Some("colab backend namespace".into()),
                ..Default::default()
            })
            .collect()
    }
}

/// The clauses between a rule's target and its braces.
pub(crate) const CLAUSES: &[(&str, &str, &str)] = &[
    ("in \"<glob>\"", "in \"$0\"", "only files matching the glob"),
    (
        "in fences",
        "in fences",
        "only inside Markdown code fences of this language",
    ),
    (
        "and fences",
        "and fences",
        "this language's files and its Markdown code fences",
    ),
    (
        "where",
        "where $0",
        "constraints on a pattern's metavariables",
    ),
];

/// `true` when `after_match` (the text after `match `) has a namespace,
/// a whole target string, and a space after it: the cursor is where the
/// clauses go.
pub(crate) fn target_closed(after_match: &str) -> bool {
    let Some((_namespace, rest)) = after_match.split_once(char::is_whitespace) else {
        return false;
    };
    let rest = rest.trim_start();
    let close = if let Some(raw) = rest.strip_prefix("r##\"") {
        raw.find("\"##").map(|i| 4 + i + 3)
    } else if let Some(raw) = rest.strip_prefix("r#\"") {
        raw.find("\"#").map(|i| 3 + i + 2)
    } else {
        rest.chars()
            .next()
            .filter(|q| *q == '"' || *q == '\'')
            .and_then(|q| rest[1..].find(q).map(|i| i + 2))
    };
    close.is_some_and(|end| rest[end..].starts_with(char::is_whitespace))
}

/// Import clauses, offered alongside the actions: they follow one inside
/// the braces.
pub(crate) const IMPORT_CLAUSES: &[(&str, &str)] = &[
    ("requires import", "requires import \"$0\""),
    ("drops import", "drops import \"$0\" when unused"),
    ("message", "message \"$0\""),
    ("severity", "severity ${1|error,warning,info|}"),
    ("let", "let \\$${1:NAME} = \\$${2:FROM} | ${3:snake}"),
];

pub(crate) fn action_completions() -> Vec<CompletionItem> {
    let item = |detail: &str, (name, snippet): &(&str, &str)| CompletionItem {
        label: (*name).into(),
        kind: Some(CompletionItemKind::KEYWORD),
        insert_text: Some((*snippet).into()),
        insert_text_format: Some(InsertTextFormat::SNIPPET),
        detail: Some(detail.into()),
        ..Default::default()
    };
    STATIC_ACTIONS
        .iter()
        .map(|a| item("DSL action", a))
        .chain(
            IMPORT_CLAUSES
                .iter()
                .map(|c| item("clause (`let` before the action, the rest after it)", c)),
        )
        .collect()
}

/// Return the slice of `text` from the start of the line up to
/// `position.character`. Used to drive context-aware completion.
pub(crate) fn line_prefix(text: &str, position: Position) -> &str {
    let line_idx = position.line as usize;
    let mut start = 0usize;
    for _ in 0..line_idx {
        match text[start..].find('\n') {
            Some(p) => start += p + 1,
            None => return "",
        }
    }
    let rest = &text[start..];
    let line_end = rest.find('\n').unwrap_or(rest.len());
    let line = &rest[..line_end];
    let char_offset = (position.character as usize).min(line.len());
    &line[..char_offset]
}
