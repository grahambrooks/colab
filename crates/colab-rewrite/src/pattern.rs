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

/// Placeholder a context wrapper uses for where the pattern goes, e.g.
/// `"package p\nfunc _() {\n\t@@PATTERN@@\n}"` for Go statements.
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
}

impl Language for TsLang {
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

/// Replace each metavariable `$` with `expando`, leaving every other `$`
/// (a PHP variable, a `$` inside a string) alone.
fn expand(query: &str, expando: char) -> String {
    let chars: Vec<char> = query.chars().collect();
    let mut out = String::with_capacity(query.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' {
            // `$$$NAME`, `$NAME`, `$_`: a run of `$` followed by a name start.
            let run = chars[i..].iter().take_while(|&&c| c == '$').count();
            // A bare `$$$` is an anonymous sequence: ast-grep reads
            // `µµµ` as one, and it is an identifier where `$$$` is not.
            let named = chars.get(i + run).is_some_and(|&c| is_var_start(c));
            if (run <= 3 && named) || (run == 3 && !named) {
                out.extend(std::iter::repeat_n(expando, run));
                i += run;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// `$X` or `$_`: one single-node metavariable and nothing else.
fn is_lone_metavariable(text: &str) -> bool {
    let Some(name) = text.strip_prefix('$') else {
        return false;
    };
    !name.is_empty()
        && !name.starts_with('$')
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

fn is_var_start(c: char) -> bool {
    c.is_ascii_uppercase() || c == '_'
}

/// Words that can stand before `(` without being a callee.
const NOT_CALLEES: &[&str] = &[
    "as", "async", "await", "box", "break", "const", "crate", "dyn", "else", "fn", "for", "if",
    "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self",
    "Self", "static", "super", "type", "unsafe", "use", "where", "while", "yield",
];

/// When `text` starts with a bare call (`f(`, or `f::<T>(` with `sep` =
/// `::`), the same text with [`QUALIFIER`] and `sep` before it, and the
/// callee's name. `None` for anything else: a metavariable callee, a
/// path that is already qualified, a macro, a keyword.
fn qualify_call<'t>(text: &'t str, sep: &str) -> Option<(String, &'t str)> {
    let body = text.trim_start();
    let lead = &text[..text.len() - body.len()];
    let end = body
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(body.len());
    let callee = &body[..end];
    let first = callee.chars().next()?;
    if !(first.is_alphabetic() || first == '_') || callee == "_" || NOT_CALLEES.contains(&callee) {
        return None;
    }
    let rest = body[end..].trim_start();
    let turbofish = rest
        .strip_prefix(sep)
        .is_some_and(|r| r.trim_start().starts_with('<'));
    (rest.starts_with('(') || turbofish).then(|| (format!("{lead}{QUALIFIER}{sep}{body}"), callee))
}

/// What the qualified form of a bare-call pattern needs to rewrite.
#[derive(Debug, Clone)]
struct Qualified {
    separator: &'static str,
    /// The pattern's callee, `to_svg` in `to_svg($A)`.
    callee: String,
}

impl Qualified {
    /// The template to fill for a qualified match: the matched path goes
    /// before a template's own bare callee when it is the same function
    /// (`to_svg($A, opts)`) or a rename of it (`render($A)`), so
    /// `svg::to_svg(x)` becomes `svg::render(x)`. A template that wraps
    /// the call (`Ok(to_svg($A))`) or is not a call (`$A.to_svg()`) is
    /// used as written.
    fn template(&self, template: &str) -> Option<String> {
        let (qualified, callee) = qualify_call(template, self.separator)?;
        let renamed = !template.contains(self.callee.as_str());
        (callee == self.callee || renamed).then_some(qualified)
    }
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
    /// The pattern's named structure, to refuse a match whose node has
    /// children the pattern does not account for (see [`Shape`]).
    shape: Shape,
}

/// The named structure of a compiled pattern.
///
/// ast-grep's default strictness ignores a candidate's *trailing*
/// children once the pattern's run out, so `new C<T>()` matches
/// `new C<T>() { … }` and `if $C { $$$B }` matches an `if` with an
/// `else`. A rewrite or delete of such a match replaces the whole node
/// and silently drops what the pattern never mentioned. A match is kept
/// only when every node it pairs with the pattern has no more named
/// children than the pattern gives it, unless a `$$$` there absorbs them.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Shape {
    /// A metavariable or a literal leaf: anything ast-grep accepted.
    Any,
    /// A `$$$` sequence.
    Many,
    /// An inner node and its named, non-comment children.
    Node(Vec<Shape>),
}

impl Shape {
    fn of(node: Node<'_>, source: &str, expando: char) -> Self {
        let text = node.utf8_text(source.as_bytes()).unwrap_or("").trim();
        if let Some(rest) = text.strip_prefix(expando) {
            let many = rest.starts_with(expando);
            let name = rest.trim_start_matches(expando);
            if name.chars().all(|c| is_var_start(c) || c.is_ascii_digit()) {
                return if many { Self::Many } else { Self::Any };
            }
        }
        if node.child_count() == 0 {
            return Self::Any;
        }
        let mut cursor = node.walk();
        // An error node holding only a metavariable (a lone `$$$B` in an
        // item list, see `parse_pattern`) is the placeholder itself, though
        // tree-sitter marks error nodes as extras.
        Self::Node(
            node.children(&mut cursor)
                .filter(|c| !c.is_missing() && (c.is_error() || (c.is_named() && !c.is_extra())))
                .map(|c| Self::of(c, source, expando))
                .collect(),
        )
    }

    /// `true` unless `candidate`, which ast-grep matched against this
    /// shape, has named children the pattern leaves out.
    fn fits<D: ast_grep_core::Doc>(&self, candidate: &ast_grep_core::Node<'_, D>) -> bool {
        let Self::Node(goals) = self else {
            return true;
        };
        if goals.contains(&Self::Many) {
            return true;
        }
        let named: Vec<_> = candidate
            .children()
            .filter(|c| c.is_named() && !c.is_extra())
            .collect();
        if named.len() != goals.len() {
            return named.len() < goals.len();
        }
        goals.iter().zip(&named).all(|(g, c)| g.fits(c))
    }
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
    /// Compile `text` for `language`.
    ///
    /// The pattern is parsed on its own first; if that leaves syntax
    /// errors, inside each of the language's contexts in turn. The error
    /// names every attempt, so a pattern that is not valid code in this
    /// language says so rather than silently matching nothing.
    pub fn compile(language: &PatternLanguage, text: &str) -> Result<Self, String> {
        let alt_in = |context: &'static &'static str| {
            let only = PatternLanguage {
                contexts: std::slice::from_ref(context),
                also: &[],
                ..language.clone()
            };
            Self::compile_one(&only, text, false)
        };
        // Text that is code only in an `also` context (a Python keyword
        // argument, a dict entry) compiles there.
        let mut primary = match Self::compile_one(language, text, true) {
            Ok(primary) => primary,
            Err(e) => language
                .also
                .iter()
                .find_map(|context| alt_in(context).ok())
                .ok_or(e)?,
        };
        for context in language.also {
            if let Ok(alt) = alt_in(context)
                && alt.tree != primary.tree
                && primary.also.iter().all(|a| a.tree != alt.tree)
            {
                primary.also.push(alt);
            }
        }
        // Text that is code in more than one context means each: Rust's
        // `also: $A` is a struct field in a declaration and in a literal.
        // The contexts after the one it compiled in are tried strictly, and
        // each that reads it as a different tree is matched as well.
        if let Some(index) = primary
            .context
            .as_deref()
            .and_then(|c| language.contexts.iter().position(|k| *k == c))
        {
            for context in &language.contexts[index + 1..] {
                let only = PatternLanguage {
                    contexts: std::slice::from_ref(context),
                    also: &[],
                    ..language.clone()
                };
                if let Ok(alt) = Self::compile_with(&only, text, false, false)
                    && alt.tree != primary.tree
                    && primary.also.iter().all(|a| a.tree != alt.tree)
                {
                    primary.also.push(alt);
                }
            }
        }
        // `to_svg($A)` also as `$COLAB_PATH::to_svg($A)`: the callee
        // `svg::to_svg` is a scoped path, which a bare identifier never
        // matches, so without this a pattern silently misses every
        // qualified call to the function it names.
        if let Some(sep) = language.qualified_calls
            && let Some((qualified, callee)) = qualify_call(text, sep)
            && let Ok(mut alt) = Self::compile_one(language, &qualified, true)
        {
            alt.qualified = Some(Qualified {
                separator: sep,
                callee: callee.to_string(),
            });
            primary.also.push(alt);
        }
        // A trailing `,` that changes no named node is a separator, not
        // structure: Rust's `Some($X) => $E,` then also matches the arm
        // written `Some(x) => { … }` (or last, without one). Where the comma
        // means something (Python's `x,` is a tuple) the trees differ and
        // the pattern is left as written.
        if let Some(stripped) = text.trim_end().strip_suffix(',')
            && let Ok(loose) = Self::compile_one(language, stripped, true)
            && loose.tree == primary.tree
        {
            primary.pattern = loose.pattern;
            primary.context = loose.context;
        }
        primary.prefilter = shared_prefilter(&primary);
        Ok(primary)
    }

    /// Compile in the language's contexts, trying the pattern on its own
    /// first when `bare`.
    fn compile_one(language: &PatternLanguage, text: &str, bare: bool) -> Result<Self, String> {
        // Strictly first, in every wrapper; a placeholder-only error (see
        // `parse_pattern`) is accepted only when nothing parses cleanly,
        // so it never displaces a wrapper that fits.
        Self::compile_with(language, text, bare, false)
            .or_else(|strict| Self::compile_with(language, text, bare, true).map_err(|_| strict))
    }

    fn compile_with(
        language: &PatternLanguage,
        text: &str,
        bare: bool,
        tolerant: bool,
    ) -> Result<Self, String> {
        if text.trim().is_empty() {
            return Err("a pattern cannot be empty".into());
        }
        let lang = TsLang {
            grammar: language.grammar.clone(),
            expando: language.expando,
            leading: language.leading,
        };
        let expanded = expand(text, lang.expando);

        // A lone metavariable (`$X`) has no shape of its own, so it matches
        // any node; a `where $X is <kind>` then picks the kind. Built
        // without a context, which would pin it to the wrapper's node kind.
        if bare
            && is_lone_metavariable(text.trim())
            && let Ok(pattern) = Pattern::try_new(text.trim(), lang.clone())
        {
            let printed = format!("{} (any node)", text.trim());
            return Ok(Self::finish(
                lang,
                pattern,
                text,
                None,
                None,
                printed,
                Shape::Any,
            ));
        }

        let mut attempts = Vec::new();
        // Bare pattern.
        let as_is = if bare {
            parse_pattern(&lang.grammar, &expanded, lang.expando, tolerant)
        } else {
            Err("not tried".to_string())
        };
        match as_is {
            Ok(tree) => match Pattern::try_new(text, lang.clone()) {
                Ok(pattern) => {
                    let root = single_root(tree.root_node());
                    let prefilter = longest_literal(root, &expanded, lang.expando);
                    let printed = print_tree(root, &expanded, lang.expando, false);
                    let shape = Shape::of(root, &expanded, lang.expando);
                    return Ok(Self::finish(
                        lang, pattern, text, prefilter, None, printed, shape,
                    ));
                }
                Err(e) => attempts.push(format!("as-is: {e}")),
            },
            Err(e) => attempts.push(format!("as-is: {e}")),
        }

        for context in language.contexts {
            let Some(slot) = context.find(PATTERN_SLOT) else {
                continue;
            };
            let filled = context.replacen(PATTERN_SLOT, text, 1);
            let filled_expanded = context.replacen(PATTERN_SLOT, &expanded, 1);
            let tree = match parse_pattern(&lang.grammar, &filled_expanded, lang.expando, tolerant)
            {
                Ok(tree) => tree,
                Err(e) => {
                    attempts.push(format!("in `{}`: {e}", context.replace('\n', " ")));
                    continue;
                }
            };
            let span = slot..slot + expanded.len();
            let trimmed = trim_span(&filled_expanded, span);
            let Some(node) = deepest_covering(tree.root_node(), trimmed.clone()) else {
                attempts.push(format!(
                    "in `{}`: the pattern is not a single syntax node there",
                    context.replace('\n', " ")
                ));
                continue;
            };
            // ast-grep's contextual pattern selects the *first* node of the
            // selector's kind in the context. If the wrapper itself has an
            // earlier node of that kind — the `_` in `func _()` for an
            // identifier pattern — that node would become the pattern and
            // match the wrong thing. Refuse the context instead.
            if first_of_kind(tree.root_node(), node.kind()).map(|n| n.byte_range())
                != Some(node.byte_range())
            {
                attempts.push(format!(
                    "in `{}`: the wrapper has an earlier `{}` than the pattern",
                    context.replace('\n', " "),
                    node.kind()
                ));
                continue;
            }
            match Pattern::contextual(&filled, node.kind(), lang.clone()) {
                Ok(pattern) => {
                    let prefilter = longest_literal(node, &filled_expanded, lang.expando);
                    let printed = print_tree(node, &filled_expanded, lang.expando, false);
                    let shape = Shape::of(node, &filled_expanded, lang.expando);
                    return Ok(Self::finish(
                        lang,
                        pattern,
                        text,
                        prefilter,
                        Some(context.to_string()),
                        printed,
                        shape,
                    ));
                }
                Err(e) => attempts.push(format!("in `{}`: {e}", context.replace('\n', " "))),
            }
        }
        Err(format!(
            "`{}` is not valid code in this language: {}",
            text,
            attempts.join("; ")
        ))
    }

    fn finish(
        lang: TsLang,
        pattern: Pattern,
        text: &str,
        prefilter: Option<String>,
        context: Option<String>,
        tree: String,
        shape: Shape,
    ) -> Self {
        Self {
            lang,
            pattern,
            text: text.to_string(),
            prefilter,
            context,
            tree,
            checks: Vec::new(),
            constraints: Vec::new(),
            lets: Vec::new(),
            also: Vec::new(),
            qualified: None,
            shape,
        }
    }

    /// Narrow the pattern with `where` constraints, compiled for
    /// `language`. Every metavariable a constraint names must be bound by
    /// the pattern, with the same sigil; every node kind must exist in
    /// the grammar; every regex and nested pattern must compile.
    pub fn constrain(
        mut self,
        language: &PatternLanguage,
        constraints: &[Constraint],
    ) -> Result<Self, String> {
        let bound = metavariables(&self.text);
        self.checks = constraints
            .iter()
            .filter(|c| !c.is_let())
            .map(|c| Check::compile(language, &bound, c))
            .collect::<Result<_, _>>()?;
        // A language server's answer costs a round trip; ask it only about
        // matches the other constraints keep.
        self.checks
            .sort_by_key(|c| matches!(c.kind, CheckKind::Semantic { .. }));
        self.lets = Derived::compile_all(language, &bound, constraints)?;
        self.constraints = constraints.to_vec();
        self.also = std::mem::take(&mut self.also)
            .into_iter()
            .map(|alt| alt.constrain(language, constraints))
            .collect::<Result<_, _>>()?;
        Ok(self)
    }

    /// Whether rewriting `template` — as code, its metavariables in place,
    /// in the wrapper the pattern compiled in — with `template` changes it
    /// again: the rule would not settle on its own output. A template that
    /// matches but rewrites to itself (fixed text) settles, and is fine.
    pub fn rematches(&self, template: &str) -> bool {
        let expanded = expand(template, self.lang.expando);
        let source = match &self.context {
            Some(context) => context.replacen(PATTERN_SLOT, &expanded, 1),
            None => expanded,
        };
        !self.find(&source).is_empty() && self.rewrite(&source, template) != source
    }

    /// The `where` constraints, as written.
    pub fn constraints(&self) -> &[Constraint] {
        &self.constraints
    }

    /// Every match in the tree that satisfies the constraints.
    fn matches<'t>(
        &self,
        grep: &'t ast_grep_core::AstGrep<StrDoc<TsLang>>,
        source: &str,
    ) -> Vec<NodeMatch<'t, StrDoc<TsLang>>> {
        // Semantic checks are sorted last. Each one asks the server about
        // every match still standing in one batch, so a file's questions
        // cost one round trip per check rather than one per match.
        let (cheap, asked) = self
            .checks
            .split_at(self.checks.partition_point(|c| !c.is_semantic()));
        let mut found: Vec<_> = candidates(grep, source, &self.pattern)
            .into_iter()
            .filter(|m| self.shape.fits(m.get_node()))
            .filter(|m| cheap.iter().all(|c| c.holds(m, source)))
            .collect();
        for check in asked {
            check.prefetch(&found, source);
            found.retain(|m| check.holds(m, source));
        }
        found
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

    /// Every match in `source` in source order, an outer match before the
    /// matches nested inside it.
    pub fn find(&self, source: &str) -> Vec<Found> {
        let mut found = self.find_own(source);
        for alt in &self.also {
            found.extend(alt.find_own(source));
        }
        found.sort_by_key(|f| (f.start, f.end));
        found.dedup_by_key(|f| (f.start, f.end));
        found
    }

    /// Matches of this compilation only, not its `also` ones.
    fn find_own(&self, source: &str) -> Vec<Found> {
        let grep = parsed(&self.lang, source);
        let mut lines = Lines::new(source);
        self.matches(&grep, source)
            .into_iter()
            .map(|m| {
                let range = m.range();
                let (line, column) = lines.at(range.start);
                let env = m.get_env();
                let mut captures: Vec<(String, String)> = env
                    .get_matched_variables()
                    .filter_map(|var| match var {
                        MetaVariable::Capture(name, _) => env
                            .get_match(&name)
                            .map(|node| (name.clone(), node.text().into_owned())),
                        MetaVariable::MultiCapture(name) => {
                            let nodes = env.get_multiple_matches(&name);
                            let text = match (nodes.first(), nodes.last()) {
                                (Some(first), Some(last)) => {
                                    source[first.range().start..last.range().end].to_string()
                                }
                                _ => String::new(),
                            };
                            Some((name, text))
                        }
                        _ => None,
                    })
                    .collect();
                captures.sort();
                captures.dedup();
                Found {
                    start: range.start,
                    end: range.end,
                    line,
                    column,
                    text: m.text().into_owned(),
                    captures,
                }
            })
            .collect()
    }

    /// Replace every match with `template`, substituting `$NAME` /
    /// `$$$NAME` bindings.
    ///
    /// Matches can nest — `errors.Wrap(errors.Wrap(err, "a"), "b")` holds
    /// two. ast-grep's own `replace_all` rewrites only the outermost,
    /// which leaves the inner one for a second run: a rule that is not
    /// idempotent. Here every binding is rewritten first, recursively,
    /// and the template is filled with those rewritten texts, so one pass
    /// reaches the fixed point. Only matches present in `source` are
    /// rewritten; text the template introduces is never re-matched, so a
    /// wrap-style template (`f($A)` → `f(g($A))`) cannot run away.
    pub fn rewrite(&self, source: &str, template: &str) -> String {
        // Each compilation matches different nodes, so they rewrite in turn.
        self.also
            .iter()
            .fold(self.rewrite_own(source, template), |out, alt| {
                alt.rewrite_own(&out, template)
            })
    }

    fn rewrite_own(&self, source: &str, template: &str) -> String {
        let grep = parsed(&self.lang, source);
        let matches = self.matches(&grep, source);
        if matches.is_empty() {
            return source.to_string();
        }
        let requalified = self.qualified.as_ref().and_then(|q| q.template(template));
        let template = requalified.as_deref().unwrap_or(template);
        let (template, names) = transformable_template(template);
        let fix = TemplateFix::with_transform(&template, &self.lang, &names);
        // A `let`'s `rewrite(<name>)` step: for each form the rewriter's
        // pattern compiled to (its `also` ones too), the fix and its
        // matches in this source; one list per `let`, empty without one.
        type Parts<'g> = Vec<(
            &'g StructuralPattern,
            TemplateFix,
            Vec<NodeMatch<'g, StrDoc<TsLang>>>,
        )>;
        let sub_parts: Vec<Parts<'_>> = self
            .lets
            .iter()
            .map(|d| {
                let grep = &grep;
                d.rewriters
                    .iter()
                    .flat_map(|(sub, template)| {
                        let (template, names) = transformable_template(template);
                        std::iter::once(sub).chain(&sub.also).map(move |form| {
                            let fix = TemplateFix::with_transform(&template, &form.lang, &names);
                            (form, fix, form.matches(grep, source))
                        })
                    })
                    .collect()
            })
            .collect();
        let subs: Vec<Vec<Rewriter<'_, '_>>> = sub_parts
            .iter()
            .map(|parts| {
                parts
                    .iter()
                    .map(|(form, fix, matches)| Rewriter {
                        pattern: &form.pattern,
                        fix,
                        lets: &[],
                        subs: &[],
                        matches,
                    })
                    .collect()
            })
            .collect();
        let rewriter = Rewriter {
            pattern: &self.pattern,
            fix: &fix,
            lets: &self.lets,
            subs: &subs,
            matches: &matches,
        };
        let edits = rewriter
            .outermost(0..source.len(), None)
            .into_iter()
            .map(|i| rewriter.render(source, i))
            .filter(|edit| !same_but_whitespace(&source[edit.start..edit.end], &edit.text))
            .collect();
        apply_edits(source, edits)
    }

    /// Remove every match. One that stands alone on its line(s) — apart
    /// from indentation and a trailing `;` or `,` — goes with its whole
    /// line. One inside a line goes with the `,` beside it, so a list
    /// element leaves a well-formed list: the following comma and the
    /// space after it (`f(a, b, c)` → `f(a, c)`), or for the last element
    /// the preceding one (`f(a, b)` → `f(a)`). A match with no comma beside
    /// it (`() => f(x)`, `return f(x)`) is left in place: removing it would
    /// leave the code around it incomplete. Rewrite those with `replace`.
    pub fn delete(&self, source: &str) -> String {
        let mut tree = None;
        let mut parsed = || {
            tree.get_or_insert_with(|| crate::parse_cache::parse(&self.lang.grammar, source))
                .clone()
        };
        let leading = self.lang.leading;
        let mut spans: Vec<(usize, usize)> = Vec::new();
        for found in self.find(source) {
            // An item's doc comments and attributes go with it.
            let start = if leading.kinds.is_empty() && leading.doc_comments.is_empty() {
                found.start
            } else {
                parsed().map_or(found.start, |t| {
                    attached_start(&t, source, found.start, found.end, leading)
                })
            };
            let (line_start, line_end) = line_span(source, start, found.end);
            let before = &source[line_start..start];
            let after = &source[found.end..line_end];
            let alone = before.trim().is_empty()
                && after
                    .trim()
                    .trim_start_matches([';', ','])
                    .trim()
                    .is_empty();
            if alone {
                spans.push((
                    line_start,
                    without_double_blank(source, line_start, line_end),
                ));
                continue;
            }
            if let Some(span) = parsed()
                .as_ref()
                .and_then(|t| with_separator(t, found.start, found.end))
            {
                spans.push(span);
            }
        }
        // Outermost first: a span inside one already taken goes with it.
        spans.sort_by_key(|&(start, end)| (start, std::cmp::Reverse(end)));
        let mut kept: Vec<(usize, usize)> = Vec::new();
        for span in spans {
            if kept.last().is_some_and(|last| span.0 < last.1) {
                continue;
            }
            kept.push(span);
        }
        let mut out = source.to_string();
        for (start, end) in kept.into_iter().rev() {
            out.replace_range(start..end, "");
        }
        out
    }

    /// `true` when `source` parses without a syntax error.
    fn parses_cleanly(&self, source: &str) -> bool {
        parse_clean(&self.lang.grammar, source).is_ok()
    }
}

/// Where the item matched at `start..end` really starts: before the doc
/// comments and attributes just above it that belong to it ([`Leading`]),
/// each on its own line(s) with no blank line between it and the next.
fn attached_start(
    tree: &tree_sitter::Tree,
    source: &str,
    start: usize,
    end: usize,
    leading: Leading,
) -> usize {
    let Some(mut node) = tree.root_node().descendant_for_byte_range(start, end) else {
        return start;
    };
    // The widest node over exactly the match: the item, not its first token.
    while let Some(parent) = node.parent() {
        if parent.byte_range() != node.byte_range() {
            break;
        }
        node = parent;
    }
    let mut at = start;
    let mut cursor = node;
    while let Some(prev) = cursor.prev_sibling() {
        let text = &source[prev.byte_range()];
        let attached = leading.kinds.contains(&prev.kind())
            || (prev.is_extra() && leading.doc_comments.iter().any(|p| text.starts_with(p)));
        // Only up to the next item: whitespace and at most one line break
        // between them, and nothing before it on its first line.
        let prev_end = prev.start_byte() + text.trim_end().len();
        let gap = &source[prev_end..at];
        let line_start = source[..prev.start_byte()].rfind('\n').map_or(0, |p| p + 1);
        if !attached
            || !gap.trim().is_empty()
            || gap.matches('\n').count() > 1
            || !source[line_start..prev.start_byte()].trim().is_empty()
        {
            break;
        }
        at = prev.start_byte();
        cursor = prev;
    }
    at
}

/// `line_end`, or past the blank line after it when the deleted lines
/// `line_start..line_end` sit between two blank lines (or open the file
/// before one): the deletion then leaves one blank line, not two.
fn without_double_blank(source: &str, line_start: usize, line_end: usize) -> usize {
    let blank_before = line_start == 0 || {
        let prev = source[..line_start - 1].rfind('\n').map_or(0, |p| p + 1);
        source[prev..line_start].trim().is_empty()
    };
    let rest = &source[line_end..];
    let next_end = rest.find('\n').map_or(rest.len(), |p| p + 1);
    let next = &rest[..next_end];
    if blank_before && !next.is_empty() && next.trim().is_empty() && next.ends_with('\n') {
        line_end + next_end
    } else {
        line_end
    }
}

/// A literal every form of `pattern` needs, so the rule's prefilter stays
/// sound however many forms it matches in: each form's literal must be in
/// a file for that form to match, so the one contained in all the others
/// is in every file any form matches. `None` when no form's literal is in
/// all the others.
fn shared_prefilter(pattern: &StructuralPattern) -> Option<String> {
    let literals: Vec<Option<&str>> = std::iter::once(pattern)
        .chain(&pattern.also)
        .map(|form| form.prefilter.as_deref())
        .collect();
    let all: Vec<&str> = literals.iter().copied().collect::<Option<_>>()?;
    all.iter()
        .filter(|candidate| all.iter().all(|other| other.contains(**candidate)))
        .max_by_key(|candidate| candidate.len())
        .map(|s| (*s).to_string())
}

/// The bytes to remove for the match at `start..end` with the `,` beside
/// it: up to the next element when a comma follows and an element after
/// it, else from the end of the previous element when a comma precedes,
/// else the match and a trailing comma alone (`f(a,)`). `None` when no
/// comma is beside it.
fn with_separator(tree: &tree_sitter::Tree, start: usize, end: usize) -> Option<(usize, usize)> {
    let mut node = tree.root_node().descendant_for_byte_range(start, end)?;
    // The widest node over exactly the match: an argument, not its inner
    // expression.
    while let Some(parent) = node.parent() {
        if parent.byte_range() != node.byte_range() {
            break;
        }
        node = parent;
    }
    let is_comma = |n: &tree_sitter::Node<'_>| !n.is_named() && n.kind() == ",";
    let next = node.next_sibling().filter(is_comma);
    let prev = node.prev_sibling().filter(is_comma);
    match (next, prev) {
        (Some(next), _) => match next.next_named_sibling() {
            Some(element) => Some((node.start_byte(), element.start_byte())),
            None => match prev {
                Some(prev) => Some((
                    prev.prev_named_sibling()
                        .map_or(prev.start_byte(), |e| e.end_byte()),
                    node.end_byte(),
                )),
                None => Some((node.start_byte(), next.end_byte())),
            },
        },
        (None, Some(prev)) => Some((
            prev.prev_named_sibling()
                .map_or(prev.start_byte(), |e| e.end_byte()),
            node.end_byte(),
        )),
        (None, None) => None,
    }
}

/// `true` when `a` and `b` differ only in whitespace outside quoted text.
///
/// A multi-line `$$$` capture filled into a one-line template loses the
/// line breaks around it, so `f($$$A)` → `f($$$A)` would reflow every
/// call it matched and report each as changed. Such an edit changes no
/// token, so it is not made. Whitespace inside `"…"`, `'…'` or `` `…` ``
/// is compared as is (a string's spaces are its content); a quote the
/// scan misreads, such as a Rust lifetime, only keeps more whitespace in
/// the comparison, so the edit is made, as it would be without the check.
fn same_but_whitespace(a: &str, b: &str) -> bool {
    fn tokens(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut quote: Option<char> = None;
        let mut escaped = false;
        for c in text.chars() {
            match quote {
                Some(q) => {
                    out.push(c);
                    if escaped {
                        escaped = false;
                    } else if c == '\\' {
                        escaped = true;
                    } else if c == q {
                        quote = None;
                    }
                }
                None if c.is_whitespace() => {}
                None => {
                    if matches!(c, '"' | '\'' | '`') {
                        quote = Some(c);
                    }
                    out.push(c);
                }
            }
        }
        out
    }
    a != b && tokens(a) == tokens(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn go() -> PatternLanguage {
        PatternLanguage {
            grammar: tree_sitter_go::LANGUAGE.into(),
            expando: 'µ',
            contexts: &["package p\nfunc _() {\n@@PATTERN@@\n}\n"],
            also: &[],
            qualified_calls: None,
            leading: Leading::NONE,
        }
    }

    fn python() -> PatternLanguage {
        PatternLanguage {
            grammar: tree_sitter_python::LANGUAGE.into(),
            expando: 'µ',
            contexts: &[],
            also: &["def _(x: @@PATTERN@@): pass\n"],
            qualified_calls: None,
            leading: Leading::NONE,
        }
    }

    const GO_SRC: &str = "package main\n\nfunc main() {\n\tif err := f(); err != nil {\n\t\treturn errors.Wrap(err, \"f failed\")\n\t}\n\tlog.Println(\"a\", 1)\n}\n";

    #[test]
    fn a_rewrite_keeps_the_layout_of_what_it_does_not_change() {
        // A one-line template over a many-line match changes one name.
        assert_eq!(
            keep_layout("f() {\n\tx := 1\n\treturn x\n}", "g() { x := 1 return x }"),
            "g() {\n\tx := 1\n\treturn x\n}"
        );
        // Next to a change, the template's spacing: tokens never fuse.
        assert_eq!(
            keep_layout("extern \"C\" {}", "unsafe extern \"C\" {}"),
            "unsafe extern \"C\" {}"
        );
        assert_eq!(keep_layout("x+y", "x in y"), "x in y");
        // Where the template breaks a line, its indentation stands.
        assert_eq!(
            keep_layout("if c:\n    a()", "if c:\n    with l:\n        a()"),
            "if c:\n    with l:\n        a()"
        );
        // After a deletion, the template's spacing; a closing brace on its
        // own line stays there.
        assert_eq!(keep_layout("w(err, \"m\")", "f(\"m\")"), "f(\"m\")");
        assert_eq!(
            keep_layout("if e {\n\treturn e\n}", "if e { return g(e) }"),
            "if e {\n\treturn g(e)\n}"
        );
        // A many-line capture in a one-line template keeps its lines'
        // indentation, and unwrapping a block keeps it unwrapped.
        assert_eq!(
            keep_layout("c({\n  t: 1,\n  u: 2,\n})", "d({ t: 1,\nu: 2, })"),
            "d({\n  t: 1,\n  u: 2,\n})"
        );
        assert_eq!(
            keep_layout("with l:\n    a()\n    b()", "a()\nb()"),
            "a()\nb()"
        );
        // A replaced token takes the place of the one it replaces.
        assert_eq!(
            keep_layout("c({\n  t: 1,\n  u: 2,\n})", "d({ tMs: 1 * 9,\nu: 2, })"),
            "d({\n  tMs: 1 * 9,\n  u: 2,\n})"
        );
        // A string is one token: its spaces are never taken from the old.
        assert_eq!(keep_layout("f(\"a  b\")", "f(\"a b\")"), "f(\"a b\")");
        // An element added before a list laid out one per line gets its
        // own line; added to a one-line list, it stays on that line.
        assert_eq!(
            keep_layout("build(\n    a,\n    b,\n)", "make(extra, a,\n    b,\n)"),
            "make(\n    extra,\n    a,\n    b,\n)"
        );
        assert_eq!(
            keep_layout("build(a, b)", "make(extra, a, b)"),
            "make(extra, a, b)"
        );
        // A token the template joins to the one before stays joined, though
        // it replaces one the original spaced: no `a:: b` from `a: b`.
        assert_eq!(
            keep_layout(
                "Capability {\n    module: \"pattern\",\n    description: $D,\n}",
                "colab_rewrite::language::pattern_capability($D)"
            ),
            "colab_rewrite::language::pattern_capability($D)"
        );
    }

    // ---------- relations, positions, any / all ---------------------------

    const REL_SRC: &str = "package p\nfunc f() error {\n\tf, err := os.Open(\"x\")\n\tif err != nil {\n\t\treturn err\n\t}\n\tdefer f.Close()\n\tg(1, 2, 3)\n\tgo func() {\n\t\tfor _, x := range xs {\n\t\t\tuse(x)\n\t\t}\n\t}()\n\tfor _, y := range ys {\n\t\tgo func() { use(y) }()\n\t}\n\treturn nil\n}\n";

    fn texts(pattern: &str, constraints: &[Constraint]) -> Vec<String> {
        constrained(pattern, constraints)
            .unwrap()
            .find(REL_SRC)
            .into_iter()
            .map(|f| f.text)
            .collect()
    }

    fn rel(relation: colab_core::Relation, pattern: &str, stop: colab_core::Stop) -> Test {
        Test::Related {
            relation,
            pattern: pattern.into(),
            stop,
        }
    }

    #[test]
    fn sibling_relations_look_past_or_only_at_the_neighbour() {
        use colab_core::{Relation::*, Stop::*};
        let defer = "defer $X.Close()";
        let after_if = rel(Follows, "if err != nil { $$$ }", Directly);
        assert_eq!(texts(defer, &[c(false, after_if)]), ["defer f.Close()"]);
        let after_open = |stop| rel(Follows, "$A, err := $$$B", stop);
        assert!(texts(defer, &[c(false, after_open(Directly))]).is_empty());
        assert_eq!(texts(defer, &[c(false, after_open(Anywhere))]).len(), 1);
        // `until` stops at a sibling matching it.
        let past_if = after_open(Until("if $C { $$$ }".into()));
        assert!(texts(defer, &[c(false, past_if)]).is_empty());
        // A matched call is its statement: its siblings are statements.
        let before_return = rel(Precedes, "return nil", Anywhere);
        assert_eq!(texts("g($$$A)", &[c(false, before_return)]), ["g(1, 2, 3)"]);
    }

    #[test]
    fn inside_and_has_take_a_stop() {
        use colab_core::{Relation::*, Stop::*};
        let use_ = "use($X)";
        let in_loop = |stop| c(false, rel(Inside, "for $$$ { $$$ }", stop));
        assert_eq!(texts(use_, &[in_loop(Anywhere)]).len(), 2);
        // The loop variable captured by a closure: `y`, not `x`.
        let closure = in_loop(Until("func() { $$$ }".into()));
        assert_eq!(texts(use_, &[closure]), ["use(x)"]);
        // `directly`: the nearest larger enclosing node only.
        let call = c(false, rel(Inside, "use($$$)", Directly));
        assert_eq!(
            texts(
                "$X",
                &[
                    c(
                        false,
                        Test::Is {
                            var: var("$X"),
                            kind: "argument_list".into()
                        }
                    ),
                    call
                ]
            )
            .len(),
            2
        );
        let has_call = |stop| c(false, rel(Has, "use($$$)", stop));
        let funcs = "func() { $$$B }";
        assert_eq!(texts(funcs, &[has_call(Anywhere)]).len(), 2);
        assert!(texts(funcs, &[has_call(Until("for $$$ { $$$ }".into()))]).len() == 1);
        assert!(texts(funcs, &[has_call(Directly)]).is_empty());
    }

    #[test]
    fn positions_fields_and_groups() {
        let int = || {
            c(
                false,
                Test::Is {
                    var: var("$X"),
                    kind: "int_literal".into(),
                },
            )
        };
        assert_eq!(texts("$X", &[int(), c(false, Test::Nth(-1))]), ["3"]);
        assert_eq!(texts("$X", &[int(), c(false, Test::Nth(2))]), ["2"]);
        let either = Test::Any(vec![c(false, Test::Nth(1)), c(false, Test::Nth(3))]);
        assert_eq!(texts("$X", &[int(), c(false, either)]), ["1", "3"]);
        let both = Test::All(vec![c(false, Test::Nth(1)), c(false, Test::Nth(-1))]);
        assert!(texts("$X", &[int(), c(false, both.clone())]).is_empty());
        assert_eq!(texts("$X", &[int(), c(true, both)]).len(), 3);
        let ident = || {
            c(
                false,
                Test::Is {
                    var: var("$F"),
                    kind: "identifier".into(),
                },
            )
        };
        let called = texts("$F", &[ident(), c(false, Test::Field("function".into()))]);
        assert_eq!(called, ["g", "use", "use"]);
        let e = constrained("$F", &[c(false, Test::Field("functon".into()))]).unwrap_err();
        assert!(e.contains("did you mean `function`"), "{e}");
        let e = constrained("$F", &[c(false, Test::Nth(0))]).unwrap_err();
        assert!(e.contains("count from 1"), "{e}");
        let e = constrained("$F", &[c(false, Test::Any(Vec::new()))]).unwrap_err();
        assert!(e.contains("at least one"), "{e}");
        // A group checks its members like the top level does.
        let unbound = Test::Any(vec![c(
            false,
            Test::Is {
                var: var("$Q"),
                kind: "identifier".into(),
            },
        )]);
        assert!(constrained("$F", &[c(false, unbound)]).is_err());
    }

    // ---------- let ------------------------------------------------------

    fn derive(var_: &str, from: &str, steps: Vec<colab_core::Transform>) -> Constraint {
        c(
            false,
            Test::Let {
                var: var(var_),
                from: var(from),
                steps,
            },
        )
    }

    #[test]
    fn a_let_fills_the_template_with_a_derived_value() {
        use colab_core::Transform::{Snake, StripPrefix, Upper};
        let lets = [
            derive("$NAME", "$F", vec![StripPrefix("Get".into())]),
            // From an earlier `let`.
            derive("$SNAKE", "$NAME", vec![Snake, Upper]),
        ];
        let p = constrained("$X.$F()", &lets).unwrap();
        let src = "package p\nfunc f() { u.GetUserID(); u.Other() }\n";
        assert_eq!(
            p.rewrite(src, "$X.$NAME() + $SNAKE"),
            "package p\nfunc f() { u.UserID() + USER_ID; u.Other() + OTHER }\n"
        );
        // Nested matches are rewritten inside out, the `let` with them.
        let nested = "package p\nfunc f() { a.GetB().GetC() }\n";
        assert_eq!(
            p.rewrite(nested, "$X.$NAME()"),
            "package p\nfunc f() { a.B().C() }\n"
        );
        let rule = PatternRule::compile(
            &go(),
            "$X.$F()",
            PatternAction::Replace("$X.$NAME()".into()),
            &lets[..1],
        )
        .unwrap();
        assert_eq!(
            rule.to_string(),
            "pattern '$X.$F()' let $NAME = $F | strip_prefix(\"Get\") -> '$X.$NAME()'"
        );
    }

    #[test]
    fn a_rewriter_rewrites_each_match_inside_a_binding() {
        let rewrite = |pattern: &str, template: &str| {
            colab_core::Transform::Rewrite(vec![colab_core::Rewriter {
                name: "r".into(),
                pattern: pattern.into(),
                template: template.into(),
            }])
        };
        let lets = [derive("$N", "$$$A", vec![rewrite("g($X)", "h($X)")])];
        let p = constrained("f($$$A)", &lets).unwrap();
        let src = "package p\nfunc m() { f(g(1), 2, g(3)); k(g(4)) }\n";
        // Each element the rewriter matches; the rest, and the separators,
        // as they were. Outside the binding nothing changes.
        assert_eq!(
            p.rewrite(src, "ff($N)"),
            "package p\nfunc m() { ff(h(1), 2, h(3)); k(g(4)) }\n"
        );
        // Later steps act on the rewritten text.
        let upper = [derive(
            "$N",
            "$$$A",
            vec![rewrite("g($X)", "h($X)"), colab_core::Transform::Upper],
        )];
        let q = constrained("f($$$A)", &upper).unwrap();
        assert_eq!(
            q.rewrite("package p\nfunc m() { f(g(x)) }\n", "f($N)"),
            "package p\nfunc m() { f(H(X)) }\n"
        );
        let err = |lets: &[Constraint]| constrained("f($$$A)", lets).unwrap_err();
        let late = derive(
            "$N",
            "$$$A",
            vec![colab_core::Transform::Lower, rewrite("g($X)", "h($X)")],
        );
        assert!(err(&[late]).contains("must be the first step"));
        let from_let = [
            derive("$M", "$$$A", vec![colab_core::Transform::Lower]),
            derive("$N", "$M", vec![rewrite("g($X)", "h($X)")]),
        ];
        assert!(err(&from_let).contains("not from a `let`"));
        let unbound = derive("$N", "$$$A", vec![rewrite("g($X)", "h($Y)")]);
        assert!(err(&[unbound]).contains("`$Y`"));
    }

    #[test]
    fn a_let_must_name_a_new_value_from_a_bound_one() {
        let lower = || vec![colab_core::Transform::Lower];
        let err = |lets: &[Constraint]| constrained("$X.$F()", lets).unwrap_err();
        assert!(err(&[derive("$X", "$F", lower())]).contains("already binds `$X`"));
        assert!(err(&[derive("$N", "$Q", lower())]).contains("not a metavariable"));
        assert!(err(&[derive("$$$N", "$F", lower())]).contains("one `$`"));
        assert!(
            err(&[derive("$N", "$F", lower()), derive("$N", "$X", lower())])
                .contains("already a `let`")
        );
        let bad = colab_core::Transform::Replace {
            regex: "(".into(),
            with: String::new(),
        };
        assert!(err(&[derive("$N", "$F", vec![bad])]).contains("replace(/(/"));
        let delete = PatternRule::compile(
            &go(),
            "$X.$F()",
            PatternAction::Delete,
            &[derive("$N", "$F", lower())],
        )
        .unwrap_err();
        assert!(delete.contains("`delete` has none"), "{delete}");
    }

    // ---------- where constraints ----------------------------------------

    fn var(text: &str) -> Var {
        Var::parse(text).unwrap()
    }

    fn c(negated: bool, test: Test) -> Constraint {
        Constraint { negated, test }
    }

    fn constrained(pattern: &str, constraints: &[Constraint]) -> Result<StructuralPattern, String> {
        StructuralPattern::compile(&go(), pattern)?.constrain(&go(), constraints)
    }

    const WRAPS: &str = "package p\n\nfunc a() {\n\terrors.Wrap(err, \"lit\")\n\terrors.Wrap(err, msg)\n\tlog.Println()\n\tlog.Println(1, 2, 3)\n}\n\nfunc TestA(t *testing.T) {\n\terrors.Wrap(err, \"in test\")\n}\n";

    fn lines(p: &StructuralPattern) -> Vec<usize> {
        p.find(WRAPS).iter().map(|f| f.line).collect()
    }

    #[test]
    fn matches_filters_on_a_bindings_text() {
        let lit = c(
            false,
            Test::Matches {
                var: var("$M"),
                regex: "^\"".into(),
            },
        );
        let p = constrained("errors.Wrap($E, $M)", std::slice::from_ref(&lit)).unwrap();
        assert_eq!(lines(&p), [4, 11]);
        let not_lit = Constraint {
            negated: true,
            ..lit
        };
        let p = constrained("errors.Wrap($E, $M)", &[not_lit]).unwrap();
        assert_eq!(lines(&p), [5]);
    }

    #[test]
    fn is_filters_on_the_bound_node_kind() {
        let p = constrained(
            "errors.Wrap($E, $M)",
            &[c(
                false,
                Test::Is {
                    var: var("$M"),
                    kind: "identifier".into(),
                },
            )],
        )
        .unwrap();
        assert_eq!(lines(&p), [5]);
    }

    #[test]
    fn count_counts_named_nodes_in_a_sequence() {
        let count = |min, max| {
            constrained(
                "log.Println($$$A)",
                &[c(
                    false,
                    Test::Count {
                        var: var("$$$A"),
                        min,
                        max,
                    },
                )],
            )
            .unwrap()
        };
        assert_eq!(lines(&count(0, Some(0))), [6]);
        assert_eq!(lines(&count(3, Some(3))), [7]);
        assert_eq!(lines(&count(1, None)), [7]);
    }

    #[test]
    fn inside_and_has_test_ancestors_and_descendants() {
        let in_test = Test::Inside("func $F(t *testing.T) { $$$ }".into());
        let p = constrained("errors.Wrap($E, $M)", &[c(false, in_test.clone())]).unwrap();
        assert_eq!(lines(&p), [11]);
        let p = constrained("errors.Wrap($E, $M)", &[c(true, in_test)]).unwrap();
        assert_eq!(lines(&p), [4, 5]);
        let has = Test::Has("log.Println($$$)".into());
        let p = constrained("func $F() { $$$B }", &[c(false, has)]).unwrap();
        assert_eq!(lines(&p), [3]);
    }

    #[test]
    fn constraints_apply_to_rewrites_too() {
        let p = constrained(
            "errors.Wrap($E, $M)",
            &[c(
                true,
                Test::Inside("func $F(t *testing.T) { $$$ }".into()),
            )],
        )
        .unwrap();
        let out = p.rewrite(WRAPS, "fmt.Errorf($M, $E)");
        assert!(out.contains("errors.Wrap(err, \"in test\")"), "{out}");
        assert_eq!(out.matches("fmt.Errorf").count(), 2, "{out}");
    }

    #[test]
    fn a_constraint_must_name_a_bound_metavariable_with_its_sigil() {
        let matches = |name: &str| {
            c(
                false,
                Test::Matches {
                    var: var(name),
                    regex: "x".into(),
                },
            )
        };
        let err = constrained("errors.Wrap($E, $M)", &[matches("$X")]).unwrap_err();
        assert!(err.contains("it binds $E, $M"), "{err}");
        let err = constrained("errors.Wrap($E, $M)", &[matches("$$$M")]).unwrap_err();
        assert!(err.contains("written `$M`"), "{err}");
        let count = c(
            false,
            Test::Count {
                var: var("$M"),
                min: 1,
                max: None,
            },
        );
        let err = constrained("errors.Wrap($E, $M)", &[count]).unwrap_err();
        assert!(err.contains("bind it as `$$$M`"), "{err}");
    }

    #[test]
    fn an_unknown_kind_or_bad_regex_fails_at_compile_time() {
        let is = c(
            false,
            Test::Is {
                var: var("$M"),
                kind: "identifer".into(),
            },
        );
        let err = constrained("errors.Wrap($E, $M)", &[is]).unwrap_err();
        assert!(err.contains("did you mean `identifier`"), "{err}");
        let bad = c(
            false,
            Test::Matches {
                var: var("$M"),
                regex: "(".into(),
            },
        );
        assert!(constrained("errors.Wrap($E, $M)", &[bad]).is_err());
        let nested = c(false, Test::Inside("func (".into()));
        let err = constrained("errors.Wrap($E, $M)", &[nested]).unwrap_err();
        assert!(err.starts_with("`inside 'func ('`"), "{err}");
    }

    fn rust() -> PatternLanguage {
        PatternLanguage {
            grammar: tree_sitter_rust::LANGUAGE.into(),
            expando: 'µ',
            contexts: &["fn f() { @@PATTERN@@ }"],
            also: &[],
            qualified_calls: None,
            leading: Leading::NONE,
        }
    }

    #[test]
    fn a_lone_sequence_in_an_item_list_is_a_placeholder() {
        // `$$$B` is not a valid item, but alone in a body it stands for
        // the items.
        let p = StructuralPattern::compile(&rust(), "impl Loader { $$$B }").unwrap();
        let src = "struct Loader;\nimpl Loader {\n    fn load(&self) {}\n}\nimpl Other {}\n";
        let found = p.find(src);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].text.starts_with("impl Loader"));
        // Any other syntax error still fails the pattern.
        assert!(StructuralPattern::compile(&rust(), "impl Loader { fn }").is_err());
    }

    #[test]
    fn a_rewrite_that_only_moves_whitespace_is_not_made() {
        let src = "package p\nfunc g() {\n\tf(\n\t\ta,\n\t\tb,\n\t)\n}\n";
        let p = StructuralPattern::compile(&go(), "f($$$A)").unwrap();
        assert_eq!(
            p.rewrite(src, "f($$$A)"),
            src,
            "an identity rewrite reflowed the call"
        );
        assert_ne!(p.rewrite(src, "h($$$A)"), src);
        // Whitespace inside a string is content.
        assert!(!same_but_whitespace(r#"f("a b")"#, r#"f("ab")"#));
        assert!(same_but_whitespace("f(\n a,\n b)", "f(a, b)"));
    }

    #[test]
    fn a_match_keeps_no_children_the_pattern_leaves_out() {
        // An `else` is a trailing child `if $C { $$$B }` does not mention:
        // rewriting that match would drop it.
        let p = StructuralPattern::compile(&go(), "if $C { $$$B }").unwrap();
        let src = "package p\n\nfunc f() {\n\tif a {\n\t\tx()\n\t}\n\tif b {\n\t\ty()\n\t} else {\n\t\tz()\n\t}\n}\n";
        let found = p.find(src);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].text.starts_with("if a"));
        // A `$$$` still takes any number of children.
        let q = StructuralPattern::compile(&go(), "f($$$A)").unwrap();
        assert_eq!(q.find("package p\nfunc g() { f(1, 2, 3) }\n").len(), 1);
    }

    #[test]
    fn an_also_context_matches_the_same_text_parsed_another_way() {
        // `Optional[str]` is a `subscript` in an expression and a
        // `generic_type` in an annotation: one pattern finds both.
        let p = StructuralPattern::compile(&python(), "Optional[$T]").unwrap();
        let src = "def f(a: Optional[str]) -> Optional[int]:\n    return cast(Optional[str], a)\n";
        assert_eq!(p.find(src).len(), 3);
        assert_eq!(
            p.rewrite(src, "$T | None"),
            "def f(a: str | None) -> int | None:\n    return cast(str | None, a)\n"
        );
    }

    #[test]
    fn a_multi_line_template_follows_tab_indentation() {
        let go = PatternLanguage {
            grammar: tree_sitter_go::LANGUAGE.into(),
            expando: 'µ',
            contexts: &["package p\nfunc _() {\n@@PATTERN@@\n}\n"],
            also: &[],
            qualified_calls: None,
            leading: Leading::NONE,
        };
        let p = StructuralPattern::compile(&go, "log.Print($A)").unwrap();
        let src = "package p\n\nfunc f() {\n\tif x {\n\t\tlog.Print(a)\n\t}\n}\n";
        let out = p.rewrite(src, "if debug {\n\tlog.Print($A)\n}");
        assert_eq!(
            out,
            "package p\n\nfunc f() {\n\tif x {\n\t\tif debug {\n\t\t\tlog.Print(a)\n\t\t}\n\t}\n}\n"
        );
    }

    #[test]
    fn a_multi_line_template_leaves_no_trailing_whitespace() {
        // ast-grep re-indents a template's later lines to the match's
        // column (spaces only; tab-indented Go is left flush-left).
        let p = StructuralPattern::compile(&python(), "print($$$A)").unwrap();
        let out = p.rewrite("def f():\n    print(1)\n", "print($$$A)\n\nprint(2)");
        assert_eq!(out, "def f():\n    print(1)\n\n    print(2)\n");
    }

    #[test]
    fn expand_rewrites_only_metavariables() {
        assert_eq!(expand("f($A, $$$REST, $_)", 'µ'), "f(µA, µµµREST, µ_)");
        assert_eq!(expand("echo $x; \"$5\"", 'µ'), "echo $x; \"$5\"");
        assert_eq!(expand("{ $$$ }", 'µ'), "{ µµµ }");
    }

    #[test]
    fn go_expression_compiles_through_a_context() {
        let p = StructuralPattern::compile(&go(), "errors.Wrap($ERR, $MSG)").unwrap();
        assert_eq!(p.prefilter(), Some("errors"));
        let found = p.find(GO_SRC);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].line, 5);
        assert_eq!(
            found[0].captures,
            vec![
                ("ERR".to_string(), "err".to_string()),
                ("MSG".to_string(), "\"f failed\"".to_string())
            ]
        );
    }

    fn qualified_rust() -> PatternLanguage {
        PatternLanguage {
            qualified_calls: Some("::"),
            ..rust()
        }
    }

    const CALLS_SRC: &str = "fn f() {\n    to_svg(a, b);\n    svg::to_svg(c, d);\n    \
                             diagram_scene::svg::to_svg(e, f);\n    x.to_svg(g, h);\n}\n";

    #[test]
    fn a_bare_call_pattern_also_finds_qualified_calls() {
        let p = StructuralPattern::compile(&qualified_rust(), "to_svg($A, $B)").unwrap();
        let found: Vec<_> = p.find(CALLS_SRC).into_iter().map(|f| f.text).collect();
        assert_eq!(
            found,
            [
                "to_svg(a, b)",
                "svg::to_svg(c, d)",
                "diagram_scene::svg::to_svg(e, f)"
            ],
            "the method call is not a call of the function"
        );
        // A user metavariable binds the same as before; the path's is internal.
        let qualified = &p.find(CALLS_SRC)[1];
        assert!(qualified.captures.contains(&("A".into(), "c".into())));
    }

    #[test]
    fn a_rewrite_keeps_the_path_it_matched() {
        let p = StructuralPattern::compile(&qualified_rust(), "to_svg($A, $B)").unwrap();
        let renamed = p.rewrite(CALLS_SRC, "render($A, &$B)");
        assert!(renamed.contains("    render(a, &b);"), "{renamed}");
        assert!(renamed.contains("    svg::render(c, &d);"), "{renamed}");
        assert!(
            renamed.contains("diagram_scene::svg::render(e, &f);"),
            "{renamed}"
        );
        assert!(renamed.contains("x.to_svg(g, h)"), "{renamed}");
        assert_eq!(p.rewrite(&renamed, "render($A, &$B)"), renamed);
        // A wrap keeps the call as matched inside it only when bare.
        let method = p.rewrite(CALLS_SRC, "$A.to_svg($B)");
        assert!(method.contains("    c.to_svg(d);"), "{method}");
    }

    #[test]
    fn only_a_bare_callee_is_qualified() {
        for text in [
            "$F($A)",
            "svg::to_svg($A)",
            "println!($A)",
            "if ($A) {}",
            "x.f($A)",
        ] {
            assert!(qualify_call(text, "::").is_none(), "{text}");
        }
        assert_eq!(
            qualify_call("parse::<T>($A)", "::").map(|q| q.0),
            Some("$COLAB_PATH::parse::<T>($A)".into())
        );
        // Without the separator set, nothing changes.
        let p = StructuralPattern::compile(&rust(), "to_svg($A, $B)").unwrap();
        assert_eq!(p.find(CALLS_SRC).len(), 1);
    }

    #[test]
    fn rewrite_substitutes_bindings() {
        let p = StructuralPattern::compile(&go(), "errors.Wrap($ERR, $MSG)").unwrap();
        let out = p.rewrite(GO_SRC, "fmt.Errorf($MSG+\": %w\", $ERR)");
        assert!(
            out.contains("return fmt.Errorf(\"f failed\"+\": %w\", err)"),
            "{out}"
        );
        assert_eq!(p.rewrite(&out, "x"), out, "nothing left to match");
    }

    #[test]
    fn multi_capture_spans_all_arguments() {
        let p = StructuralPattern::compile(&go(), "log.Println($$$ARGS)").unwrap();
        let found = p.find(GO_SRC);
        assert_eq!(
            found[0].captures,
            vec![("ARGS".to_string(), "\"a\", 1".to_string())]
        );
        let out = p.rewrite(GO_SRC, "slog.Info($$$ARGS)");
        assert!(out.contains("slog.Info(\"a\", 1)"), "{out}");
    }

    #[test]
    fn nested_matches_are_rewritten_in_one_pass() {
        let p = StructuralPattern::compile(&go(), "errors.Wrap($ERR, $MSG)").unwrap();
        let src = "package main\n\nfunc f() error {\n\treturn errors.Wrap(errors.Wrap(err, \"a\"), \"b\")\n}\n";
        let tpl = "fmt.Errorf(\"%s: %w\", $MSG, $ERR)";
        let out = p.rewrite(src, tpl);
        assert!(
            out.contains(
                "return fmt.Errorf(\"%s: %w\", \"b\", fmt.Errorf(\"%s: %w\", \"a\", err))"
            ),
            "{out}"
        );
        assert_eq!(p.rewrite(&out, tpl), out, "idempotent");
    }

    #[test]
    fn nested_multi_captures_and_wrap_templates_rewrite_each_original_match_once() {
        let p = StructuralPattern::compile(&go(), "f($$$A)").unwrap();
        let src = "package main\n\nfunc main() {\n\tf(1, f(2))\n}\n";
        let out = p.rewrite(src, "f(g($$$A))");
        assert!(out.contains("f(g(1, f(g(2))))"), "{out}");
    }

    #[test]
    fn template_dollars_that_are_not_metavariables_survive() {
        assert_eq!(
            transformable_template("$this->x($$$ARGS, $A, \"$5\", $$)"),
            (
                "$this->x($ARGS, $A, \"$5\", $$)".to_string(),
                vec!["ARGS".to_string(), "A".to_string()]
            )
        );
    }

    #[test]
    fn repeated_metavariable_must_bind_equal_text() {
        let p = StructuralPattern::compile(&python(), "$A == $A").unwrap();
        assert_eq!(p.find("x == x\nx == y\n").len(), 1);
    }

    #[test]
    fn delete_removes_whole_statement_lines() {
        let p = StructuralPattern::compile(&go(), "log.Println($$$ARGS)").unwrap();
        let out = p.delete(GO_SRC);
        assert!(!out.contains("log.Println"));
        assert!(!out.contains("\n\t\n"), "no blank line left: {out:?}");
    }

    #[test]
    fn delete_takes_a_list_elements_comma_and_leaves_other_nested_matches() {
        let p = StructuralPattern::compile(&python(), "print($X)").unwrap();
        let src = "print(0)\ny = [print(1), 2]\nz = [3, print(4)]\nn = len(print(5))\n";
        assert_eq!(p.delete(src), "y = [2]\nz = [3]\nn = len(print(5))\n");
        // Only the trailing comma when it is the sole element; nested
        // matches inside a removed element go with it.
        let q = StructuralPattern::compile(&go(), "f($$$A)").unwrap();
        let go_src = "package p\nfunc g() { h(f(1,), 2); h(f(f(3)), 4) }\n";
        assert_eq!(q.delete(go_src), "package p\nfunc g() { h(2); h(4) }\n");
        let sole = "package p\nfunc g() { h(f(1,)) }\n";
        assert_eq!(q.delete(sole), sole);
    }

    #[test]
    fn a_rewrite_that_would_break_the_syntax_is_not_applied() {
        let rule = PatternRule::compile(
            &python(),
            "foo($A)",
            PatternAction::Replace("bar($A".into()),
            &[],
        )
        .unwrap();
        let src = "x = foo(1)\n";
        assert_eq!(rule.apply(src), src);
        // A sound template on the same input still applies.
        let ok = PatternRule::compile(
            &python(),
            "foo($A)",
            PatternAction::Replace("bar($A)".into()),
            &[],
        )
        .unwrap();
        assert_eq!(ok.apply(src), "x = bar(1)\n");
    }

    #[test]
    fn an_already_broken_file_is_still_rewritten() {
        // The guard protects clean files; it must not freeze files that
        // were already unparseable (tree-sitter recovers around errors).
        let ok = PatternRule::compile(
            &python(),
            "foo($A)",
            PatternAction::Replace("bar($A)".into()),
            &[],
        )
        .unwrap();
        assert_eq!(ok.apply("x = foo(1)\ndef (\n"), "x = bar(1)\ndef (\n");
    }

    #[test]
    fn invalid_pattern_names_every_attempt() {
        let err = StructuralPattern::compile(&go(), "func (").unwrap_err();
        assert!(err.contains("as-is"), "{err}");
        assert!(err.contains("func _()"), "{err}");
    }

    #[test]
    fn python_statement_pattern_needs_no_context() {
        let p = StructuralPattern::compile(&python(), "self.assertEquals($A, $B)").unwrap();
        let out = p.rewrite(
            "class T:\n    def test(self):\n        self.assertEquals(f(1), 2)\n",
            "self.assertEqual($A, $B)",
        );
        assert!(out.contains("self.assertEqual(f(1), 2)"), "{out}");
        assert_eq!(p.prefilter(), Some("assertEquals"));
    }

    #[test]
    fn all_metavariable_pattern_has_no_prefilter() {
        let p = StructuralPattern::compile(&python(), "$A + $B").unwrap();
        assert_eq!(p.prefilter(), None);
    }

    #[test]
    fn pattern_tree_shows_metavariables_and_the_context_used() {
        let p = StructuralPattern::compile(&go(), "errors.Wrap($ERR, $MSG)").unwrap();
        assert!(p.context().unwrap().contains("func _()"));
        let tree = p.tree();
        assert!(tree.starts_with("call_expression"), "{tree}");
        assert!(tree.contains("\"$ERR\""), "{tree}");
        assert!(tree.contains("field: field_identifier"), "{tree}");
    }

    #[test]
    fn syntax_tree_reports_errors() {
        let grammar: tree_sitter::Language = tree_sitter_go::LANGUAGE.into();
        let (tree, has_error) = syntax_tree(&grammar, "package p\nfunc f() { x( }\n").unwrap();
        assert!(has_error);
        assert!(tree.contains("ERROR") || tree.contains("MISSING"), "{tree}");
        let (_, has_error) = syntax_tree(&grammar, "package p\n").unwrap();
        assert!(!has_error);
    }

    #[test]
    fn bare_pattern_tree_starts_at_the_matched_node() {
        let p = StructuralPattern::compile(&python(), "foo.bar($A)").unwrap();
        assert!(p.context().is_none());
        assert!(p.tree().starts_with("call"), "{}", p.tree());
    }

    #[test]
    fn a_context_whose_wrapper_shadows_the_pattern_kind_is_refused() {
        // `x` alone selects an `identifier`; in Go's function-body wrapper the
        // function name `_` is an earlier `identifier`, so using that context
        // would silently match `_` instead. It must be refused, not used.
        let err = StructuralPattern::compile(&go(), "x").unwrap_err();
        assert!(err.contains("earlier `identifier`"), "{err}");
    }

    #[test]
    fn compiled_pattern_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<StructuralPattern>();
    }
}
