//! Abstract syntax tree produced by the LALRPOP grammar.
//!
//! These types mirror the codemod DSL one-to-one and have no runtime
//! behaviour. They are lowered into the executable
//! [`crate::Refactoring`] IR by [`crate::compiler`].

/// A complete codemod script: `refactor "<name>" { <item>* }`.
///
/// Items are either match blocks or `include` directives, intermixed
/// in source order. The compiler expands includes recursively so the
/// runtime IR sees a flat list of match clauses.
#[derive(PartialEq, Debug)]
pub struct Command {
    pub refactor_name: String,
    pub items: Vec<Item>,
}

/// One element inside a `refactor "..." { … }` block.
#[derive(PartialEq, Debug)]
pub enum Item {
    Match(Match),
    Include(String),
}

/// One `match <namespace> "<target>" [in "<glob>"] [where …] { <action> <import clause>* }`
/// block.
/// A script may contain any number of these; they are applied in source
/// order.
#[derive(PartialEq, Debug)]
pub struct Match {
    pub namespace: Namespace,
    pub match_string: String,
    /// Optional path glob from an `in "<glob>"` clause. `None` means the
    /// rule applies wherever its language backend considers a file
    /// relevant.
    pub scope: Option<String>,
    /// Constraints from a `where` clause, in order. Empty without one.
    pub constraints: Vec<colab_core::Constraint>,
    pub action: Action,
    /// `requires import` / `drops import … when unused` clauses after the
    /// action, in order.
    pub imports: Vec<ImportClause>,
    /// Byte range of the whole block in the script, `match` to `}`.
    /// Lets the LSP place a diagnostic on the rule that caused it.
    pub span: Span,
}

/// An import clause on a rule, run only on the files the rule changed.
#[derive(PartialEq, Eq, Debug, Clone)]
pub enum ImportClause {
    /// `requires import "<x>"`: ensure the import is present.
    Requires(String),
    /// `drops import "<x>" when unused`: delete the import if nothing in
    /// the file still uses it.
    DropsUnused(String),
}

impl std::fmt::Display for ImportClause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportClause::Requires(x) => write!(f, "requires import \"{x}\""),
            ImportClause::DropsUnused(x) => write!(f, "drops import \"{x}\" when unused"),
        }
    }
}

/// A byte range in the script source.
#[derive(PartialEq, Eq, Debug, Clone, Copy, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

/// A target namespace such as `go::import`.
#[derive(PartialEq, Debug)]
pub struct Namespace {
    pub lang: String,
    pub module: String,
}

/// The action performed when a match succeeds.
#[derive(PartialEq, Debug)]
pub enum Action {
    /// Replace the matched value with the supplied string.
    Replace(String),
    /// Remove the matched element entirely.
    Delete,
    /// Idempotently add the matched element if it is missing. The
    /// `match_string` is the value to ensure exists; no other input
    /// is needed.
    Ensure,
    /// Rewrite a matched call expression using a string template.
    /// Placeholders in the template:
    ///
    /// - `$1`, `$2`, … — 1-indexed positional argument from the
    ///   matched call.
    /// - `$args` — the original argument list, comma-joined.
    /// - `$func` — the matched function name (verbatim).
    /// - `$$` — a literal `$`.
    ReplaceCall(String),
    /// Give the matched key or element this value, creating it (and any
    /// missing parents) when absent and overwriting it when different.
    /// Config-file backends only; the value is in the file's own syntax.
    Set(String),
    /// Add the matched key or element with this value only when it is
    /// absent; an existing value is left alone.
    Insert(String),
}

impl Action {
    /// The DSL keyword this action is written with.
    pub fn keyword(&self) -> &'static str {
        match self {
            Action::Replace(_) => "replace",
            Action::Delete => "delete",
            Action::Ensure => "ensure",
            Action::ReplaceCall(_) => "replace_call",
            Action::Set(_) => "set",
            Action::Insert(_) => "insert",
        }
    }
}
