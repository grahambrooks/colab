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
}

/// ast-grep's view of a [`PatternLanguage`].
#[derive(Clone)]
struct TsLang {
    grammar: tree_sitter::Language,
    expando: char,
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
        grep.root()
            .find_all(&self.pattern)
            .filter(|m| self.shape.fits(m.get_node()))
            .filter(|m| self.checks.iter().all(|c| c.holds(m, source)))
            .collect()
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
        self.matches(&grep, source)
            .into_iter()
            .map(|m| {
                let range = m.range();
                let (line, column) = line_column(source, range.start);
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
        let mut spans: Vec<(usize, usize)> = Vec::new();
        for found in self.find(source) {
            let (line_start, line_end) = line_span(source, found.start, found.end);
            let before = &source[line_start..found.start];
            let after = &source[found.end..line_end];
            let alone = before.trim().is_empty()
                && after
                    .trim()
                    .trim_start_matches([';', ','])
                    .trim()
                    .is_empty();
            if alone {
                spans.push((line_start, line_end));
                continue;
            }
            let tree = tree.get_or_insert_with(|| {
                let mut parser = Parser::new();
                parser
                    .set_language(&self.lang.grammar)
                    .ok()
                    .and_then(|()| parser.parse(source, None))
            });
            if let Some(span) = tree
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

/// Fills a template for one match after rewriting the matches nested in
/// its bindings — see [`StructuralPattern::rewrite`].
struct Rewriter<'a, 't> {
    pattern: &'a Pattern,
    fix: &'a TemplateFix,
    lets: &'a [Derived],
    /// The rewriter of each `let` that has a `rewrite` step, by position:
    /// one per form its pattern compiled to (empty without one).
    subs: &'a [Vec<Rewriter<'a, 't>>],
    /// Every match in the source, nested ones included.
    matches: &'a [NodeMatch<'t, StrDoc<TsLang>>],
}

impl Rewriter<'_, '_> {
    /// Indices of the matches inside `span` that no other match inside it
    /// contains, in source order. `outer` is the match whose binding
    /// `span` is, so it does not count as nested in itself; of several
    /// matches over the same bytes (a call and its expression statement),
    /// the first is kept.
    fn outermost(&self, span: std::ops::Range<usize>, outer: Option<usize>) -> Vec<usize> {
        let inside: Vec<(usize, std::ops::Range<usize>)> = self
            .matches
            .iter()
            .enumerate()
            .filter(|&(i, _)| Some(i) != outer)
            .map(|(i, m)| (i, m.range()))
            .filter(|(_, r)| r.start >= span.start && r.end <= span.end)
            .collect();
        let mut out: Vec<(usize, std::ops::Range<usize>)> = Vec::new();
        for (i, r) in &inside {
            let covered = inside.iter().any(|(j, other)| {
                let contains = other.start <= r.start && r.end <= other.end;
                contains && (other != r || j < i)
            });
            if !covered {
                out.push((*i, r.clone()));
            }
        }
        out.sort_by_key(|(_, r)| r.start);
        out.into_iter().map(|(i, _)| i).collect()
    }

    /// `source[span]` with each outermost match inside it rewritten.
    fn rewrite_span(&self, source: &str, span: std::ops::Range<usize>, outer: usize) -> String {
        let mut out = String::new();
        let mut at = span.start;
        for i in self.outermost(span.clone(), Some(outer)) {
            let edit = self.render(source, i);
            if edit.start < at {
                continue;
            }
            out.push_str(&source[at..edit.start]);
            out.push_str(&edit.text);
            at = edit.end;
        }
        out.push_str(&source[at..span.end]);
        out
    }

    /// The edit replacing match `i`, its bindings rewritten first.
    fn render(&self, source: &str, i: usize) -> Edit {
        let mut nm = self.matches[i].clone();
        let vars: Vec<MetaVariable> = nm.get_env().get_matched_variables().collect();
        // Each binding's (rewritten) text and source span, for the `let`s
        // derived from it.
        type Value = (String, MetaVariable, String, Option<std::ops::Range<usize>>);
        let mut values: Vec<Value> = Vec::new();
        for var in vars {
            let (name, span) = match &var {
                MetaVariable::Capture(name, _) => {
                    let span = nm.get_env().get_match(name).map(ast_grep_core::Node::range);
                    (name.clone(), span)
                }
                MetaVariable::MultiCapture(name) => {
                    let nodes = nm.get_env().get_multiple_matches(name);
                    let span = match (nodes.first(), nodes.last()) {
                        // A sequence can end in a terminator that is only
                        // whitespace (Go's newline after a statement);
                        // the template's own text follows it instead.
                        (Some(first), Some(last)) => {
                            let (start, end) = (first.range().start, last.range().end);
                            Some(start..start + source[start..end].trim_end().len())
                        }
                        _ => None,
                    };
                    (name.clone(), span)
                }
                _ => continue,
            };
            let text = span
                .clone()
                .map(|span| self.rewrite_span(source, span, i))
                .unwrap_or_default();
            nm.get_env_mut()
                .insert_transformation(&var, &name, text.clone().into_bytes());
            values.push((name, var, text, span));
        }
        for (k, derived) in self.lets.iter().enumerate() {
            let Some((_, var, text, span)) = values.iter().find(|(n, ..)| *n == derived.from)
            else {
                continue;
            };
            // A rewriter starts from the binding's source: each of its
            // matches there rewritten, the text between them kept.
            let start = match (self.subs.get(k), span) {
                (Some(forms), Some(span)) if !forms.is_empty() => {
                    rewrite_with(forms, source, span.clone())
                }
                _ => text.clone(),
            };
            let (var, text) = (var.clone(), derived.apply(&start));
            nm.get_env_mut()
                .insert_transformation(&var, &derived.var, text.clone().into_bytes());
            values.push((derived.var.clone(), var, text, None));
        }
        let edit = nm.make_edit(self.pattern, self.fix);
        let text = String::from_utf8_lossy(&edit.inserted_text);
        let text = indent_with_tabs(source, edit.position, &text);
        let (start, end) = (edit.position, edit.position + edit.deleted_length);
        let text = keep_layout(&source[start..end], &strip_blank_line_indent(&text));
        Edit::new(start, end, text)
    }
}

/// `new` (a rendered template) with the whitespace of `old` (the text it
/// replaces) around every token the two share.
///
/// A template replaces the whole match in its own layout, so
/// `func ($R $T) $F() { $$$B }` → `func ($R $T) $NAME() { $$$B }` would
/// put a many-line body on one line. Tokens are aligned (quoted text is
/// one token, so a string's spaces are never mixed): an unchanged token
/// keeps the whitespace before it in `old`, unless it follows a change on
/// the same line, or the template breaks the line there; a new one keeps the whitespace
/// before it in `new`. A rewrite then changes what it changes and leaves
/// the layout around it, as a layout-only rewrite is not made at all
/// ([`same_but_whitespace`]).
fn keep_layout(old: &str, new: &str) -> String {
    let (a, b) = (layout_tokens(old), layout_tokens(new));
    if a.is_empty() || b.is_empty() {
        return new.to_string();
    }
    let ta: Vec<&str> = a.iter().map(|t| t.1).collect();
    let tb: Vec<&str> = b.iter().map(|t| t.1).collect();
    let ops = similar::capture_diff_slices(similar::Algorithm::Myers, &ta, &tb);
    let mut out = String::with_capacity(new.len());
    // Whose layout the line being written follows: the original's, once a
    // line of it is kept, or the template's (from the start, and after a
    // line the template made).
    let mut original_line = false;
    let emit = |out: &mut String, line: &mut bool, space: &str, token: &str, original: bool| {
        if space.contains('\n') {
            *line = original;
        }
        out.push_str(space);
        out.push_str(token);
    };
    for op in ops {
        let (o, n) = (op.old_range(), op.new_range());
        if !matches!(op.tag(), similar::DiffTag::Equal) {
            for (k, (space, token)) in b[n.clone()].iter().enumerate() {
                // A token that replaces others takes their place: the
                // original's whitespace before the first of them (not at
                // the edit's start, which sits where the match did, and
                // not where there was none, so `x+y` → `x in y` keeps the
                // template's spaces).
                let in_place = k == 0 && n.start != 0 && !o.is_empty() && !a[o.start].0.is_empty();
                if in_place {
                    emit(&mut out, &mut original_line, a[o.start].0, token, true);
                } else {
                    emit(&mut out, &mut original_line, space, token, false);
                }
            }
            continue;
        }
        let new_start = n.start;
        for (k, ((old_space, token), (new_space, _))) in a[o.clone()].iter().zip(&b[n]).enumerate()
        {
            let (old_break, new_break) = (old_space.contains('\n'), new_space.contains('\n'));
            let next_to_change = k == 0 && (o.start != 0 || new_start != 0);
            let original = if new_start + k == 0 {
                // The edit's first token sits where the match did.
                false
            } else if next_to_change && !old_break {
                // The original's space belonged to a token that is gone,
                // and without a line break nothing of its layout is lost.
                false
            } else if old_break && new_break {
                // A line both break: as the line before it went, so a
                // block keeps its indentation and a re-indented one
                // stays re-indented.
                original_line
            } else {
                // Where only the template breaks the line, its layout
                // stands; elsewhere (a closing brace on its own line, a
                // body a one-line template would join) the original's.
                !new_break
            };
            let space = if original { old_space } else { new_space };
            emit(&mut out, &mut original_line, space, token, original);
        }
    }
    // Trailing whitespace belongs to whichever text has the last word.
    out.push_str(&new[new.trim_end().len()..]);
    out
}

/// `text` as `(whitespace before, token)` pairs: a quoted string, a run
/// of identifier characters, or any other single character.
fn layout_tokens(text: &str) -> Vec<(&str, &str)> {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = Vec::new();
    let mut chars = text.char_indices().peekable();
    let mut space_start = 0;
    while let Some(&(i, c)) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        let space = &text[space_start..i];
        let mut end = i + c.len_utf8();
        chars.next();
        if matches!(c, '"' | '\'' | '`') {
            let mut escaped = false;
            for (j, d) in chars.by_ref() {
                end = j + d.len_utf8();
                if escaped {
                    escaped = false;
                } else if d == '\\' {
                    escaped = true;
                } else if d == c || d == '\n' {
                    break;
                }
            }
        } else if word(c) {
            while let Some(&(j, d)) = chars.peek() {
                if !word(d) {
                    break;
                }
                end = j + d.len_utf8();
                chars.next();
            }
        }
        out.push((space, &text[i..end]));
        space_start = end;
    }
    out
}

/// `source[span]` with every outermost match of any of `forms` (one
/// rewriter's pattern as each context compiled it) rewritten, the text
/// between them kept; of two overlapping edits the outer one is made.
fn rewrite_with(forms: &[Rewriter<'_, '_>], source: &str, span: std::ops::Range<usize>) -> String {
    let mut edits: Vec<Edit> = forms
        .iter()
        .flat_map(|form| {
            form.outermost(span.clone(), None)
                .into_iter()
                .map(|i| form.render(source, i))
        })
        .collect();
    edits.sort_by_key(|e| (e.start, std::cmp::Reverse(e.end)));
    let mut out = String::new();
    let mut at = span.start;
    for edit in edits {
        if edit.start < at {
            continue;
        }
        out.push_str(&source[at..edit.start]);
        out.push_str(&edit.text);
        at = edit.end;
    }
    out.push_str(&source[at..span.end]);
    out
}

/// ast-grep re-indents a multi-line template's later lines to the
/// match's column with spaces, and not at all when the line is indented
/// with tabs. In that case give each later non-empty line the matched
/// line's own indentation, tabs and all.
fn indent_with_tabs(source: &str, at: usize, text: &str) -> String {
    let line_start = source[..at].rfind('\n').map_or(0, |i| i + 1);
    let indent: String = source[line_start..at]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    if !indent.contains('\t') || !text.contains('\n') {
        return text.to_string();
    }
    let mut lines = text.split_inclusive('\n');
    let mut out = lines.next().unwrap_or_default().to_string();
    for line in lines {
        if !line.trim().is_empty() {
            out.push_str(&indent);
        }
        out.push_str(line);
    }
    out
}

/// ast-grep indents every line of a multi-line template to the match's
/// column, blank ones included, which leaves trailing whitespace. Empty
/// those lines again.
fn strip_blank_line_indent(text: &str) -> String {
    if !text.contains('\n') {
        return text.to_string();
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let last = lines.len() - 1;
    lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            // The first line follows the match's own prefix and the last
            // precedes the rest of its line; only lines in between are
            // whole lines.
            if i != 0 && i != last && line.trim().is_empty() {
                ""
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `template` with every `$$$NAME` written as `$NAME`, and the names it
/// uses — so ast-grep takes each binding's value from the rewritten text
/// [`Rewriter::render`] supplies instead of from the original source.
fn transformable_template(template: &str) -> (String, Vec<String>) {
    let mut out = String::with_capacity(template.len());
    let mut names: Vec<String> = Vec::new();
    let mut rest = template;
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let run = tail.chars().take_while(|&c| c == '$').count().min(3);
        let after = &tail[run..];
        let len = after
            .find(|c: char| !(c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit()))
            .unwrap_or(after.len());
        let name = &after[..len];
        let is_var = matches!(run, 1 | 3) && after.starts_with(is_var_start);
        if is_var {
            out.push('$');
            out.push_str(name);
            if !names.iter().any(|n| n == name) {
                names.push(name.to_string());
            }
            rest = &after[len..];
        } else {
            out.push_str(&tail[..run]);
            rest = after;
        }
    }
    out.push_str(rest);
    (out, names)
}

/// The metavariables `pattern` binds, in order of first use. `$_` and a
/// bare `$$$` bind nothing.
fn metavariables(pattern: &str) -> Vec<Var> {
    let mut out: Vec<Var> = Vec::new();
    let mut rest = pattern;
    while let Some(i) = rest.find('$') {
        let tail = &rest[i..];
        let run = tail.chars().take_while(|&c| c == '$').count();
        let after = &tail[run..];
        let len = after
            .find(|c: char| !(c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit()))
            .unwrap_or(after.len());
        let name = &after[..len];
        if matches!(run, 1 | 3)
            && name != "_"
            && after.starts_with(is_var_start)
            && !out.iter().any(|v| v.name == name)
        {
            out.push(Var {
                name: name.to_string(),
                multi: run == 3,
            });
        }
        rest = &after[len..];
    }
    out
}

/// A template metavariable the pattern does not bind (nor a `let`) would
/// be filled with nothing; refuse it. A `where` pattern's metavariables
/// are its own, so `$F` from `follows '$V, err := $F($$$A)'` is not one.
fn unbound_in_template(
    pattern: &str,
    template: &str,
    constraints: &[Constraint],
) -> Result<(), String> {
    let bound = metavariables(pattern);
    let lets: Vec<&str> = constraints
        .iter()
        .filter_map(|c| match &c.test {
            Test::Let { var, .. } => Some(var.name.as_str()),
            _ => None,
        })
        .collect();
    let (_, names) = transformable_template(template);
    let Some(name) = names
        .iter()
        .find(|n| !bound.iter().any(|b| b.name == **n) && !lets.contains(&n.as_str()))
    else {
        return Ok(());
    };
    let mut known: Vec<String> = bound.iter().map(ToString::to_string).collect();
    known.extend(lets.iter().map(|l| format!("${l}")));
    let binds = if known.is_empty() {
        "binds none".to_string()
    } else {
        format!("binds {}", known.join(", "))
    };
    Err(format!(
        "the template uses `${name}`, which the pattern does not bind ({binds}), so it would be \
         filled with nothing; a `where` pattern's metavariables are its own"
    ))
}

/// One compiled `let`: `var` is `from`'s text through `steps`.
#[derive(Debug)]
struct Derived {
    var: String,
    from: String,
    /// A first `rewrite(<name>, …)` step: each rewriter's pattern,
    /// compiled for this language, and its template (empty without one).
    rewriters: Vec<(StructuralPattern, String)>,
    steps: Vec<crate::derive::Step>,
}

impl Derived {
    /// The `let`s among `constraints`, in order. Each derives from a
    /// metavariable the pattern binds or an earlier `let`, and names a
    /// new `$VAR`, written with one `$`: it holds text, not nodes.
    fn compile_all(
        language: &PatternLanguage,
        bound: &[Var],
        constraints: &[Constraint],
    ) -> Result<Vec<Self>, String> {
        let mut out: Vec<Self> = Vec::new();
        for c in constraints {
            let Test::Let { var, from, steps } = &c.test else {
                continue;
            };
            if var.multi {
                return Err(format!(
                    "`{c}`: a `let` holds text, so it is named with one `$`: `${}`",
                    var.name
                ));
            }
            if let Some(b) = bound.iter().find(|b| b.name == var.name) {
                return Err(format!(
                    "`{c}`: the pattern already binds `{b}`; name the derived value something else"
                ));
            }
            if out.iter().any(|d| d.var == var.name) {
                return Err(format!("`{c}`: `{var}` is already a `let`"));
            }
            let earlier = !from.multi && out.iter().any(|d| d.var == from.name);
            if !earlier {
                require_bound(bound, from).map_err(|e| format!("`{c}`: {e}"))?;
            }
            let (rewriters, rest) = match steps.split_first() {
                Some((colab_core::Transform::Rewrite(list), rest)) => {
                    if earlier {
                        return Err(format!(
                            "`{c}`: a rewriter works on the code a metavariable binds, so `rewrite` takes `{from}` from the pattern, not from a `let`"
                        ));
                    }
                    let compiled = list
                        .iter()
                        .map(|r| {
                            unbound_in_template(&r.pattern, &r.template, &[])
                                .and_then(|()| StructuralPattern::compile(language, &r.pattern))
                                .map(|sub| (sub, r.template.clone()))
                                .map_err(|e| format!("`rewriter {}`: {e}", r.name))
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    (compiled, rest)
                }
                _ => (Vec::new(), steps.as_slice()),
            };
            if let Some(late) = rest
                .iter()
                .find(|s| matches!(s, colab_core::Transform::Rewrite(_)))
            {
                return Err(format!(
                    "`{c}`: `{late}` must be the first step: a rewriter works on the code, not on text another step made"
                ));
            }
            out.push(Self {
                var: var.name.clone(),
                from: from.name.clone(),
                rewriters,
                steps: rest
                    .iter()
                    .map(crate::derive::Step::compile)
                    .collect::<Result<_, _>>()?,
            });
        }
        Ok(out)
    }

    fn apply(&self, text: &str) -> String {
        self.steps
            .iter()
            .fold(text.to_string(), |text, step| step.apply(&text))
    }
}

/// One compiled `where` constraint.
struct Check {
    negated: bool,
    kind: CheckKind,
}

enum CheckKind {
    Matches {
        var: Var,
        regex: regex::Regex,
    },
    Is {
        name: String,
        kind: String,
    },
    Count {
        name: String,
        min: usize,
        max: Option<usize>,
    },
    Inside(Forms),
    Has(Forms),
    Related {
        relation: Relation,
        pattern: Forms,
        stop: StopAt,
    },
    Any(Vec<Check>),
    All(Vec<Check>),
    Nth(i64),
    Field(String),
    /// Read from the scan phases' facts, through `colab_core::facts`.
    In {
        var: Var,
        set: String,
        field: String,
    },
    /// Asked of the language server, through `colab_core::semantic`.
    Semantic {
        var: Var,
        query: colab_core::semantic::Query,
    },
}

/// A compiled [`Stop`].
enum StopAt {
    Anywhere,
    Directly,
    Until(Forms),
}

/// A nested pattern as every context compiled it: `retries: $N` is a
/// label on its own and a property in an object, and a nested pattern
/// matches either.
struct Forms(Vec<Pattern>);

impl Forms {
    fn match_node(&self, node: &TsNode<'_>) -> bool {
        self.0.iter().any(|p| node.matches(p))
    }
}

type TsNode<'t> = ast_grep_core::Node<'t, StrDoc<TsLang>>;

/// `node`, or the outermost ancestor spanning exactly the same text: a
/// matched call is also its expression statement, so its siblings are the
/// statements around it.
fn widest<'t>(node: &TsNode<'t>) -> TsNode<'t> {
    let mut n = node.clone();
    while let Some(parent) = n.parent() {
        if parent.range() != n.range() {
            break;
        }
        n = parent;
    }
    n
}

/// `node` and each ancestor spanning the same text, innermost first.
fn same_span<'t>(node: &TsNode<'t>) -> Vec<TsNode<'t>> {
    let mut out = vec![node.clone()];
    while let Some(parent) = out.last().and_then(ast_grep_core::Node::parent) {
        if parent.range() != node.range() {
            break;
        }
        out.push(parent);
    }
    out
}

/// Whether some node related to `node` matches `pattern`, searched
/// nearest first and no further than `stop`.
fn related(node: &TsNode<'_>, relation: Relation, pattern: &Forms, stop: &StopAt) -> bool {
    // Candidates nearest first, and whether each is the last `directly`
    // allows.
    let search = |candidates: &mut dyn Iterator<Item = TsNode<'_>>,
                  last: &dyn Fn(&TsNode<'_>) -> bool| {
        for c in candidates {
            if pattern.match_node(&c) {
                return true;
            }
            match stop {
                StopAt::Anywhere => {}
                StopAt::Directly if last(&c) => return false,
                StopAt::Directly => {}
                StopAt::Until(until) if until.match_node(&c) => return false,
                StopAt::Until(_) => {}
            }
        }
        false
    };
    let real = |n: &TsNode<'_>| n.is_named() && !n.is_extra();
    match relation {
        // `directly`: up to the first enclosing node that is larger than
        // the match (wrappers over the same text do not count).
        Relation::Inside => search(&mut node.ancestors(), &|a: &TsNode<'_>| {
            a.range() != node.range()
        }),
        Relation::Has => match stop {
            StopAt::Directly => node.children().any(|c| pattern.match_node(&c)),
            StopAt::Anywhere => node.dfs().skip(1).any(|n| pattern.match_node(&n)),
            // Depth first, not descending into a node matching `until`.
            StopAt::Until(until) => {
                let mut stack: Vec<TsNode<'_>> = node.children().collect();
                while let Some(n) = stack.pop() {
                    if pattern.match_node(&n) {
                        return true;
                    }
                    if !until.match_node(&n) {
                        stack.extend(n.children());
                    }
                }
                false
            }
        },
        Relation::Follows => {
            let base = widest(node);
            search(&mut base.prev_all().filter(real), &|_: &TsNode<'_>| true)
        }
        Relation::Precedes => {
            let base = widest(node);
            search(&mut base.next_all().filter(real), &|_: &TsNode<'_>| true)
        }
    }
}

/// Whether `node` is its parent's `n`th named child (negative: from the end).
fn is_nth(node: &TsNode<'_>, n: i64) -> bool {
    let base = widest(node);
    let Some(parent) = base.parent() else {
        return false;
    };
    let siblings: Vec<_> = parent
        .children()
        .filter(|c| c.is_named() && !c.is_extra())
        .map(|c| c.range())
        .collect();
    let Some(at) = siblings.iter().position(|r| *r == base.range()) else {
        return false;
    };
    let len = siblings.len() as i64;
    let at = at as i64;
    if n > 0 { at + 1 == n } else { len - at == -n }
}

/// Whether `node` (or a node spanning the same text) is its parent's
/// child in field `name`.
fn in_field(node: &TsNode<'_>, name: &str) -> bool {
    same_span(node).iter().any(|c| {
        c.parent().is_some_and(|parent| {
            parent
                .field_children(name)
                .any(|f| f.range() == c.range() && f.kind() == c.kind())
        })
    })
}

impl Check {
    fn compile(
        language: &PatternLanguage,
        bound: &[Var],
        constraint: &Constraint,
    ) -> Result<Self, String> {
        let kind = match &constraint.test {
            Test::Matches { var, regex } => {
                require_bound(bound, var)?;
                // `/` ends the DSL's regex literal, so it is written `\/`.
                let regex = regex::Regex::new(&regex.replace(r"\/", "/"))
                    .map_err(|e| format!("`{var} matches /{regex}/`: {e}"))?;
                CheckKind::Matches {
                    var: var.clone(),
                    regex,
                }
            }
            Test::Is { var, kind } => {
                require_bound(bound, var)?;
                if var.multi {
                    return Err(format!(
                        "`{var} is {kind}`: `is` checks one node; bind it as `${}`",
                        var.name
                    ));
                }
                if language.grammar.id_for_node_kind(kind, true) == 0 {
                    let kinds = named_kinds(&language.grammar);
                    let hint = colab_core::suggest::closest(kind, kinds.iter().copied())
                        .map(|k| format!(" (did you mean `{k}`?)"))
                        .unwrap_or_default();
                    return Err(format!(
                        "`{var} is {kind}`: the grammar has no node kind `{kind}`{hint}; `colab ast` shows the kinds in a snippet"
                    ));
                }
                CheckKind::Is {
                    name: var.name.clone(),
                    kind: kind.clone(),
                }
            }
            Test::Count { var, min, max } => {
                require_bound(bound, var)?;
                if !var.multi {
                    return Err(format!(
                        "`{var} count`: `count` measures a sequence; bind it as `$$${}`",
                        var.name
                    ));
                }
                if max.is_some_and(|max| max < *min) {
                    return Err(format!("`{constraint}`: the range is empty"));
                }
                CheckKind::Count {
                    name: var.name.clone(),
                    min: *min,
                    max: *max,
                }
            }
            Test::Inside(text) => CheckKind::Inside(nested(language, "inside", text)?),
            Test::Has(text) => CheckKind::Has(nested(language, "has", text)?),
            Test::Related {
                relation,
                pattern,
                stop,
            } => CheckKind::Related {
                relation: *relation,
                pattern: nested(language, relation.keyword(), pattern)?,
                stop: match stop {
                    Stop::Anywhere => StopAt::Anywhere,
                    Stop::Directly => StopAt::Directly,
                    Stop::Until(until) => StopAt::Until(nested(language, "until", until)?),
                },
            },
            Test::Any(all) | Test::All(all) => {
                if all.is_empty() {
                    return Err(format!("`{constraint}` needs at least one constraint"));
                }
                let checks = all
                    .iter()
                    .map(|c| Self::compile(language, bound, c))
                    .collect::<Result<Vec<_>, _>>()?;
                if matches!(constraint.test, Test::Any(_)) {
                    CheckKind::Any(checks)
                } else {
                    CheckKind::All(checks)
                }
            }
            Test::Nth(0) => {
                return Err(format!(
                    "`{constraint}`: positions count from 1 (`nth 1` is the first, `nth -1` the last)"
                ));
            }
            Test::Nth(n) => CheckKind::Nth(*n),
            Test::In { var, set, field } => {
                require_bound(bound, var)?;
                CheckKind::In {
                    var: var.clone(),
                    set: set.clone(),
                    field: field
                        .as_ref()
                        .map_or_else(|| var.name.clone(), |f| f.name.clone()),
                }
            }
            Test::Semantic { var, query } => {
                require_bound(bound, var)?;
                CheckKind::Semantic {
                    var: var.clone(),
                    query: query.clone(),
                }
            }
            Test::Field(name) => {
                if language.grammar.field_id_for_name(name).is_none() {
                    let fields: Vec<&str> = (1..=language.grammar.field_count() as u16)
                        .filter_map(|id| language.grammar.field_name_for_id(id))
                        .collect();
                    let hint = colab_core::suggest::closest(name, fields.iter().copied())
                        .map(|f| format!(" (did you mean `{f}`?)"))
                        .unwrap_or_default();
                    return Err(format!(
                        "`{constraint}`: the grammar has no field `{name}`{hint}; `colab ast` shows the fields in a snippet"
                    ));
                }
                CheckKind::Field(name.clone())
            }
            Test::Let { .. } => unreachable!("`let` is compiled by `Derived`"),
        };
        Ok(Self {
            negated: constraint.negated,
            kind,
        })
    }

    /// Whether match `m` in `source` satisfies this constraint.
    fn holds(&self, m: &NodeMatch<'_, StrDoc<TsLang>>, source: &str) -> bool {
        let env = m.get_env();
        let result = match &self.kind {
            CheckKind::Matches { var, regex } => {
                let text = if var.multi {
                    let nodes = env.get_multiple_matches(&var.name);
                    match (nodes.first(), nodes.last()) {
                        (Some(first), Some(last)) => {
                            source[first.range().start..last.range().end].to_string()
                        }
                        _ => String::new(),
                    }
                } else {
                    env.get_match(&var.name)
                        .map(|n| n.text().into_owned())
                        .unwrap_or_default()
                };
                regex.is_match(&text)
            }
            CheckKind::Is { name, kind } => env.get_match(name).is_some_and(|n| n.kind() == *kind),
            CheckKind::Count { name, min, max } => {
                // Separators (`,`) are unnamed nodes in the sequence; count
                // what a reader would count.
                let n = env
                    .get_multiple_matches(name)
                    .iter()
                    .filter(|n| n.is_named())
                    .count();
                n >= *min && max.is_none_or(|max| n <= max)
            }
            CheckKind::Inside(forms) => m.get_node().ancestors().any(|a| forms.match_node(&a)),
            CheckKind::Has(forms) => m.get_node().dfs().skip(1).any(|n| forms.match_node(&n)),
            CheckKind::Related {
                relation,
                pattern,
                stop,
            } => related(m.get_node(), *relation, pattern, stop),
            CheckKind::Any(checks) => checks.iter().any(|c| c.holds(m, source)),
            CheckKind::All(checks) => checks.iter().all(|c| c.holds(m, source)),
            CheckKind::Nth(n) => is_nth(m.get_node(), *n),
            CheckKind::Field(name) => in_field(m.get_node(), name),
            CheckKind::In { var, set, field } => {
                let text = if var.multi {
                    let nodes = env.get_multiple_matches(&var.name);
                    match (nodes.first(), nodes.last()) {
                        (Some(first), Some(last)) => {
                            source[first.range().start..last.range().end].to_string()
                        }
                        _ => String::new(),
                    }
                } else {
                    env.get_match(&var.name)
                        .map(|n| n.text().into_owned())
                        .unwrap_or_default()
                };
                colab_core::facts::contains(set, field, &text)
            }
            CheckKind::Semantic { var, query } => {
                let span = if var.multi {
                    let nodes = env.get_multiple_matches(&var.name);
                    match (nodes.first(), nodes.last()) {
                        (Some(first), Some(last)) => Some(first.range().start..last.range().end),
                        _ => None,
                    }
                } else {
                    env.get_match(&var.name).map(ast_grep_core::Node::range)
                };
                // An empty sequence has no code to ask about.
                span.is_some_and(|span| colab_core::semantic::ask(source, span, query))
            }
        };
        result != self.negated
    }
}

fn require_bound(bound: &[Var], var: &Var) -> Result<(), String> {
    match bound.iter().find(|b| b.name == var.name) {
        Some(b) if b.multi == var.multi => Ok(()),
        Some(b) => Err(format!(
            "`{var}` is written `{b}` in the pattern; use the same sigil"
        )),
        None if bound.is_empty() => Err(format!(
            "`{var}` is not a metavariable of the pattern, which binds none"
        )),
        None => {
            let names: Vec<String> = bound.iter().map(ToString::to_string).collect();
            Err(format!(
                "`{var}` is not a metavariable of the pattern; it binds {}",
                names.join(", ")
            ))
        }
    }
}

/// An `inside` / `has` pattern, compiled in the same language. Its
/// metavariables are its own: they are not unified with the outer
/// pattern's.
fn nested(language: &PatternLanguage, keyword: &str, text: &str) -> Result<Forms, String> {
    let compiled = StructuralPattern::compile(language, text)
        .map_err(|e| format!("`{keyword} '{text}'`: {e}"))?;
    let mut forms = vec![compiled.pattern];
    forms.extend(compiled.also.into_iter().map(|alt| alt.pattern));
    Ok(Forms(forms))
}

/// Every visible named node kind the grammar defines.
fn named_kinds(grammar: &tree_sitter::Language) -> Vec<&str> {
    let mut kinds: Vec<&str> = (0..grammar.node_kind_count() as u16)
        .filter(|&id| grammar.node_kind_is_named(id) && grammar.node_kind_is_visible(id))
        .filter_map(|id| grammar.node_kind_for_id(id))
        .collect();
    kinds.sort_unstable();
    kinds.dedup();
    kinds
}

/// The body of every source backend's
/// [`LanguageBackend::build_rule_where`]: a block without constraints is
/// an ordinary [`LanguageBackend::build_rule`]. On `pattern` (with
/// `replace` or `delete`) constraints are compiled by `compile` (the
/// backend's own `PatternOp::compile`); on `symbol` and `call` they may
/// only be `inside` / `not inside`, a syntactic scope ([`crate::scope`]).
pub fn build_rule_where<B, F>(
    backend: &B,
    module: &str,
    spec: RuleSpec,
    constraints: &[Constraint],
    compile: F,
) -> colab_core::Result<Box<dyn Operation>>
where
    B: LanguageBackend + ?Sized,
    F: FnOnce(&str, PatternAction, &[Constraint]) -> colab_core::Result<Box<dyn Operation>>,
{
    if constraints.is_empty() {
        return backend.build_rule(module, spec);
    }
    // `symbol` and `call` take a syntactic scope: `where inside '<pattern>'`.
    if matches!(module, "symbol" | "call") {
        let inner = backend.build_rule(module, spec)?;
        let scoped = crate::scope::Scoped::new(backend.lang(), module, inner, constraints, |p| {
            compile_scope(backend, p)
        })?;
        return Ok(Box::new(scoped));
    }
    if module != "pattern" {
        return Err(colab_core::backend::where_unsupported(
            backend.lang(),
            module,
        ));
    }
    let action = match &spec {
        RuleSpec::Replace { replacement, .. } => PatternAction::Replace(replacement.clone()),
        RuleSpec::Delete { .. } => PatternAction::Delete,
        _ => return Err(backend.unsupported(module, &spec)),
    };
    compile(spec.target(), action, constraints)
}

/// A scope pattern, compiled as the backend's own `<lang>::pattern` (with
/// `delete`, never applied: only its matches are used).
fn compile_scope<B: LanguageBackend + ?Sized>(
    backend: &B,
    pattern: &str,
) -> colab_core::Result<Box<dyn Operation>> {
    backend.build_rule(
        "pattern",
        RuleSpec::Delete {
            target: pattern.to_string(),
        },
    )
}

/// What a `<lang>::pattern` rule does with each match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatternAction {
    /// Replace the match with this template (`$NAME` / `$$$NAME`
    /// substituted).
    Replace(String),
    /// Remove the match (with its line, when it stands alone).
    Delete,
}

/// A compiled pattern plus its action: everything a backend's
/// `<lang>::pattern` operation does except decide which files it applies
/// to and print its namespace.
#[derive(Debug)]
pub struct PatternRule {
    pub pattern: StructuralPattern,
    pub action: PatternAction,
    /// How this language spells a qualified name, for
    /// [`conflicts`](PatternRule::conflicts). `None` skips that check.
    pub qualifiers: Option<Qualifiers>,
}

/// How a language spells a qualified reference such as `fmt.Errorf`, and
/// which qualifiers a file's imports bind. With it, a rewrite that
/// introduces a qualifier nothing imports is reported as a conflict.
#[derive(Clone, Copy)]
pub struct Qualifiers {
    /// The node kind of a qualified reference: `selector_expression` in
    /// Go, `attribute` in Python.
    pub kind: &'static str,
    /// The field holding the qualifier: `operand` in Go, `object` in
    /// Python. Only a qualifier that is a single leaf is checked.
    pub field: &'static str,
    /// Every name a file's imports bind, alias or conventional name. `*`
    /// means "cannot tell" (a wildcard import), which silences the check.
    pub imported: fn(&str) -> Vec<String>,
    /// Qualifiers that need no import: builtins such as Python's `str`.
    pub builtin: &'static [&'static str],
}

/// `true` when `template` spells `name.` with `name` as a whole word, so
/// `fmt.Errorf` qualifies by `fmt` but `xfmt.F` does not.
fn template_qualifies(template: &str, name: &str) -> bool {
    let dotted = format!("{name}.");
    template.match_indices(&dotted).any(|(at, _)| {
        !template[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$')
    })
}

impl std::fmt::Debug for Qualifiers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Qualifiers({}.{})", self.kind, self.field)
    }
}

impl PatternRule {
    /// Compile `text` with its `where` constraints (`&[]` for none).
    pub fn compile(
        language: &PatternLanguage,
        text: &str,
        action: PatternAction,
        constraints: &[Constraint],
    ) -> Result<Self, String> {
        if matches!(action, PatternAction::Delete)
            && let Some(c) = constraints.iter().find(|c| c.is_let())
        {
            return Err(format!(
                "`{c}`: a `let` derives a value for the `replace` template, and `delete` has none"
            ));
        }
        if let PatternAction::Replace(template) = &action {
            unbound_in_template(text, template, constraints)?;
        }
        Ok(Self {
            pattern: StructuralPattern::compile(language, text)?
                .constrain(language, constraints)?,
            action,
            qualifiers: None,
        })
    }

    /// Check rewrites for qualifiers nothing imports (see [`Qualifiers`]).
    pub fn with_qualifiers(mut self, qualifiers: Qualifiers) -> Self {
        self.qualifiers = Some(qualifiers);
        self
    }

    /// Why [`apply`](PatternRule::apply) leaves `source` unchanged although
    /// the pattern matches there: the rewritten file would not parse.
    /// A warning when the template, read as code with its metavariables
    /// in place, is itself a match: every run would rewrite its own output
    /// again (`f($A)` → `f(g($A))`). An identity template is a no-op and
    /// is not flagged.
    pub fn advice(&self) -> Vec<String> {
        let PatternAction::Replace(template) = &self.action else {
            return Vec::new();
        };
        if template.trim() == self.pattern.text().trim() || !self.pattern.rematches(template) {
            return Vec::new();
        }
        vec![
            "the template matches the pattern again, so every run rewrites its own output once \
             more; narrow the pattern (a `where` clause, or a name the template changes), or apply \
             it once and remove the rule"
                .into(),
        ]
    }

    pub fn why_unchanged(&self, source: &str) -> Option<String> {
        let PatternAction::Replace(template) = &self.action else {
            return None;
        };
        let out = self.pattern.rewrite(source, template);
        if out == source && !self.pattern.find(source).is_empty() {
            return Some(
                "the template gives back what it matched, apart from whitespace, so nothing was \
                 changed"
                    .into(),
            );
        }
        if out == source
            || self.pattern.parses_cleanly(&out)
            || !self.pattern.parses_cleanly(source)
        {
            return None;
        }
        Some(
            "the rewritten code would not parse, so it was skipped; check the template \
             (a `$$$` capture keeps its separators, so `$$$ARGS,` doubles a comma)"
                .into(),
        )
    }

    /// Qualifiers this rule's template introduced that the file neither
    /// used `before` nor imports `after` (the file as the whole script
    /// leaves it): `fmt.` in a template, with no `requires import "fmt"`
    /// and no later rule adding it. Syntactic, so it errs towards silence:
    /// a qualifier already present anywhere in `before`, as any leaf,
    /// counts as known.
    pub fn conflicts(&self, before: &str, after: &str) -> Vec<String> {
        let (Some(q), PatternAction::Replace(template)) = (self.qualifiers, &self.action) else {
            return Vec::new();
        };
        let grammar = &self.pattern.lang.grammar;
        let (Ok(old), Ok(new)) = (parse_clean(grammar, before), parse_clean(grammar, after)) else {
            return Vec::new();
        };
        let mut known: std::collections::HashSet<&str> = std::collections::HashSet::new();
        crate::visit_all(&old, |node| {
            if node.child_count() == 0 {
                known.insert(&before[node.byte_range()]);
            }
        });
        let imported = (q.imported)(after);
        let mut introduced: Vec<String> = Vec::new();
        crate::visit_all(&new, |node| {
            if node.kind() != q.kind {
                return;
            }
            let Some(qualifier) = node.child_by_field_name(q.field) else {
                return;
            };
            if qualifier.child_count() != 0 {
                return;
            }
            let name = &after[qualifier.byte_range()];
            // Only a qualifier this rule's own template spells is its doing;
            // `after` may hold later rules' changes too.
            if template_qualifies(template, name)
                && !known.contains(name)
                && !q.builtin.contains(&name)
                && !imported.iter().any(|i| i == "*" || i == name)
                && !introduced.iter().any(|i| i == name)
            {
                introduced.push(name.to_string());
            }
        });
        introduced
            .into_iter()
            .map(|name| {
                format!(
                    "the rewrite uses `{name}.`, which this file does not import; \
                     add `requires import \"…\"` to the rule"
                )
            })
            .collect()
    }

    /// Rewrite `source`. If it parsed cleanly and the rewrite would leave
    /// a syntax error — a template that is not valid code where it lands —
    /// the file is returned unchanged rather than written broken. The rule
    /// then reports no change for that file, while `find` still lists its
    /// matches: that gap is the signal to fix the template.
    pub fn apply(&self, source: &str) -> String {
        let out = match &self.action {
            PatternAction::Replace(template) => self.pattern.rewrite(source, template),
            PatternAction::Delete => self.pattern.delete(source),
        };
        if out != source
            && !self.pattern.parses_cleanly(&out)
            && self.pattern.parses_cleanly(source)
        {
            return source.to_string();
        }
        out
    }

    pub fn prefilter(&self) -> Option<&str> {
        self.pattern.prefilter()
    }

    /// Every match in `source`, for read-only search.
    pub fn find(&self, source: &str) -> Vec<Found> {
        self.pattern.find(source)
    }
}

impl std::fmt::Display for PatternRule {
    /// `pattern 'errors.Wrap($E, $M)' -> 'fmt.Errorf(...)'`, without the
    /// language prefix, which the backend adds.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "pattern '{}'{}{}",
            self.pattern.text(),
            colab_core::constraint::display_where(self.pattern.constraints()),
            colab_core::constraint::display_lets(self.pattern.constraints())
        )?;
        match &self.action {
            PatternAction::Replace(template) => write!(f, " -> '{template}'"),
            PatternAction::Delete => write!(f, " -> delete"),
        }
    }
}

/// The most lines [`syntax_tree`] prints before eliding the rest.
pub const MAX_TREE_LINES: usize = 2000;

/// Parse `source` and print its named-node tree, for debugging a pattern
/// against real code. Returns the printed tree and whether the parse hit
/// a syntax error.
pub fn syntax_tree(
    grammar: &tree_sitter::Language,
    source: &str,
) -> Result<(String, bool), String> {
    let mut parser = Parser::new();
    parser
        .set_language(grammar)
        .map_err(|e| format!("grammar failed to load: {e}"))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| "the parser gave up".to_string())?;
    let root = tree.root_node();
    Ok((print_tree(root, source, '$', true), root.has_error()))
}

/// One named node per line, indented by depth:
/// `field: kind  L:C` plus the text for leaves (positions only when
/// `positions` — a pattern's would be relative to a hidden context). Unnamed tokens
/// (punctuation, keywords) are left out — they are not what a pattern's
/// structure is compared on. `expando` is shown as `$`.
fn print_tree(node: Node<'_>, source: &str, expando: char, positions: bool) -> String {
    let mut out = String::new();
    let mut lines = 0usize;
    let mut cursor = node.walk();
    let style = Style { expando, positions };
    print_node(&mut cursor, source, style, 0, &mut out, &mut lines);
    if lines > MAX_TREE_LINES {
        let _ = writeln!(out, "… {} more node(s) not shown", lines - MAX_TREE_LINES);
    }
    out
}

#[derive(Clone, Copy)]
struct Style {
    expando: char,
    positions: bool,
}

fn print_node(
    cursor: &mut tree_sitter::TreeCursor<'_>,
    source: &str,
    style: Style,
    depth: usize,
    out: &mut String,
    lines: &mut usize,
) {
    let node = cursor.node();
    let mut child_depth = depth;
    if node.is_named() || node.is_error() || node.is_missing() {
        *lines += 1;
        if *lines <= MAX_TREE_LINES {
            let field = cursor
                .field_name()
                .map(|f| format!("{f}: "))
                .unwrap_or_default();
            let pos = node.start_position();
            let kind = if node.is_missing() {
                format!("MISSING {}", node.kind())
            } else {
                node.kind().to_string()
            };
            let _ = write!(out, "{}{}{}", "  ".repeat(depth), field, kind);
            if style.positions {
                let _ = write!(out, "  {}:{}", pos.row + 1, pos.column + 1);
            }
            if node.named_child_count() == 0
                && let Ok(text) = node.utf8_text(source.as_bytes())
            {
                let shown: String = text.replace(style.expando, "$");
                let shown = if shown.len() > 80 {
                    format!("{}…", shown.chars().take(80).collect::<String>())
                } else {
                    shown
                };
                let _ = write!(out, "  {shown:?}");
            }
            out.push('\n');
        }
        child_depth += 1;
    }
    if cursor.goto_first_child() {
        loop {
            print_node(cursor, source, style, child_depth, out, lines);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();
    }
}

/// What `colab ast` prints for one input; mirrors `colab_core::SyntaxTree`
/// field for field (this crate does not depend on `colab-core`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeReport {
    pub tree: String,
    pub has_error: bool,
    pub context: Option<String>,
    pub prefilter: Option<String>,
}

/// The tree for a snippet (`is_pattern == false`) or for `text` compiled
/// as a pattern exactly as a `<lang>::pattern` rule would compile it.
pub fn describe(
    language: &PatternLanguage,
    text: &str,
    is_pattern: bool,
) -> Result<TreeReport, String> {
    if is_pattern {
        let compiled = StructuralPattern::compile(language, text)?;
        Ok(TreeReport {
            tree: compiled.tree().to_string(),
            has_error: false,
            context: compiled.context().map(str::to_string),
            prefilter: compiled.prefilter().map(str::to_string),
        })
    } else {
        let (tree, has_error) = syntax_tree(&language.grammar, text)?;
        Ok(TreeReport {
            tree,
            has_error,
            context: None,
            prefilter: None,
        })
    }
}

/// Parse `text`, failing if the tree contains a syntax error.
/// `source` as ast-grep's tree, parsed once per text (`parse_cache`).
fn parsed(lang: &TsLang, source: &str) -> ast_grep_core::AstGrep<StrDoc<TsLang>> {
    match crate::parse_cache::parse(&lang.grammar, source) {
        Some(tree) => ast_grep_core::AstGrep::doc(StrDoc {
            src: source.to_string(),
            lang: lang.clone(),
            tree,
        }),
        None => lang.ast_grep(source),
    }
}

fn parse_clean(grammar: &tree_sitter::Language, text: &str) -> Result<tree_sitter::Tree, String> {
    let tree = crate::parse_cache::parse(grammar, text)
        .ok_or_else(|| "the parser gave up (or the grammar failed to load)".to_string())?;
    if tree.root_node().has_error() {
        return Err("syntax error".into());
    }
    Ok(tree)
}

/// Parse a pattern (metavariables already expanded). Like [`parse_clean`],
/// except that an error node holding nothing but a metavariable is
/// allowed: `$$$B` alone in a Rust `impl` body or a Go struct is not a
/// valid item to the grammar, but it is a placeholder, not a mistake, and
/// ast-grep matches it as one. Any other error, or a missing node, still
/// fails the pattern.
fn parse_pattern(
    grammar: &tree_sitter::Language,
    text: &str,
    expando: char,
    tolerant: bool,
) -> Result<tree_sitter::Tree, String> {
    if !tolerant {
        return parse_clean(grammar, text);
    }
    let mut parser = Parser::new();
    parser
        .set_language(grammar)
        .map_err(|e| format!("grammar failed to load: {e}"))?;
    let tree = parser
        .parse(text, None)
        .ok_or_else(|| "the parser gave up".to_string())?;
    if !tree.root_node().has_error() {
        return Ok(tree);
    }
    let placeholder = |node: Node<'_>| {
        let t = node.utf8_text(text.as_bytes()).unwrap_or("").trim();
        t.strip_prefix(expando).is_some_and(|rest| {
            let name = rest.trim_start_matches(expando);
            !name.is_empty() && name.chars().all(|c| is_var_start(c) || c.is_ascii_digit())
        })
    };
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.is_missing() || (node.is_error() && !placeholder(node)) {
            return Err("syntax error".into());
        }
        if node.has_error() && !node.is_error() {
            let mut cursor = node.walk();
            stack.extend(node.children(&mut cursor));
        }
    }
    Ok(tree)
}

/// Narrow `span` to exclude surrounding whitespace.
fn trim_span(text: &str, span: std::ops::Range<usize>) -> std::ops::Range<usize> {
    let slice = &text[span.clone()];
    let start = span.start + (slice.len() - slice.trim_start().len());
    let end = span.end - (slice.len() - slice.trim_end().len());
    start..end
}

/// The deepest named node whose byte range is exactly `span`.
fn deepest_covering(root: Node<'_>, span: std::ops::Range<usize>) -> Option<Node<'_>> {
    let mut node = root.named_descendant_for_byte_range(span.start, span.end)?;
    if node.byte_range() != span {
        return None;
    }
    // Descend through single-child wrappers that cover the same span
    // (`expression_statement` → `call_expression`).
    loop {
        let child = (0..node.named_child_count())
            .filter_map(|i| node.named_child(i as u32))
            .find(|c| c.byte_range() == span);
        match child {
            Some(c) => node = c,
            None => return Some(node),
        }
    }
}

/// The node a bare pattern actually matches on: ast-grep descends from the
/// parse root through wrappers with a single child (`module` →
/// `expression_statement` → `call`), ignoring a trailing missing or
/// empty-kind node. Mirrored here so `colab ast` shows that node.
fn single_root(mut node: Node<'_>) -> Node<'_> {
    loop {
        let single = match node.child_count() {
            1 => true,
            2 => node
                .child(1)
                .is_some_and(|c| c.is_missing() || c.kind().is_empty()),
            _ => false,
        };
        match node.child(0) {
            Some(child) if single => node = child,
            _ => return node,
        }
    }
}

/// The first node of `kind` in preorder — the one ast-grep's contextual
/// pattern will select.
fn first_of_kind<'t>(root: Node<'t>, kind: &str) -> Option<Node<'t>> {
    let mut cursor = root.walk();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == kind {
            return Some(node);
        }
        let children: Vec<Node<'t>> = node.children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
    None
}

/// The longest named-leaf text under `node` that is not a metavariable.
fn longest_literal(node: Node<'_>, source: &str, expando: char) -> Option<String> {
    let mut best: Option<String> = None;
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.child_count() == 0 {
            if n.is_named()
                && let Ok(text) = n.utf8_text(source.as_bytes())
                && !text.trim().is_empty()
                && !text.starts_with(expando)
                && best.as_ref().is_none_or(|b| text.len() > b.len())
            {
                best = Some(text.to_string());
            }
            continue;
        }
        for i in 0..n.child_count() {
            if let Some(child) = n.child(i) {
                stack.push(child);
            }
        }
    }
    best
}

fn line_column(source: &str, offset: usize) -> (usize, usize) {
    let before = &source[..offset];
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    (line, source[line_start..offset].chars().count() + 1)
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
        }
    }

    fn python() -> PatternLanguage {
        PatternLanguage {
            grammar: tree_sitter_python::LANGUAGE.into(),
            expando: 'µ',
            contexts: &[],
            also: &["def _(x: @@PATTERN@@): pass\n"],
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
