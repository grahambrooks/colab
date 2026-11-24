//! Hover in `.colab` files: what a `<lang>::<module>` or an action does.

use super::*;

impl Backend {
    /// Markdown for the token at byte `offset`, if colab knows it.
    pub(crate) fn hover_at(&self, text: &str, offset: usize) -> Option<String> {
        let word = word_at(text, offset)?;
        if let Some((lang, module)) = word.split_once("::") {
            let backend = self.backends.get(lang)?;
            let cap = backend.capabilities().iter().find(|c| c.module == module)?;
            let mut out = format!("**`{lang}::{module}`** — {}\n\nActions:\n", cap.description);
            for action in cap.actions {
                let _ = writeln!(out, "- `{}` — {}", action.name, action.description);
            }
            return Some(out);
        }
        if STATIC_ACTIONS.iter().any(|(name, _)| *name == word) {
            let checks = colab_dsl::check(text, &self.backends).ok()?;
            let check = checks
                .iter()
                .find(|c| c.span.start <= offset && offset <= c.span.end)?;
            let (lang, module) = check.namespace.split_once("::")?;
            let cap = self
                .backends
                .get(lang)?
                .capabilities()
                .iter()
                .find(|c| c.module == module)?;
            return match cap.actions.iter().find(|a| a.name == word) {
                Some(action) => Some(format!(
                    "**`{word}`** on `{}` — {}",
                    check.namespace, action.description
                )),
                None => Some(format!(
                    "`{}` has no `{word}` action; it supports {}.",
                    check.namespace,
                    cap.actions
                        .iter()
                        .map(|a| format!("`{}`", a.name))
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            };
        }
        let keyword = match word {
            "refactor" => {
                "**`refactor \"<name>\" { … }`** — one named script; its `match` blocks run in order."
            }
            "match" => {
                "**`match <lang>::<module> \"<target>\" [in \"<glob>\"] { <action> }`** — one rule."
            }
            "in" => {
                "**`in \"<glob>\"`** — limit the rule to matching paths (a leading `./` is ignored)."
            }
            "include" => {
                "**`include \"<path>\"`** — splice another script's rules in here, resolved relative to this file."
            }
            "pattern" => {
                "**`pattern <name> = '<code>'`** — a pattern `where` clauses name: `inside <name>`, `has`, `follows`, `precedes`, `until`. Script-wide, shared through `include`."
            }
            "let" => {
                "**`let $NEW = $FROM | step | …`** — before a `<lang>::pattern` rule's `replace`: a value derived from a binding, for the template. Steps: `lower`, `upper`, `capitalize`, `snake`, `kebab`, `camel`, `pascal`, `upper_snake`, `strip_prefix(\"…\")`, `strip_suffix(\"…\")`, `replace(/re/, \"with\")`, `substring(start, end)`."
            }
            _ => return None,
        };
        Some(keyword.to_string())
    }
}

/// The `[A-Za-z0-9_:]` run around byte `offset`, if any.
pub(crate) fn word_at(text: &str, offset: usize) -> Option<&str> {
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == ':';
    let offset = offset.min(text.len());
    let start = text[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(offset, |(i, _)| i);
    let end = text[offset..]
        .char_indices()
        .find(|(_, c)| !is_word(*c))
        .map_or(text.len(), |(i, _)| offset + i);
    let word = text[start..end].trim_matches(':');
    (!word.is_empty()).then_some(word)
}
