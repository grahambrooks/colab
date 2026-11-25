//! Every `<lang>::pattern` rule on `colab_syntax`, colab's own tree and
//! matcher, while the languages still parse with tree-sitter: a grammar's
//! names (built once per grammar), a source text as a colab tree, a
//! pattern compiled with a backend's [`PatternLanguage`] into its forms,
//! its `where` checks and `let`s, and find, rewrite and delete over a
//! file and the macro calls whose contents it reads. [`RulePattern`] is
//! what `pattern::PatternRule` holds.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use colab_syntax::fragment::{self, Compiled, Fragments};
use colab_syntax::{Grammar, Tree};

use crate::pattern::{PatternLanguage, Syntax};

mod check;
mod delete;
mod rewrite;
mod rule;
pub use check::{Checks, Enclosing, Frame, Nested, bound};
pub use delete::delete;
pub use rewrite::{Lets, rewrite};
pub use rule::RulePattern;

/// `language`'s kind and field names, built on first use.
pub fn grammar_of(language: &tree_sitter::Language) -> Arc<Grammar> {
    static GRAMMARS: OnceLock<Mutex<HashMap<tree_sitter::Language, Arc<Grammar>>>> =
        OnceLock::new();
    let mut grammars = GRAMMARS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    grammars
        .entry(language.clone())
        .or_insert_with(|| {
            let name = language.name().unwrap_or("?").to_string();
            Arc::new(Grammar::from_tree_sitter(&name, language))
        })
        .clone()
}

/// `source` as a colab tree for `language`: by its own parser when it has
/// one ([`Syntax::Own`]), else by tree-sitter ([`tree`]).
pub fn source_tree(language: &PatternLanguage, source: &str) -> Option<Tree> {
    match &language.syntax {
        Syntax::Own(parser) => Some(parser.parse(source)),
        Syntax::TreeSitter(grammar) => tree(grammar, source),
    }
}

/// [`source_tree`] for a fragment, whose parse is never reused, so it
/// skips the shared cache.
pub fn fragment_tree(language: &PatternLanguage, text: &str) -> Option<Tree> {
    match &language.syntax {
        Syntax::Own(parser) => Some(parser.parse(text)),
        Syntax::TreeSitter(grammar) => {
            let mut parser = tree_sitter::Parser::new();
            parser.set_language(grammar).ok()?;
            let parsed = parser.parse(text, None)?;
            Tree::from_tree_sitter(&parsed, text, grammar_of(grammar)).ok()
        }
    }
}

/// The kind and field names of `language`'s trees: its own parser's, or
/// tree-sitter's.
pub fn grammar_for(language: &PatternLanguage) -> Arc<Grammar> {
    match &language.syntax {
        Syntax::Own(parser) => parser.grammar(),
        Syntax::TreeSitter(grammar) => grammar_of(grammar),
    }
}

/// `source` as a colab tree, parsed through the shared parse cache.
pub fn tree(language: &tree_sitter::Language, source: &str) -> Option<Tree> {
    let parsed = crate::parse_cache::parse(language, source)?;
    Tree::from_tree_sitter(&parsed, source, grammar_of(language)).ok()
}

/// What a form needs from a rewrite beyond its bindings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    /// The pattern as written (or another reading of it).
    Plain,
    /// `to_svg($A)` read as `$COLAB_PATH::to_svg($A)` (or rooted,
    /// `::$COLAB_PATH::to_svg($A)`), so it also finds the function called
    /// by its path; a rewrite puts the path back before the template's
    /// callee.
    Qualified {
        separator: &'static str,
        callee: String,
        rooted: bool,
    },
    /// `S { host: $H }` read with these `(name, VAR)` fields as the
    /// shorthand `S { host }`; a rewrite writes them back the same way.
    Shorthand { fields: Vec<(String, String)> },
}

/// One form of a pattern for the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineForm {
    pub pattern: colab_syntax::pattern::Pattern,
    pub context: Option<String>,
    pub role: Role,
    /// Compiled without the pattern's trailing `;`, which its node does
    /// not hold (a TypeScript class field's): a template's goes too.
    pub loose_semicolon: bool,
}

/// Every form of a pattern, the first the one it reads as first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Forms {
    pub forms: Vec<EngineForm>,
    pub tolerant: bool,
    /// The pattern as written.
    pub text: String,
}

/// The modifier rules for `language`'s patterns: its modifiers' kinds
/// and its modifier tokens may be left out; a spelled modifiers node is
/// matched exactly.
pub fn rules_for(language: &PatternLanguage) -> colab_syntax::pattern::Rules {
    let grammar = grammar_for(language);
    let kinds = |names: &[&str]| -> Vec<colab_syntax::Kind> {
        names.iter().flat_map(|n| grammar.kinds_named(n)).collect()
    };
    let modifiers = language.modifiers.map_or(&[][..], |m| m.kinds);
    colab_syntax::pattern::Rules {
        absorb: kinds(modifiers),
        exact: kinds(modifiers),
        // Keywords no sentinel stands for may be left out one by one, and
        // a rewrite puts back those its template leaves out
        // (`keep_modifier_words`).
        loose: kinds(language.modifier_tokens),
    }
}

/// `text` compiled for `language` with every form the engine reads it in
/// (the text transforms are `pattern/compile.rs`'s): the readings in
/// each context; the qualified and rooted call; the field shorthand;
/// the pattern without a trailing `;` its node does not hold, or a
/// trailing `,` that changes nothing. Modifiers need no forms: the
/// matcher takes the ones a pattern leaves out ([`rules_for`]).
pub fn compile_forms(language: &PatternLanguage, text: &str) -> Result<Forms, String> {
    let rules = rules_for(language);
    let plain = |compiled: Compiled, loose: bool| -> Vec<EngineForm> {
        compiled
            .forms
            .into_iter()
            .map(|f| EngineForm {
                pattern: f.pattern.with_rules(rules.clone()),
                context: f.context,
                role: Role::Plain,
                loose_semicolon: loose,
            })
            .collect()
    };
    let first = |text: &str| -> Option<colab_syntax::pattern::Pattern> {
        compile_fragment(language, text)
            .ok()
            .and_then(|c| c.forms.into_iter().next())
            .map(|f| f.pattern.with_rules(rules.clone()))
    };
    // A trailing `;` the node does not hold: compiled without it when it
    // reads the same without it, or only so.
    let semicolon = text
        .trim_end()
        .strip_suffix(';')
        .filter(|s| !s.trim().is_empty() && !s.trim_end().ends_with(';'));
    let read = compile_fragment(language, text);
    if let Some(stripped) = semicolon {
        let same = match &read {
            Err(_) => true,
            Ok(with) => compile_fragment(language, stripped)
                .is_ok_and(|w| w.forms[0].pattern == with.forms[0].pattern),
        };
        if same && let Ok(mut loose) = compile_forms(language, stripped) {
            for form in &mut loose.forms {
                form.loose_semicolon = true;
            }
            loose.text = text.to_string();
            return Ok(loose);
        }
    }
    let compiled = read?;
    let tolerant = compiled.tolerant;
    let mut forms = plain(compiled, false);
    let add = |form: EngineForm, forms: &mut Vec<EngineForm>| {
        if forms.iter().all(|f| f.pattern != form.pattern) {
            forms.push(form);
        }
    };
    // `name = $V;` is also a class field without its `;`.
    if let Some(stripped) = semicolon
        && let Ok(loose) = compile_fragment(language, stripped)
    {
        let grammar = grammar_for(language);
        for form in plain(loose, true) {
            let field = form
                .pattern
                .kind()
                .is_some_and(|k| grammar.kind_name(k).contains("field"));
            if field {
                add(form, &mut forms);
            }
        }
    }
    // A trailing `,` that changes nothing: the candidate's is passed over
    // anyway, and a pattern that spells it would demand it.
    if let Some(stripped) = text.trim_end().strip_suffix(',')
        && let Some(loose) = first(stripped)
        && loose.kind() == forms[0].pattern.kind()
    {
        forms[0].pattern = loose;
    }
    if let Some(sep) = language.qualified_calls
        && let Some((qualified, callee)) = crate::pattern::qualify_call(text, sep)
    {
        let rooted = crate::pattern::root_path(&qualified, sep);
        // Rooted first: the plain form also matches `\M\f()` with the
        // leading separator passed over, and a rewrite by it would drop
        // the separator; forms rewrite in order, so the rooted one goes
        // first and leaves the plain one nothing there.
        for (text, rooted) in [(rooted, true), (qualified, false)] {
            if let Some(pattern) = first(&text) {
                let role = Role::Qualified {
                    separator: sep,
                    callee: callee.to_string(),
                    rooted,
                };
                add(
                    EngineForm {
                        pattern,
                        context: None,
                        role,
                        loose_semicolon: false,
                    },
                    &mut forms,
                );
            }
        }
    }
    if let Some(shorthand) = language.shorthand_fields {
        let fields = crate::pattern::shorthand_fields(text, shorthand);
        let subsets: Vec<Vec<usize>> = if fields.len() <= 3 {
            (1..1usize << fields.len())
                .map(|bits| (0..fields.len()).filter(|i| bits & (1 << i) != 0).collect())
                .collect()
        } else {
            vec![(0..fields.len()).collect()]
        };
        for subset in subsets {
            let mut alt = text.to_string();
            for &i in subset.iter().rev() {
                let (range, name, _) = &fields[i];
                alt.replace_range(range.clone(), &format!("{name}{}", shorthand.kept));
            }
            if let Some(pattern) = first(&alt) {
                let fields = subset
                    .iter()
                    .map(|&i| (fields[i].1.clone(), fields[i].2.clone()))
                    .collect();
                add(
                    EngineForm {
                        pattern,
                        context: None,
                        role: Role::Shorthand { fields },
                        loose_semicolon: false,
                    },
                    &mut forms,
                );
            }
        }
    }
    Ok(Forms {
        forms,
        tolerant,
        text: text.to_string(),
    })
}

/// A literal every match of `forms` must contain, so a file without it is
/// skipped unparsed (`Operation::prefilter`): the longest text of a named
/// token that every form spells (the forms read the pattern differently,
/// so a literal only one of them needs proves nothing). Named tokens are
/// matched by exact text, so the literal's absence implies no match.
/// `None` for a pattern with no such literal (a lone metavariable).
pub fn prefilter(forms: &Forms) -> Option<String> {
    fn literals(node: &colab_syntax::pattern::PatternNode, out: &mut Vec<String>) {
        use colab_syntax::pattern::PatternNode;
        match node {
            PatternNode::Token {
                text, named: true, ..
            } if !text.trim().is_empty() => {
                out.push(text.clone());
            }
            PatternNode::Node { children, .. } => {
                for c in children {
                    literals(c, out);
                }
            }
            _ => {}
        }
    }
    let mut each = forms.forms.iter().map(|f| {
        let mut out = Vec::new();
        literals(f.pattern.root(), &mut out);
        out
    });
    let first = each.next()?;
    let rest: Vec<Vec<String>> = each.collect();
    first
        .into_iter()
        .filter(|l| rest.iter().all(|r| r.contains(l)))
        .max_by(|a, b| a.len().cmp(&b.len()).then_with(|| b.cmp(a)))
}

/// A match of a form, as a span of the file it was found in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub range: std::ops::Range<usize>,
    /// The form that matched (an index into [`Forms::forms`]).
    pub form: usize,
    /// It was found inside a macro call's contents.
    pub in_macro: bool,
    /// The match's text (all of its node's, a trailing newline included).
    pub text: String,
    /// Its metavariables' bindings, sorted by name; a `$$$` binding is the
    /// text from its first node to its last.
    pub captures: Vec<(String, String)>,
}

/// Every match of `forms` in `source`: in its tree, then inside the
/// contents of the macro calls the language reads (Rust's `assert!(…)`,
/// `vec![…]`, a project's own from `colab.toml`), each re-parsed in the
/// language's macro wrapper and its matches mapped back. A macro whose matches
/// would reach into the wrapper is left out, so a rewrite never edits
/// outside the call. Sorted by position, outer matches first.
pub fn find(language: &PatternLanguage, forms: &Forms, source: &str) -> Vec<Found> {
    find_where(language, forms, &Checks::default(), source)
}

/// [`find`], keeping only the matches every one of `checks` holds for (a
/// rule's `where`), judged in the file or, for a macro's contents, as if
/// in the file ([`Frame`]).
pub fn find_where(
    language: &PatternLanguage,
    forms: &Forms,
    checks: &Checks,
    source: &str,
) -> Vec<Found> {
    let Some(tree) = self::source_tree(language, source) else {
        return Vec::new();
    };
    let mut out = find_in_tree(forms, checks, &tree, None);
    out.extend(find_in_macros(language, forms, checks, &tree, None, 0));
    out.sort_by_key(|f| (f.range.start, std::cmp::Reverse(f.range.end), f.form));
    out.dedup_by(|a, b| a.range == b.range);
    out
}

fn find_in_tree(
    forms: &Forms,
    checks: &Checks,
    tree: &Tree,
    frame: Option<&Frame<'_>>,
) -> Vec<Found> {
    let mut out = Vec::new();
    for (form, f) in forms.forms.iter().enumerate() {
        let mut found = f.pattern.find_all(tree);
        checks.retain(&mut found, tree.text(), frame);
        out.extend(found.into_iter().map(|m| {
            let mut captures: Vec<(String, String)> = m
                .bindings
                .iter()
                .map(|(name, b)| (name.clone(), b.text().to_string()))
                .collect();
            captures.sort();
            captures.dedup();
            Found {
                range: m.node.range(),
                form,
                in_macro: frame.is_some(),
                text: m.node.text().to_string(),
                captures,
            }
        }));
    }
    out
}

/// Macro nesting colab reads into, at most: each level is a re-parse.
const MACRO_DEPTH: usize = 8;

fn find_in_macros(
    language: &PatternLanguage,
    forms: &Forms,
    checks: &Checks,
    tree: &Tree,
    frame: Option<&Frame<'_>>,
    depth: usize,
) -> Vec<Found> {
    each_macro(
        language,
        forms,
        checks,
        tree,
        frame,
        depth,
        |region, _, found| {
            let (slot, base) = (region.slot, region.inner.start);
            Some(
                found
                    .into_iter()
                    .map(|f| Found {
                        range: f.range.start - slot + base..f.range.end - slot + base,
                        in_macro: true,
                        ..f
                    })
                    .collect::<Vec<_>>(),
            )
        },
    )
    .into_iter()
    .flatten()
    .collect()
}

/// A listed macro call's contents, as [`each_macro`] reads them.
pub(crate) struct Region {
    /// The contents in the text: inside the call's delimiters.
    pub(crate) inner: std::ops::Range<usize>,
    /// Where the contents start in `wrapped`.
    pub(crate) slot: usize,
    /// The contents in the language's macro wrapper.
    pub(crate) wrapped: String,
}

/// `then` for each listed macro call in `tree` whose contents parse in the
/// wrapper and hold a match of `forms` that `checks` hold for, none of
/// them reaching into the wrapper: given the region, its frame, and the
/// matches in the wrapped text (nested macros' included).
fn each_macro<R>(
    language: &PatternLanguage,
    forms: &Forms,
    checks: &Checks,
    tree: &Tree,
    frame: Option<&Frame<'_>>,
    depth: usize,
    mut then: impl FnMut(&Region, &Frame<'_>, Vec<Found>) -> Option<R>,
) -> Vec<R> {
    let Some(spec) = language.macro_contents else {
        return Vec::new();
    };
    let Some(slot) = spec.wrapper.find(crate::pattern::PATTERN_SLOT) else {
        return Vec::new();
    };
    if depth >= MACRO_DEPTH {
        return Vec::new();
    }
    let extra = colab_core::run::Run::current().map_or_else(Vec::new, |run| run.macros());
    let listed = |name: &str| spec.macros.contains(&name) || extra.iter().any(|m| m == name);
    let mut out = Vec::new();
    for call in tree.nodes().filter(|n| n.kind_name() == spec.invocation) {
        if frame.is_some_and(|f| !f.holds(call)) {
            continue;
        }
        // A tree without fields (rust-analyzer's) names the macro by the
        // call's first named child, its path.
        let name = call
            .child_by_field(spec.name_field)
            .or_else(|| call.children().find(colab_syntax::Node::is_named))
            .map(|n| n.text().rsplit("::").next().unwrap_or(n.text()).trim());
        if !name.is_some_and(listed) {
            continue;
        }
        let Some(body) = call
            .children()
            .filter(colab_syntax::Node::is_named)
            .last()
            .filter(|b| b.kind_name() == spec.body && b.range().len() >= 2)
        else {
            continue;
        };
        // Inside the delimiters: `(`…`)`, `[`…`]`, `{`…`}`.
        let inner = body.start() + 1..body.end() - 1;
        let contents = &tree.text()[inner.clone()];
        let wrapped = spec
            .wrapper
            .replacen(crate::pattern::PATTERN_SLOT, contents, 1);
        let Some(wrapped_tree) = self::source_tree(language, &wrapped) else {
            continue;
        };
        if wrapped_tree.has_error() {
            continue;
        }
        let within = slot..slot + contents.len();
        // Beyond the contents: this call and its ancestors (within the
        // enclosing contents, for a nested call), then the enclosing
        // frame's.
        let mut outside: Vec<Enclosing<'_>> = std::iter::once(call)
            .chain(std::iter::successors(
                call.parent(),
                colab_syntax::Node::parent,
            ))
            .filter(|a| frame.is_none_or(|f| f.holds(*a)))
            .map(|a| -> Enclosing<'_> { Box::new(move |n: &Nested| n.matches(a)) })
            .collect();
        if let Some(f) = frame {
            outside.extend(
                f.outside
                    .iter()
                    .map(|outer| -> Enclosing<'_> { Box::new(move |n: &Nested| outer(n)) }),
            );
        }
        let base = inner.start;
        let map: Box<dyn Fn(usize) -> usize + '_> = match frame {
            Some(f) => Box::new(move |p: usize| (f.map)(p.saturating_sub(slot) + base)),
            None => Box::new(move |p: usize| p.saturating_sub(slot) + base),
        };
        let inner_frame = Frame {
            contents: within.clone(),
            outside: &outside,
            file: frame.map_or(tree.text(), |f| f.file),
            map,
        };
        let mut found = find_in_tree(forms, checks, &wrapped_tree, Some(&inner_frame));
        found.extend(find_in_macros(
            language,
            forms,
            checks,
            &wrapped_tree,
            Some(&inner_frame),
            depth + 1,
        ));
        if found.is_empty()
            || found
                .iter()
                .any(|f| f.range.start < within.start || f.range.end > within.end)
        {
            continue;
        }
        let region = Region {
            inner: inner.clone(),
            slot,
            wrapped: wrapped.clone(),
        };
        out.extend(then(&region, &inner_frame, found));
    }
    out
}

/// `text` with the contents of each macro call `forms` match in rewritten
/// by `rewrite` (given the wrapped contents and their frame) and put back,
/// unless the rewrite changed the wrapper around them.
pub(crate) fn edit_macros(
    language: &PatternLanguage,
    forms: &Forms,
    checks: &Checks,
    text: &str,
    frame: Option<&Frame<'_>>,
    depth: usize,
    rewrite: impl Fn(&str, &Frame<'_>, usize) -> String,
) -> String {
    let Some(tree) = self::source_tree(language, text) else {
        return text.to_string();
    };
    let changed = each_macro(
        language,
        forms,
        checks,
        &tree,
        frame,
        depth,
        |region, inner, _| {
            Some((
                region.inner.clone(),
                region.slot,
                region.wrapped.clone(),
                rewrite(&region.wrapped, inner, depth + 1),
            ))
        },
    );
    let mut out = text.to_string();
    for (inner, slot, wrapped, changed) in changed.into_iter().rev() {
        let suffix = wrapped.len() - slot - inner.len();
        let (head, tail) = (&wrapped[..slot], &wrapped[wrapped.len() - suffix..]);
        if changed == wrapped
            || changed.len() < head.len() + tail.len()
            || !changed.starts_with(head)
            || !changed.ends_with(tail)
        {
            continue;
        }
        out.replace_range(inner, &changed[head.len()..changed.len() - tail.len()]);
    }
    out
}

/// `text` compiled as a pattern for `language` by colab's own matcher:
/// bare or in the language's contexts, with every other reading as a form.
pub fn compile_fragment(language: &PatternLanguage, text: &str) -> Result<Compiled, String> {
    let lang = Fragments {
        expando: language.expando,
        contexts: language.contexts,
        also: language.also,
    };
    let parse = |filled: &str| fragment_tree(language, filled);
    fragment::compile(text, &lang, &parse)
}
