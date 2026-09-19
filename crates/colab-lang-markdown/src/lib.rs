//! Markdown backend for colab: managed blocks.
//!
//! A managed block is the text between two HTML-comment marker lines, which
//! Markdown renderers hide:
//!
//! ```text
//! <!-- tool:begin — anything after the name is kept -->
//! …managed text…
//! <!-- tool:end -->
//! ```
//!
//! `markdown::block "tool"` targets it by name. Everything outside the
//! markers belongs to the file's author and is never touched.

pub mod block;

use colab_core::{ActionCapability, Capability, LanguageBackend, Operation, Result, RuleSpec};

/// Plug-in entry point for the `markdown` namespace.
pub struct MarkdownBackend;

const CAPABILITIES: &[Capability] = &[Capability {
    module: "block",
    description: "Edit the managed block between `<!-- <name>:begin … -->` and `<!-- <name>:end -->` lines; text outside the markers is never touched. A file with the markers repeated or unbalanced is left alone.",
    actions: &[
        ActionCapability {
            name: "delete",
            description: "Remove the block and its markers.",
        },
        ActionCapability {
            name: "set",
            description: "Make the block's content exactly this text, appending the block at the end of the file when it is missing.",
        },
        ActionCapability {
            name: "insert",
            description: "Append the block with this text only when it is missing.",
        },
    ],
}];

impl LanguageBackend for MarkdownBackend {
    fn lang(&self) -> &'static str {
        "markdown"
    }

    fn description(&self) -> &'static str {
        "Markdown managed blocks between HTML-comment markers."
    }

    fn capabilities(&self) -> &'static [Capability] {
        CAPABILITIES
    }

    fn build_rule(&self, module: &str, spec: RuleSpec) -> Result<Box<dyn Operation>> {
        match (module, spec) {
            ("block", RuleSpec::Set { target, value }) => {
                Ok(Box::new(block::BlockEdit::set(&target, &value)?))
            }
            ("block", RuleSpec::Insert { target, value }) => {
                Ok(Box::new(block::BlockEdit::insert(&target, &value)?))
            }
            ("block", RuleSpec::Delete { target }) => {
                Ok(Box::new(block::BlockEdit::delete(&target)?))
            }
            (other, spec) => Err(self.unsupported(other, &spec)),
        }
    }
}
