//! Language-blind text backend for colab.
//!
//! Every other backend reads a file through a grammar. This one reads it
//! as text, for the change no grammar can say: "this string, in every
//! file, whatever the language": an extension renamed in Rust strings,
//! Markdown, TOML, shell scripts and comments alike.
//!
//! Two modules, both taking `replace` and `delete`:
//!
//! - `text::literal "<s>"` matches every occurrence of `s`, exactly.
//! - `text::word "<s>"` matches only occurrences that are not part of a
//!   longer word: where `s` starts (ends) with a letter, digit or `_`, the
//!   character before (after) it must not be one. `.codemod` then matches
//!   in `a.codemod` but not in `a.codemods`.
//!
//! The rules run on every file the walk reaches, apart from common binary
//! formats, so scope them with `in "<glob>"` or by the paths given.

pub mod literal;

use colab_core::{ActionCapability, Capability, LanguageBackend, Operation, Result, RuleSpec};

use literal::{Literal, Mode};

/// Plug-in entry point for the `text` namespace.
pub struct TextBackend;

const ACTIONS: &[ActionCapability] = &[
    ActionCapability {
        name: "replace",
        description: "Replace each occurrence with this text, verbatim.",
    },
    ActionCapability {
        name: "delete",
        description: "Remove each occurrence (the text only, not its line).",
    },
];

const CAPABILITIES: &[Capability] = &[
    Capability {
        module: "literal",
        description: "Every occurrence of a literal string, compared exactly (case and whitespace included), in any text file.",
        actions: ACTIONS,
    },
    Capability {
        module: "word",
        description: "Occurrences of a literal string that are not part of a longer word (letters, digits, `_`), in any text file.",
        actions: ACTIONS,
    },
];

impl LanguageBackend for TextBackend {
    fn lang(&self) -> &'static str {
        "text"
    }

    fn description(&self) -> &'static str {
        "Language-blind text edits: a literal string or whole word, in any text file."
    }

    fn capabilities(&self) -> &'static [Capability] {
        CAPABILITIES
    }

    fn build_rule(&self, module: &str, spec: RuleSpec) -> Result<Box<dyn Operation>> {
        let mode = match module {
            "literal" => Mode::Literal,
            "word" => Mode::Word,
            other => return Err(self.unsupported(other, &spec)),
        };
        match spec {
            RuleSpec::Replace {
                target,
                replacement,
            } => Ok(Box::new(Literal::new(mode, target, Some(replacement))?)),
            RuleSpec::Delete { target } => Ok(Box::new(Literal::new(mode, target, None)?)),
            spec => Err(self.unsupported(module, &spec)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_modules_and_actions_name_the_alternatives() {
        let err = TextBackend
            .build_rule("literl", RuleSpec::Delete { target: "x".into() })
            .unwrap_err()
            .to_string();
        assert!(err.contains("literal"), "{err}");
        let err = TextBackend
            .build_rule("literal", RuleSpec::Ensure { target: "x".into() })
            .unwrap_err()
            .to_string();
        assert!(err.contains("replace"), "{err}");
    }
}
