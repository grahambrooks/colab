//! The plug-in surface that language backends implement.
//!
//! [`LanguageBackend`] is owned by each `colab-lang-*` crate. The DSL
//! crate lowers a parsed `match <lang>::<module> "..." { <action> }`
//! block into a [`RuleSpec`] and asks the registered backend for an
//! [`Operation`] — a ready-to-run transform.
//!
//! Backends are registered in [`BackendRegistry`] by the binary
//! (typically [`colab-cli`]); the DSL crate has no compile-time
//! knowledge of which backends exist.

use std::fmt;
use std::path::Path;

use crate::error::Result;

/// A single ready-to-run transformation produced by a [`LanguageBackend`].
///
/// Implementations MUST return `source_code` unchanged when there is
/// nothing to rewrite (the walker uses string equality with the input
/// to skip writes, and rule composition relies on irrelevant rules
/// being identity).
pub trait Operation: fmt::Debug + fmt::Display + Send + Sync {
    /// Returns `true` if this operation wants to operate on `path`.
    fn is_file_relevant(&self, path: &Path) -> bool;

    /// Apply the transformation to `source_code`, returning the new
    /// contents.
    fn apply(&self, source_code: &str) -> String;
}

/// The lowered form of a single `match` block: action plus arguments,
/// kept backend-neutral so [`colab-dsl`] does not need to know which
/// language is targeted.
#[derive(Debug, Clone)]
pub enum RuleSpec {
    /// `replace` action: rewrite `target` with `replacement` in the
    /// scope of the namespace's module.
    Replace { target: String, replacement: String },
    /// `delete` action: remove the matched element entirely.
    Delete { target: String },
    /// `ensure` action: idempotently add `target` if it is missing.
    Ensure { target: String },
    /// `replace_call` action: rewrite a call expression whose
    /// function name equals `target` using `template`. See
    /// `colab_dsl::ast::Action::ReplaceCall` for the template
    /// placeholder list.
    ReplaceCall { target: String, template: String },
}

/// One module-level capability advertised by a [`LanguageBackend`].
///
/// `name` is the value that appears after `<lang>::` in the DSL, e.g.
/// `"import"` for `go::import`. This data drives `colab schema`,
/// `colab list-rules`, and the LSP completion surface.
pub struct Capability {
    pub module: &'static str,
    pub description: &'static str,
    pub actions: &'static [ActionCapability],
}

/// One DSL action supported within a [`Capability`] (e.g. `replace`).
pub struct ActionCapability {
    pub name: &'static str,
    pub description: &'static str,
}

/// Plug-in implemented by each `colab-lang-*` crate.
pub trait LanguageBackend: Send + Sync {
    /// The DSL namespace prefix this backend owns ("go", "rust", ...).
    fn lang(&self) -> &'static str;

    /// Human-readable description of the backend (one short sentence).
    fn description(&self) -> &'static str {
        ""
    }

    /// Static description of every module + action this backend
    /// supports. Used by discovery commands and the LSP.
    fn capabilities(&self) -> &'static [Capability];

    /// Lower a single match block into a concrete [`Operation`].
    ///
    /// Returns [`crate::Error::UnsupportedOperation`] for unknown
    /// modules or action/module combinations the backend does not
    /// implement.
    fn build_rule(&self, module: &str, spec: RuleSpec) -> Result<Box<dyn Operation>>;
}

/// Lookup table mapping language names to their backends. The binary
/// constructs one of these and hands it to [`colab-dsl`] at compile
/// time; [`colab-dsl`] never imports backend crates directly.
#[derive(Default)]
pub struct BackendRegistry {
    backends: Vec<Box<dyn LanguageBackend>>,
}

impl BackendRegistry {
    /// Create an empty registry. Use [`register`](Self::register) to
    /// add backends.
    pub fn new() -> Self {
        Self {
            backends: Vec::new(),
        }
    }

    /// Add a backend. The first registered backend for a given
    /// `lang()` wins on lookup.
    pub fn register(&mut self, backend: Box<dyn LanguageBackend>) {
        self.backends.push(backend);
    }

    /// Find the backend that owns `lang`, or `None` if unregistered.
    pub fn get(&self, lang: &str) -> Option<&dyn LanguageBackend> {
        self.backends
            .iter()
            .find(|b| b.lang() == lang)
            .map(|b| b.as_ref())
    }

    /// Languages currently registered, in registration order.
    pub fn languages(&self) -> Vec<&'static str> {
        self.backends.iter().map(|b| b.lang()).collect()
    }
}
