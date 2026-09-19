//! TOML backend for colab: config-file edits rather than source rewrites.
//!
//! One module, `toml::key`, addresses a key or an array element by path
//! (see [`path`]) and `set`s, `insert`s or `delete`s it. Edits go through
//! `toml_edit`, which implements TOML 1.1, so comments and formatting in
//! the rest of the file are kept. `set` and `insert` create what is
//! missing, so scope them with `in "<glob>"` or run them on named files.

pub mod keys;
pub mod path;

use colab_core::{ActionCapability, Capability, LanguageBackend, Operation, Result, RuleSpec};

/// Plug-in entry point for the `toml` namespace.
pub struct TomlBackend;

const CAPABILITIES: &[Capability] = &[Capability {
    module: "key",
    description: "Edit a key or array element addressed by a dotted path; `name[field=value]` selects the element of an array (of tables or of inline tables) whose field equals value. Values are TOML, compared by meaning.",
    actions: &[
        ActionCapability {
            name: "delete",
            description: "Remove the key or element; a missing one is a no-op.",
        },
        ActionCapability {
            name: "set",
            description: "Give the key or element this value, creating it and missing parents, or overwriting a value that differs.",
        },
        ActionCapability {
            name: "insert",
            description: "Add the key or element with this value only when it is absent.",
        },
    ],
}];

impl LanguageBackend for TomlBackend {
    fn lang(&self) -> &'static str {
        "toml"
    }

    fn description(&self) -> &'static str {
        "TOML config edits by key path via toml_edit, preserving comments and formatting."
    }

    fn capabilities(&self) -> &'static [Capability] {
        CAPABILITIES
    }

    fn build_rule(&self, module: &str, spec: RuleSpec) -> Result<Box<dyn Operation>> {
        match (module, spec) {
            ("key", RuleSpec::Set { target, value }) => {
                Ok(Box::new(keys::KeyEdit::set(&target, &value)?))
            }
            ("key", RuleSpec::Insert { target, value }) => {
                Ok(Box::new(keys::KeyEdit::insert(&target, &value)?))
            }
            ("key", RuleSpec::Delete { target }) => Ok(Box::new(keys::KeyEdit::delete(&target)?)),
            (other, spec) => Err(self.unsupported(other, &spec)),
        }
    }
}
