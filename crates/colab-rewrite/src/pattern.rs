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

use std::borrow::Cow;

use ast_grep_core::language::Language;
use ast_grep_core::matcher::PatternBuilder;
use ast_grep_core::meta_var::MetaVariable;
use ast_grep_core::replacer::TemplateFix;
use ast_grep_core::tree_sitter::{LanguageExt, StrDoc, TSLanguage};
use ast_grep_core::{NodeMatch, Pattern, PatternError};
use colab_core::{Constraint, LanguageBackend, Operation, RuleSpec, Test, Var};
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
        self.grammar.field_id_for_name(field).map(|f| f.get())
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
        if text.trim().is_empty() {
            return Err("a pattern cannot be empty".into());
        }
        let lang = TsLang {
            grammar: language.grammar.clone(),
            expando: language.expando,
        };
        let expanded = expand(text, lang.expando);

        let mut attempts = Vec::new();
        // Bare pattern.
        match parse_clean(&lang.grammar, &expanded) {
            Ok(tree) => match Pattern::try_new(text, lang.clone()) {
                Ok(pattern) => {
                    let root = single_root(tree.root_node());
                    let prefilter = longest_literal(root, &expanded, lang.expando);
                    let printed = print_tree(root, &expanded, lang.expando, false);
                    return Ok(Self::finish(lang, pattern, text, prefilter, None, printed));
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
            let tree = match parse_clean(&lang.grammar, &filled_expanded) {
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
                    return Ok(Self::finish(
                        lang,
                        pattern,
                        text,
                        prefilter,
                        Some(context.to_string()),
                        printed,
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
            .map(|c| Check::compile(language, &bound, c))
            .collect::<Result<_, _>>()?;
        self.constraints = constraints.to_vec();
        Ok(self)
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
        let grep = self.lang.ast_grep(source);
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
        let grep = self.lang.ast_grep(source);
        let matches = self.matches(&grep, source);
        if matches.is_empty() {
            return source.to_string();
        }
        let (template, names) = transformable_template(template);
        let fix = TemplateFix::with_transform(&template, &self.lang, &names);
        let rewriter = Rewriter {
            pattern: &self.pattern,
            fix: &fix,
            matches: &matches,
        };
        let edits = rewriter
            .outermost(0..source.len(), None)
            .into_iter()
            .map(|i| rewriter.render(source, i))
            .collect();
        apply_edits(source, edits)
    }

    /// Remove every match that stands alone on its line(s) — apart from
    /// indentation and a trailing `;` or `,` — taking the whole line with
    /// it. A match nested inside a larger expression (`() => f(x)`,
    /// `[f(x), 2]`) is left in place: removing it would leave the code
    /// around it incomplete. Rewrite those with `replace` instead.
    pub fn delete(&self, source: &str) -> String {
        let spans: Vec<(usize, usize)> = self
            .find(source)
            .into_iter()
            .filter(|found| {
                let (line_start, line_end) = line_span(source, found.start, found.end);
                let before = &source[line_start..found.start];
                let after = &source[found.end..line_end];
                before.trim().is_empty()
                    && after
                        .trim()
                        .trim_start_matches([';', ','])
                        .trim()
                        .is_empty()
            })
            .map(|found| (found.start, found.end))
            .collect();
        crate::delete_lines(source, spans)
    }

    /// `true` when `source` parses without a syntax error.
    fn parses_cleanly(&self, source: &str) -> bool {
        parse_clean(&self.lang.grammar, source).is_ok()
    }
}

/// Fills a template for one match after rewriting the matches nested in
/// its bindings — see [`StructuralPattern::rewrite`].
struct Rewriter<'a, 't> {
    pattern: &'a Pattern,
    fix: &'a TemplateFix,
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
        for var in vars {
            let (name, span) = match &var {
                MetaVariable::Capture(name, _) => {
                    let span = nm.get_env().get_match(name).map(|n| n.range());
                    (name.clone(), span)
                }
                MetaVariable::MultiCapture(name) => {
                    let nodes = nm.get_env().get_multiple_matches(name);
                    let span = match (nodes.first(), nodes.last()) {
                        (Some(first), Some(last)) => Some(first.range().start..last.range().end),
                        _ => None,
                    };
                    (name.clone(), span)
                }
                _ => continue,
            };
            let text = span
                .map(|span| self.rewrite_span(source, span, i))
                .unwrap_or_default();
            nm.get_env_mut()
                .insert_transformation(&var, &name, text.into_bytes());
        }
        let edit = nm.make_edit(self.pattern, self.fix);
        Edit::new(
            edit.position,
            edit.position + edit.deleted_length,
            strip_blank_line_indent(&String::from_utf8_lossy(&edit.inserted_text)),
        )
    }
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
    Inside(Pattern),
    Has(Pattern),
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
            CheckKind::Inside(pattern) => m.get_node().inside(pattern),
            CheckKind::Has(pattern) => m.get_node().has(pattern),
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
fn nested(language: &PatternLanguage, keyword: &str, text: &str) -> Result<Pattern, String> {
    StructuralPattern::compile(language, text)
        .map(|p| p.pattern)
        .map_err(|e| format!("`{keyword} '{text}'`: {e}"))
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
/// an ordinary [`LanguageBackend::build_rule`]; constraints are accepted
/// only on `pattern`, with `replace` or `delete`, and are compiled by
/// `compile` (the backend's own `PatternOp::compile`).
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
}

impl PatternRule {
    /// Compile `text` with its `where` constraints (`&[]` for none).
    pub fn compile(
        language: &PatternLanguage,
        text: &str,
        action: PatternAction,
        constraints: &[Constraint],
    ) -> Result<Self, String> {
        Ok(Self {
            pattern: StructuralPattern::compile(language, text)?
                .constrain(language, constraints)?,
            action,
        })
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
            "pattern '{}'{}",
            self.pattern.text(),
            colab_core::constraint::display_where(self.pattern.constraints())
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
        out.push_str(&format!(
            "… {} more node(s) not shown\n",
            lines - MAX_TREE_LINES
        ));
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
            out.push_str(&format!("{}{}{}", "  ".repeat(depth), field, kind));
            if style.positions {
                out.push_str(&format!("  {}:{}", pos.row + 1, pos.column + 1));
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
                out.push_str(&format!("  {shown:?}"));
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
fn parse_clean(grammar: &tree_sitter::Language, text: &str) -> Result<tree_sitter::Tree, String> {
    let mut parser = Parser::new();
    parser
        .set_language(grammar)
        .map_err(|e| format!("grammar failed to load: {e}"))?;
    let tree = parser
        .parse(text, None)
        .ok_or_else(|| "the parser gave up".to_string())?;
    if tree.root_node().has_error() {
        return Err("syntax error".into());
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
    let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
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
        }
    }

    fn python() -> PatternLanguage {
        PatternLanguage {
            grammar: tree_sitter_python::LANGUAGE.into(),
            expando: 'µ',
            contexts: &[],
        }
    }

    const GO_SRC: &str = "package main\n\nfunc main() {\n\tif err := f(); err != nil {\n\t\treturn errors.Wrap(err, \"f failed\")\n\t}\n\tlog.Println(\"a\", 1)\n}\n";

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
    fn delete_leaves_nested_matches_in_place() {
        let p = StructuralPattern::compile(&python(), "print($X)").unwrap();
        let src = "print(0)\ny = [print(1), 2]\n";
        assert_eq!(p.delete(src), "y = [print(1), 2]\n");
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
