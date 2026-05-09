//! Abstract syntax tree produced by the LALRPOP grammar.
//!
//! These types mirror the codemod DSL one-to-one and have no runtime
//! behaviour. They are lowered into the executable
//! [`crate::codemod::Refactoring`] IR by [`crate::codemod::compiler`].

/// A complete codemod script: `refactor "<name>" { <body> }`.
#[derive(PartialEq, Debug)]
pub struct Command {
    pub refactor_name: String,
    pub body: Body,
}

/// The body of a refactor: a single `match` block with one action.
#[derive(PartialEq, Debug)]
pub struct Body {
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
}
