//! Code-shaped structural patterns with metavariables.
//!
//! `errors.Wrap($ERR, $MSG)` matches any call of `errors.Wrap` with two
//! arguments, binding them to `$ERR` / `$MSG`, wherever it appears.
//! Matching is delegated to [`ast_grep_core`], which works on the same
//! tree-sitter version as every colab backend. Nothing outside this module
//! sees an ast-grep type, so a backend only supplies its grammar, an
//! expando character, and context wrappers ([`PatternLanguage`]).
//!
//! Metavariables follow ast-grep:
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
//! The module is laid out by stage: `compile.rs` (contexts, forms, the
//! qualified-call form, the shape a match must fit),
//! `check.rs` (`where` constraints and `let`s), `find.rs` (matching in a
//! file's own tree), `macros.rs` (matching inside macro calls' contents),
//! `rewrite.rs` (templates and layout), `delete.rs` (deleting a match
//! with what belongs to it), `parse.rs` and `tree.rs` (parsing helpers and
//! the printed tree), and `rule.rs` (the `Operation` every backend wraps).
use std::fmt::Write as _;

use std::borrow::Cow;

use ast_grep_core::language::Language;
use ast_grep_core::matcher::PatternBuilder;
use ast_grep_core::meta_var::MetaVariable;
use ast_grep_core::replacer::TemplateFix;
use ast_grep_core::tree_sitter::{LanguageExt, StrDoc, TSLanguage};
use ast_grep_core::{NodeMatch, Pattern, PatternError};
use colab_core::{Constraint, LanguageBackend, Operation, Relation, RuleSpec, Stop, Test, Var};
use tree_sitter::{Node, Parser};

use crate::{Edit, apply_edits, line_span};

mod compile;
use compile::*;
mod find;
mod macros;
use macros::*;
mod delete;
mod rewrite;
use rewrite::*;
mod check;
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

/// What a backend supplies to make `<lang>::pattern` work.
#[derive(Clone)]
pub struct PatternLanguage {
    /// The backend's tree-sitter grammar.
    pub grammar: tree_sitter::Language,
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
    /// Whether a struct or object literal can name a field by itself for
    /// a variable of the same name (Rust's `S { text }`, JS's `{ text }`):
    /// a pattern's `text: $T` then also matches `text`, with `$T` bound
    /// to `text`. `false` where that spelling means something else (Go's
    /// `T{a}` is positional, Python's `{a}` a set).
    pub shorthand_fields: bool,
}

/// An item's modifiers — visibility, `static`, annotations: Rust's `pub`,
/// Java's `@Override public static` — which a pattern for the item
/// usually leaves out. `fn $F($$$A) { $$$B }` then also matches
/// `pub(super) fn f() {}`, and `void $F() {}` matches `public static void
/// f() {}`.
///
/// An item pattern that spells no modifiers is compiled once more with
/// `sentinel` in front, which parses as the modifiers and becomes the
/// reserved metavariable [`MODIFIERS`], so one more form matches every
/// combination. That form refuses an item with none (the pattern as
/// written matches those), and a rewrite whose template is an item that
/// spells none gets the matched ones back. A pattern that spells one
/// (`pub fn …`, `static function …`) is matched as written.
#[derive(Clone, Copy, Debug)]
pub struct Modifiers {
    /// Text no code contains that parses where an item's modifiers go:
    /// Java's `@ColabMods`, Rust's `pub(in colab_mods)`.
    pub sentinel: &'static str,
    /// The node kinds an item's modifiers parse as, the sentinel's
    /// included: Java's `modifiers`; C#'s `modifier` and `attribute_list`.
    pub kinds: &'static [&'static str],
    /// Whether the modifiers are several sibling nodes (C#, PHP: one per
    /// keyword), captured as a `$$$` sequence, rather than one node that
    /// holds them all (Java's `modifiers`).
    pub several: bool,
}

/// The metavariable an item's modifiers are bound to in the form compiled
/// with [`Modifiers::sentinel`]. Reserved, like [`QUALIFIER`].
pub const MODIFIERS: &str = "COLAB_MODS";

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

/// ast-grep's view of a [`PatternLanguage`].
#[derive(Clone)]
struct TsLang {
    grammar: tree_sitter::Language,
    expando: char,
    /// What a deleted match takes with it (not used by ast-grep).
    leading: Leading,
    /// Macro calls a match may be inside (not used by ast-grep).
    macro_contents: Option<MacroContents>,
    /// How an item's modifiers parse: a pattern's [`Modifiers::sentinel`]
    /// is the metavariable [`MODIFIERS`].
    modifiers: Option<Modifiers>,
}

impl Language for TsLang {
    fn extract_meta_var(&self, source: &str) -> Option<MetaVariable> {
        if let Some(modifiers) = self.modifiers
            && source == modifiers.sentinel
        {
            return Some(if modifiers.several {
                MetaVariable::MultiCapture(MODIFIERS.to_string())
            } else {
                MetaVariable::Capture(MODIFIERS.to_string(), true)
            });
        }
        meta_variable(source, self.expando)
    }

    fn pre_process_pattern<'q>(&self, query: &'q str) -> Cow<'q, str> {
        if self.expando == '$' {
            Cow::Borrowed(query)
        } else {
            Cow::Owned(expand(query, self.expando))
        }
    }

    fn expando_char(&self) -> char {
        self.expando
    }

    fn kind_to_id(&self, kind: &str) -> u16 {
        self.grammar.id_for_node_kind(kind, true)
    }

    fn field_to_id(&self, field: &str) -> Option<u16> {
        self.grammar
            .field_id_for_name(field)
            .map(std::num::NonZero::get)
    }

    fn build_pattern(&self, builder: &PatternBuilder) -> Result<Pattern, PatternError> {
        builder.build(|src| StrDoc::try_new(src, self.clone()))
    }
}

impl LanguageExt for TsLang {
    fn get_ts_language(&self) -> TSLanguage {
        self.grammar.clone()
    }
}

/// ast-grep's own reading of a metavariable (`$A`, `$_`, `$$$`, `$$$A`,
/// `$$A`), which is crate-private there and so repeated here for
/// [`TsLang`]'s `extract_meta_var`, which adds the modifiers sentinel.
fn meta_variable(src: &str, expando: char) -> Option<MetaVariable> {
    let valid_first = |c: char| matches!(c, 'A'..='Z' | '_');
    let valid = |c: char| matches!(c, 'A'..='Z' | '_' | '0'..='9');
    let ellipsis: String = std::iter::repeat_n(expando, 3).collect();
    if src == ellipsis {
        return Some(MetaVariable::Multiple);
    }
    if let Some(name) = src.strip_prefix(&ellipsis) {
        if !name.starts_with(valid_first) || !name.chars().all(valid) {
            return None;
        }
        return Some(if name.starts_with('_') {
            MetaVariable::Multiple
        } else {
            MetaVariable::MultiCapture(name.to_owned())
        });
    }
    let rest = src.strip_prefix(expando)?;
    let (name, named) = match rest.strip_prefix(expando) {
        Some(name) => (name, false),
        None => (rest, true),
    };
    if !name.starts_with(valid_first) || !name.chars().all(valid) {
        return None;
    }
    Some(if name.starts_with('_') {
        MetaVariable::Dropped(named)
    } else {
        MetaVariable::Capture(name.to_owned(), named)
    })
}

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

/// A compiled structural pattern.
pub struct StructuralPattern {
    lang: TsLang,
    pattern: Pattern,
    text: String,
    prefilter: Option<String>,
    /// The context wrapper the pattern compiled in, if it needed one.
    context: Option<String>,
    /// The pattern's syntax tree as matched (see [`syntax_tree`]).
    tree: String,
    /// `where` constraints every match must satisfy.
    checks: Vec<Check>,
    /// The constraints as written, for display.
    constraints: Vec<Constraint>,
    /// `let` values derived from bindings for the template, in order.
    lets: Vec<Derived>,
    /// The same pattern compiled in the language's `also` contexts.
    also: Vec<StructuralPattern>,
    /// Set on the form compiled with [`QUALIFIER`] before a bare callee:
    /// a template's own bare call gets the matched path back.
    qualified: Option<Qualified>,
    /// Set on the form compiled with a visibility before an item pattern
    /// written without one: a template for the same item gets it back.
    visible: Option<compile::Visible>,
    /// Set on a form with struct fields written shorthand (`text` for
    /// `text: $T`): `(field, metavariable)` pairs, so a template's `$T`
    /// becomes `text` and `text: text` is written back as `text`.
    shorthand: Option<Vec<(String, String)>>,
    /// The pattern's named structure, to refuse a match whose node has
    /// children the pattern does not account for (see [`Shape`]).
    shape: Shape,
}

impl std::fmt::Debug for StructuralPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StructuralPattern")
            .field("text", &self.text)
            .field("prefilter", &self.prefilter)
            .field("constraints", &self.constraints)
            .finish()
    }
}

impl StructuralPattern {
    /// The `where` constraints, as written.
    pub fn constraints(&self) -> &[Constraint] {
        &self.constraints
    }

    /// The context wrapper the pattern needed, or `None` if it parsed on
    /// its own.
    pub fn context(&self) -> Option<&str> {
        self.context.as_deref()
    }

    /// The pattern's syntax tree, as it will be matched: named nodes,
    /// one per line, metavariables shown with `$`.
    pub fn tree(&self) -> &str {
        &self.tree
    }

    /// The pattern as written.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The longest literal (a named leaf that is not a metavariable) the
    /// pattern contains. Leaves are compared by exact text, so a file
    /// without this literal cannot match — a sound prefilter. `None` when
    /// the pattern is all metavariables and punctuation.
    pub fn prefilter(&self) -> Option<&str> {
        self.prefilter.as_deref()
    }
}
