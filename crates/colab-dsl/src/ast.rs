//! Abstract syntax tree produced by the LALRPOP grammar.
//!
//! These types mirror the codemod DSL one-to-one and have no runtime
//! behaviour. They are lowered into the executable
//! [`crate::Refactoring`] IR by [`crate::compiler`].

/// A complete codemod script: `refactor "<name>" { <match>* }`.
#[derive(PartialEq, Debug)]
pub struct Command {
    pub refactor_name: String,
    pub matches: Vec<Match>,
}

/// One `match <namespace> "<target>" { <action> }` block. A script
/// may contain any number of these; they are applied in source order.
#[derive(PartialEq, Debug)]
pub struct Match {
    pub namespace: Namespace,
    pub match_string: String,
    pub action: Action,
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
}
