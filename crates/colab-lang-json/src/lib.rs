//! JSON backend for colab: config-file edits rather than source rewrites.
//!
//! One module, `json::key`, addresses a member by JSON Pointer (RFC 6901,
//! `/enabledPlugins/name@marketplace`) and `set`s, `insert`s or `delete`s
//! it. Edits are made to the text in place, located with tree-sitter-json,
//! so key order, indentation and everything else in the file is kept.
//! `set` and `insert` create what is missing, so scope them with
//! `in "<glob>"` or run them on named files.

pub mod key;
pub mod pointer;

use std::cell::RefCell;

use colab_core::{ActionCapability, Capability, LanguageBackend, Operation, Result, RuleSpec};
use tree_sitter::{Parser, Tree};

thread_local! {
    static PARSER: RefCell<Parser> = RefCell::new({
        let mut p = Parser::new();
        p.set_language(&tree_sitter_json::LANGUAGE.into())
            .expect("failed to load tree-sitter JSON grammar");
        p
    });
}

pub(crate) fn parse(source: &str) -> Option<Tree> {
    PARSER.with_borrow_mut(|p| p.parse(source, None))
}

/// Plug-in entry point for the `json` namespace.
pub struct JsonBackend;

const CAPABILITIES: &[Capability] = &[Capability {
    module: "key",
    description: "Edit an object member addressed by JSON Pointer (`/a/b`, with `~1` for `/` and `~0` for `~`). Values are JSON, compared by meaning; the rest of the file keeps its text.",
    actions: &[
        ActionCapability {
            name: "delete",
            description: "Remove the member; a missing one is a no-op.",
        },
        ActionCapability {
            name: "set",
            description: "Give the member this value, creating it and missing parent objects, or overwriting a value that differs.",
        },
        ActionCapability {
            name: "insert",
            description: "Add the member with this value only when it is absent.",
        },
    ],
}];

impl LanguageBackend for JsonBackend {
    fn lang(&self) -> &'static str {
        "json"
    }

    fn description(&self) -> &'static str {
        "JSON config edits by JSON Pointer, in place via tree-sitter-json."
    }

    fn capabilities(&self) -> &'static [Capability] {
        CAPABILITIES
    }

    fn build_rule(&self, module: &str, spec: RuleSpec) -> Result<Box<dyn Operation>> {
        match (module, spec) {
            ("key", RuleSpec::Set { target, value }) => {
                Ok(Box::new(key::MemberEdit::set(&target, &value)?))
            }
            ("key", RuleSpec::Insert { target, value }) => {
                Ok(Box::new(key::MemberEdit::insert(&target, &value)?))
            }
            ("key", RuleSpec::Delete { target }) => Ok(Box::new(key::MemberEdit::delete(&target)?)),
            (other, spec) => Err(self.unsupported(other, &spec)),
        }
    }
}
