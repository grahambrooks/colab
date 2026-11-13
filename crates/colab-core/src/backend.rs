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

    /// A literal that MUST appear in the source for this operation to
    /// change anything — a cheap necessary condition checked with
    /// `str::contains` before the expensive tree-sitter parse.
    ///
    /// This is a filter, not a matcher: returning a literal only promises
    /// that its *absence* implies a no-op. The parse still does the real
    /// matching, so an over-broad literal costs performance, never
    /// correctness.
    ///
    /// Return `None` to disable the fast path. `ensure`-style operations
    /// MUST return `None`, since they act precisely when the target is
    /// absent; so must `set` and `insert`.
    fn prefilter(&self) -> Option<&str> {
        None
    }

    /// The value this rule looks for, when it looks for one — an import
    /// path, a function name. Paired with
    /// [`observed_targets`](Operation::observed_targets) to explain a rule
    /// that matched nothing ("no import `a/b`; saw `a/b/v2`").
    ///
    /// Return `None` (the default) to opt out of near-miss reporting.
    fn near_miss_target(&self) -> Option<&str> {
        None
    }

    /// Every value this rule's module recognises in `source`, whether or
    /// not it equals the target: all import paths, all called function
    /// names. Only called for a rule that matched nothing, on a bounded
    /// sample of files, so it may parse.
    fn observed_targets(&self, _source: &str) -> Vec<String> {
        Vec::new()
    }

    /// Exactly where this rule matches in `source`, with any metavariable
    /// bindings — for read-only search (`colab find`). `None` (the
    /// default) means "locate me by the lines my rewrite changes", which
    /// [`crate::find::locate`] does generically.
    fn locate(&self, _source: &str) -> Option<Vec<crate::find::Located>> {
        None
    }
}

/// The rejection for a `where` clause on a module that takes none.
pub fn where_unsupported(lang: &str, module: &str) -> crate::Error {
    crate::Error::UnsupportedOperation(format!(
        "`{lang}::{module}` does not take a `where` clause; only `{lang}::pattern` does"
    ))
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
    /// `set` action: give `target` the value `value`, creating or
    /// overwriting it. The value is in the target file's own syntax.
    Set { target: String, value: String },
    /// `insert` action: add `target` with `value` only when it is absent.
    Insert { target: String, value: String },
}

impl RuleSpec {
    /// The DSL keyword this spec came from. Error messages name the
    /// action rather than printing the `Debug` form of the whole spec,
    /// which leaks Rust internals and buries the useful part.
    pub fn action_name(&self) -> &'static str {
        match self {
            RuleSpec::Replace { .. } => "replace",
            RuleSpec::Delete { .. } => "delete",
            RuleSpec::Ensure { .. } => "ensure",
            RuleSpec::ReplaceCall { .. } => "replace_call",
            RuleSpec::Set { .. } => "set",
            RuleSpec::Insert { .. } => "insert",
        }
    }

    /// The match string this spec targets.
    pub fn target(&self) -> &str {
        match self {
            RuleSpec::Replace { target, .. }
            | RuleSpec::Delete { target }
            | RuleSpec::Ensure { target }
            | RuleSpec::ReplaceCall { target, .. }
            | RuleSpec::Set { target, .. }
            | RuleSpec::Insert { target, .. } => target,
        }
    }
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
    /// implement. Build that error with
    /// [`unsupported`](LanguageBackend::unsupported) so every backend
    /// rejects names the same way.
    fn build_rule(&self, module: &str, spec: RuleSpec) -> Result<Box<dyn Operation>>;

    /// [`build_rule`](LanguageBackend::build_rule) for a block with a
    /// `where` clause. Only `<lang>::pattern` takes constraints. Source
    /// backends override this to compile them through
    /// `colab_rewrite::pattern::build_rule_where`, and every other backend
    /// keeps this default, which rejects them by name.
    fn build_rule_where(
        &self,
        module: &str,
        spec: RuleSpec,
        constraints: &[crate::Constraint],
    ) -> Result<Box<dyn Operation>> {
        if constraints.is_empty() {
            return self.build_rule(module, spec);
        }
        Err(where_unsupported(self.lang(), module))
    }

    /// The module that `requires import "<x>"` lowers to an `ensure` on:
    /// `import` for Go, `use` for Rust, `include` for C. `None` (the
    /// default) rejects the clause.
    fn import_module(&self) -> Option<&'static str> {
        None
    }

    /// The operation behind `drops import "<target>" when unused`: delete
    /// the import, but only when nothing in the file still uses it. A
    /// backend that cannot tell whether an import is used keeps this
    /// default, which rejects the clause rather than guessing.
    fn drop_unused_import(&self, target: &str) -> Result<Box<dyn Operation>> {
        Err(crate::Error::UnsupportedOperation(format!(
            "`{}` cannot tell whether an import is still used, so `drops import \"{}\" when unused` is not supported; delete it with a separate rule after checking",
            self.lang(),
            target
        )))
    }

    /// Debugging aid behind `colab ast`: the printed syntax tree of a
    /// snippet ([`AstInput::Source`]) or of a compiled `pattern`
    /// ([`AstInput::Pattern`]). Backends without a source grammar keep the
    /// default, which says so.
    fn syntax_tree(&self, _input: AstInput<'_>) -> Result<SyntaxTree> {
        Err(crate::Error::UnsupportedOperation(format!(
            "`{}` has no syntax tree to show: `colab ast` works for source-language backends",
            self.lang()
        )))
    }

    /// The standard rejection for a `build_rule` catch-all arm.
    ///
    /// Distinguishes the two failures a single catch-all otherwise
    /// conflates — a misspelled *module*, and a valid module asked for an
    /// *action* it does not implement — and lists the valid names for
    /// whichever it was.
    fn unsupported(&self, module: &str, spec: &RuleSpec) -> crate::Error {
        match self.capabilities().iter().find(|c| c.module == module) {
            None => {
                let modules: Vec<&str> = self.capabilities().iter().map(|c| c.module).collect();
                crate::Error::UnsupportedOperation(format!(
                    "unknown module `{}::{}`; {}",
                    self.lang(),
                    module,
                    crate::suggest::candidates_note(module, "modules", &modules)
                ))
            }
            Some(cap) => {
                let actions: Vec<&str> = cap.actions.iter().map(|a| a.name).collect();
                crate::Error::UnsupportedOperation(format!(
                    "`{}::{}` does not support the `{}` action; {}",
                    self.lang(),
                    module,
                    spec.action_name(),
                    crate::suggest::candidates_note(spec.action_name(), "actions", &actions)
                ))
            }
        }
    }
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

/// What `colab ast` is asked to print.
#[derive(Debug, Clone, Copy)]
pub enum AstInput<'a> {
    /// A snippet of source code.
    Source(&'a str),
    /// A `<lang>::pattern` target, compiled exactly as a rule would be.
    Pattern(&'a str),
}

/// A printed syntax tree, kept as text so `colab-core` stays free of any
/// grammar engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxTree {
    /// One named node per line, indented, with positions and leaf text.
    pub tree: String,
    /// `true` when the snippet did not parse cleanly.
    pub has_error: bool,
    /// For a pattern: the context wrapper it needed, if any.
    pub context: Option<String>,
    /// For a pattern: its prefilter literal, if any.
    pub prefilter: Option<String>,
}
