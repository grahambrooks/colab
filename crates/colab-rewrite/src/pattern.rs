//! Code-shaped structural patterns with metavariables.
//!
//! `errors.Wrap($ERR, $MSG)` matches any call of `errors.Wrap` with two
//! arguments, binding them to `$ERR` / `$MSG`, wherever it appears.
//! Matching is colab's own (`colab_syntax::pattern`, driven by
//! [`crate::syntax`]); this module is what a backend supplies
//! ([`PatternLanguage`]: grammar, expando character, context wrappers and
//! the language's modifier, shorthand and macro rules), the
//! [`PatternRule`] every backend wraps, `colab ast`'s printed tree, and
//! the text helpers rewriting and deleting share.
//!
//! Metavariables:
//!
//! | Syntax     | Matches                                         |
//! | ---------- | ----------------------------------------------- |
//! | `$NAME`    | exactly one named node, bound to `NAME`          |
//! | `$$$NAME`  | zero or more sibling nodes, bound to `NAME`      |
//! | `$_`       | one node, not bound                              |
//!
//! A name used twice must bind identical text. Names are upper-case
//! letters, digits and `_`, starting with a letter or `_`.
//!
//! The module's files: `compile.rs` (the text transforms behind the
//! qualified-call and shorthand forms), `check.rs` (the `where` gate every
//! backend's `build_rule_where` shares), `rewrite.rs` (layout keeping and
//! template helpers), `delete.rs` (what a deleted match takes with it),
//! `parse.rs` and `tree.rs` (parsing helpers and the printed tree), and
//! `rule.rs` (the `Operation` every backend wraps).
use std::fmt::Write as _;

use colab_core::{Constraint, LanguageBackend, Operation, RuleSpec, Test, Var};

mod compile;
use compile::*;
// The text transforms both matchers' forms share (`crate::syntax`).
pub(crate) use compile::{qualify_call, root_path, shorthand_fields, shorthand_template};
pub(crate) use delete::{attached_start, own_start, with_separator, without_double_blank};
pub(crate) use rewrite::{
    indent_later_lines, keep_layout, keep_modifier_words, only_metavariables, same_but_whitespace,
    shift_lines, strip_blank_line_indent, tighten, transformable_template, unbound_in_template,
};
mod check;
mod delete;
mod rewrite;
pub use check::*;
mod rule;
pub use rule::*;
mod tree;
pub use tree::*;
mod parse;
use parse::*;
#[cfg(test)]
mod tests;

/// Placeholder a context wrapper uses for where the pattern goes, e.g.
/// `"package p\nfunc _() {\n\t@@PATTERN@@\n}"` for Go statements.
pub const PATTERN_SLOT: &str = "@@PATTERN@@";

/// What parses a language: a tree-sitter grammar, or a parser of colab's
/// own (step 3 of the move off tree-sitter, one language at a time).
/// Kinds, fields and the rest of a [`PatternLanguage`] are its trees'.
#[derive(Clone)]
pub enum Syntax {
    TreeSitter(tree_sitter::Language),
    Own(&'static dyn colab_syntax::SourceParser),
}

impl Syntax {
    /// The tree-sitter grammar, for what still reads tree-sitter's tree
    /// (a backend's own modules, the qualifier check); `None` for an own
    /// parser.
    pub fn tree_sitter(&self) -> Option<&tree_sitter::Language> {
        match self {
            Self::TreeSitter(grammar) => Some(grammar),
            Self::Own(_) => None,
        }
    }
}

/// What a backend supplies to make `<lang>::pattern` work.
#[derive(Clone)]
pub struct PatternLanguage {
    /// What parses the language.
    pub syntax: Syntax,
    /// The character `$` is rewritten to before the pattern is parsed,
    /// for grammars in which `$` cannot start an identifier (Go, Python,
    /// Rust, …). Use `'$'` where it can (JavaScript, Java).
    pub expando: char,
    /// Wrappers, each containing [`PATTERN_SLOT`] once, tried in order
    /// when the pattern does not parse cleanly on its own — so a Go
    /// expression, which is not a valid Go file, still parses.
    pub contexts: &'static [&'static str],
    /// Wrappers a pattern is compiled in *as well*, wherever it parses
    /// there as a different tree, and matched in all of them: Python's
    /// `Optional[$T]` is a `subscript` in an expression but a
    /// `generic_type` in an annotation, and a pattern should find both.
    pub also: &'static [&'static str],
    /// The language's path separator (`::` in Rust), when a call's
    /// callee can be written bare or qualified. A pattern that starts
    /// with a bare call, `to_svg($A)`, then also matches
    /// `svg::to_svg(x)` and `crate::svg::to_svg(x)`, and a template that
    /// starts with a bare call keeps the path it matched (see
    /// [`QUALIFIER`]). `None` leaves callees compared as written.
    pub qualified_calls: Option<&'static str>,
    /// What a deleted match takes with it from just before it: the doc
    /// comments and attributes that belong to the item (see [`Leading`]).
    pub leading: Leading,
    /// Macro calls whose contents a pattern reads too (see
    /// [`MacroContents`]); `None` where the grammar parses macro arguments
    /// as code already (C, C++) or the language has none.
    pub macro_contents: Option<MacroContents>,
    /// How an item's modifiers parse (see [`Modifiers`]): an item pattern
    /// written without them then matches the item with any, and a rewrite
    /// keeps them. `None` where the language has none a pattern would
    /// leave out.
    pub modifiers: Option<Modifiers>,
    /// How a struct or object literal names a field by itself for a
    /// variable of the same name (Rust's `S { text }`, JS's `{ text }`):
    /// a pattern's `text: $T` then also matches the
    /// shorthand, with `$T` bound to `text`. `None` where that spelling
    /// means something else (Go's `T{a}` is positional, Python's `{a}` a
    /// set) or the grammar cannot tell it apart (C#'s `new { A }` and
    /// `new { A = b }` parse as the same flat list of names).
    pub shorthand_fields: Option<Shorthand>,
    /// Modifier keywords the grammar keeps as anonymous tokens of the
    /// item (JavaScript's `static`, `async`, `get`). Matching skips
    /// anonymous tokens, so `f() {}` matches `static async f() {}`; a
    /// rewrite then puts back the ones leading the match that the pattern
    /// does not spell, so `f2() {}` gives `static async f2() {}`, not
    /// `f2() {}`. Empty where modifiers are named nodes.
    pub modifier_tokens: &'static [&'static str],
    /// A grammar that tells a method call from a call (rust-analyzer's
    /// `method_call_expr` beside `call_expr`) where the pattern's should
    /// not: `$F($$$A)` matches `x.m(a)` too, `$F` binding `x.m`, as when
    /// both were tree-sitter's `call_expression`. `None` where one kind is
    /// both.
    pub method_calls: Option<MethodCalls>,
}

/// The kinds of a call and of a method call, for
/// [`PatternLanguage::method_calls`].
#[derive(Clone, Copy, Debug)]
pub struct MethodCalls {
    pub call: &'static str,
    pub method_call: &'static str,
}

/// An item's modifiers — visibility, `static`, annotations: Rust's `pub`,
/// Java's `@Override public static` — which a pattern for the item
/// usually leaves out. `fn $F($$$A) { $$$B }` then also matches
/// `pub(super) fn f() {}`, and `void $F() {}` matches `public static void
/// f() {}`.
///
/// A pattern node that spells none of these kinds matches an item with any
/// of them leading it: the matcher takes them as given
/// (`colab_syntax::pattern::Rules::absorb`), and a rewrite whose template
/// is the same item spelling none gets them back. A pattern that spells
/// one (`pub fn …`, `static function …`) is matched as written, a spelled
/// modifiers node token for token.
#[derive(Clone, Copy, Debug)]
pub struct Modifiers {
    /// The node kinds an item's modifiers parse as: Java's `modifiers`;
    /// C#'s `modifier` and `attribute_list`; Rust's `visibility_modifier`.
    pub kinds: &'static [&'static str],
}

/// Macro calls whose arguments the grammar keeps as an unparsed token
/// tree (Rust's `assert!(…)`, `vec![…]`), which a pattern would otherwise
/// never see into.
///
/// The contents of each listed macro's token tree are put into `wrapper`
/// (at [`PATTERN_SLOT`]) and parsed. Only when that parses cleanly are
/// they matched, rewritten and deleted in, and the edits mapped back; a
/// macro whose contents are not expressions (a `matches!` pattern, a DSL)
/// is never listed, so it is never guessed at. Nested macros are read
/// the same way, from the parsed contents. A project adds its own
/// expression macros through the run ([`colab_core::run::Run::set_macros`],
/// `colab.toml` `[patterns] macros`).
///
/// Constraints judge a match there as one in the file: its ancestors are
/// the contents' own nodes, then the macro call and the file's nodes
/// around it (`inside`, `directly inside 'assert!($$$A)'`), never the
/// wrapper; its neighbours and position are among the contents' own
/// (`follows`, `nth`: a macro's arguments are the wrapper call's, in no
/// `field`); and a language server is asked about the file, at the
/// position the contents came from (`type`, `resolves`, …).
#[derive(Clone, Copy, Debug)]
pub struct MacroContents {
    /// The macro-call node kind: Rust's `macro_invocation`.
    pub invocation: &'static str,
    /// The field naming the macro (`macro`), whose last path segment is
    /// compared with `macros`.
    pub name_field: &'static str,
    /// The token-tree node kind, the call's last child: `token_tree`.
    pub body: &'static str,
    /// The macros whose arguments are expressions, by name.
    pub macros: &'static [&'static str],
    /// Where the contents parse as call arguments, at [`PATTERN_SLOT`].
    pub wrapper: &'static str,
}

/// A language's field shorthand ([`PatternLanguage::shorthand_fields`]):
/// `name<separator> $V` written `name<kept>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shorthand {
    /// What the shorthand keeps of `name: value` after the name: nothing
    /// (`text`) in Rust and JS.
    pub kept: &'static str,
    /// The brackets a field list opens and closes with (`{` `}`).
    pub open: &'static [char],
    pub close: &'static [char],
}

impl Shorthand {
    /// `{ name: value }` → `{ name }`: Rust's struct literals, JS's objects.
    pub const BRACES: Self = Self {
        kept: "",
        open: &['{'],
        close: &['}'],
    };
}

/// The nodes just before an item that belong to it, so deleting the item
/// deletes them too instead of leaving a doc comment documenting nothing.
///
/// A node counts when it is the item's previous sibling (or that one's,
/// and so on), with no blank line between them, and is either one of
/// `kinds` or a comment that starts with one of `doc_comments`. A plain
/// comment is never taken: it may be about the code around it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Leading {
    /// Node kinds that belong to the item after them: Rust's
    /// `attribute_item`, Python's `decorator`.
    pub kinds: &'static [&'static str],
    /// Prefixes that make a comment a doc comment: `///`, `/**`.
    pub doc_comments: &'static [&'static str],
}

impl Leading {
    /// Nothing before an item belongs to it.
    pub const NONE: Self = Self {
        kinds: &[],
        doc_comments: &[],
    };
}

/// The metavariable a qualified-call form binds the matched path to.
/// A user pattern cannot bind it by accident: it is reserved.
pub const QUALIFIER: &str = "$COLAB_PATH";

/// One place a pattern matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub start: usize,
    pub end: usize,
    /// 1-based line and column (in characters) of `start`.
    pub line: usize,
    pub column: usize,
    pub text: String,
    /// Metavariable bindings, sorted by name. A `$$$` capture is the
    /// source text spanning all of its nodes.
    pub captures: Vec<(String, String)>,
}
