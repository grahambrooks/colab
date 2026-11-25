//! Compiling a pattern's text: a fragment of code that is rarely a valid
//! file on its own (an expression, a method, a `case`), so it is parsed
//! bare and, failing that, inside each of a language's *contexts*
//! (wrappers holding [`SLOT`]) until one reads it as one node.
//!
//! The parser is the caller's (`parse`), so this works the same over
//! tree-sitter today and a parser of colab's own later, which will give
//! fragments entry points of their own and need no contexts.
//!
//! Text that reads as code in more than one context means each (Rust's
//! `also: $A` is a field in a declaration and in a literal), so every
//! later context, and every one of the language's `also` contexts, that
//! reads it as a *different* pattern adds a form, matched beside the first.

use crate::pattern::{MetaVar, Pattern, expand, meta_variable};
use crate::{Node, Tree};

/// Where a pattern goes in a context.
pub const SLOT: &str = "@@PATTERN@@";

/// What a language supplies for compiling fragments.
#[derive(Debug, Clone, Copy)]
pub struct Fragments<'a> {
    /// What `$` becomes before parsing, where `$` cannot start an
    /// identifier (`µ`), or `$` where it can.
    pub expando: char,
    /// Tried in order when the text does not parse on its own.
    pub contexts: &'a [&'a str],
    /// Compiled in as well, wherever they read the text differently.
    pub also: &'a [&'a str],
}

/// One reading of a pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    pub pattern: Pattern,
    /// The context it was read in; `None` on its own.
    pub context: Option<String>,
}

/// A compiled pattern: its first reading, then the others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compiled {
    pub forms: Vec<Form>,
    /// It parsed only with a placeholder kept in an error node (a lone
    /// `$$$B` where the grammar wants an item), nowhere cleanly.
    pub tolerant: bool,
}

/// Whether a tree's errors are acceptable: none, or, when `tolerant`, only
/// those a placeholder causes: `ERROR` nodes holding nothing but a
/// metavariable, and a missing token straight after one (rust-analyzer
/// reads a lone `$$$B` in an item list as a macro call missing its `!`).
fn parses(tree: &Tree, expando: char, tolerant: bool) -> bool {
    if !tree.has_error() {
        return true;
    }
    if !tolerant {
        return false;
    }
    let mut after_placeholder = false;
    for n in tree.nodes() {
        if n.is_trivia() {
            continue;
        }
        if n.is_missing() {
            if !after_placeholder {
                return false;
            }
            continue;
        }
        if n.is_error() && meta_variable(n.text(), expando).is_none() {
            return false;
        }
        if n.is_token() {
            after_placeholder = meta_variable(n.text(), expando).is_some();
        }
    }
    true
}

/// The deepest node spanning exactly `span` (or `span` and whitespace
/// after it: Go's `expression_case` owns its newline).
fn spanning(tree: &Tree, span: std::ops::Range<usize>) -> Option<Node<'_>> {
    // The smallest node holding the span: it is the pattern only if it
    // starts where the span does and ends there or in whitespace.
    let node = tree.root().covering(span.clone())?;
    let tail = tree.text().get(span.end..node.end())?;
    (node.start() == span.start && (node.end() == span.end || tail.trim().is_empty()))
        .then_some(node)
}

/// `text` with surrounding whitespace excluded from `span`.
fn trim_span(text: &str, span: std::ops::Range<usize>) -> std::ops::Range<usize> {
    let slice = &text[span.clone()];
    let start = span.start + (slice.len() - slice.trim_start().len());
    let end = span.end - (slice.len() - slice.trim_end().len());
    start..end.max(start)
}

/// One attempt: `text` (expanded) in `context`, or bare when `None`.
fn read(
    expanded: &str,
    context: Option<&str>,
    expando: char,
    tolerant: bool,
    parse: &dyn Fn(&str) -> Option<Tree>,
) -> Result<Form, String> {
    let (filled, slot) = match context {
        Some(c) => {
            let slot = c
                .find(SLOT)
                .ok_or_else(|| "the context has no slot".to_string())?;
            (c.replacen(SLOT, expanded, 1), slot)
        }
        None => (expanded.to_string(), 0),
    };
    let tree = parse(&filled).ok_or_else(|| "the parser gave up".to_string())?;
    if !parses(&tree, expando, tolerant) {
        return Err("syntax error".into());
    }
    let span = trim_span(&filled, slot..slot + expanded.len());
    let node = spanning(&tree, span)
        .ok_or_else(|| "the pattern is not a single syntax node there".to_string())?;
    // Only the root spans it: it is several nodes (`$A\nuse x;`), and as a
    // pattern the root would match whole files.
    if node.id() == 0 {
        return Err("the pattern is more than one syntax node".into());
    }
    // A context's own text before the slot holding a node of the pattern's
    // kind means the context reads the pattern as something it is not
    // meant to: the languages' contexts put a declared type before a
    // parameter slot precisely so a bare name gains no type reading.
    if context.is_some()
        && tree
            .nodes()
            .any(|n| n.kind() == node.kind() && n.end() <= slot && !n.is_trivia())
    {
        return Err(format!(
            "the wrapper has an earlier `{}` than the pattern",
            node.kind_name()
        ));
    }
    Ok(Form {
        pattern: Pattern::from_node(node, expando),
        context: context.map(str::to_string),
    })
}

fn describe(context: Option<&str>) -> String {
    match context {
        None => "as-is".into(),
        Some(c) => format!("in `{}`", c.replace('\n', " ").trim_end()),
    }
}

/// The first reading of `expanded`, bare (when `bare`) then in each of
/// `contexts`; every failure named when none reads it.
fn first(
    expanded: &str,
    contexts: &[&str],
    bare: bool,
    lang: &Fragments<'_>,
    tolerant: bool,
    parse: &dyn Fn(&str) -> Option<Tree>,
    attempts: &mut Vec<String>,
) -> Option<Form> {
    let tries = bare
        .then_some(None)
        .into_iter()
        .chain(contexts.iter().map(|c| Some(*c)));
    for context in tries {
        match read(expanded, context, lang.expando, tolerant, parse) {
            Ok(form) => return Some(form),
            Err(e) => attempts.push(format!("{}: {e}", describe(context))),
        }
    }
    None
}

/// Compile `text` for a language: its forms, the first the one it reads
/// as first.
pub fn compile(
    text: &str,
    lang: &Fragments<'_>,
    parse: &dyn Fn(&str) -> Option<Tree>,
) -> Result<Compiled, String> {
    if text.trim().is_empty() {
        return Err("a pattern cannot be empty".into());
    }
    let expanded = expand(text, lang.expando);
    // A lone metavariable has no shape of its own: it matches any node.
    if let Some(var @ MetaVar::One { .. }) = meta_variable(&expanded, lang.expando) {
        return Ok(Compiled {
            forms: vec![Form {
                pattern: Pattern::new(crate::pattern::PatternNode::Var(var)),
                context: None,
            }],
            tolerant: false,
        });
    }
    let mut attempts = Vec::new();
    let mut strict = Vec::new();
    let (primary, tolerant) = if let Some(form) = first(
        &expanded,
        lang.contexts,
        true,
        lang,
        false,
        parse,
        &mut strict,
    ) {
        (form, false)
    } else if let Some(form) = first(
        &expanded,
        lang.contexts,
        true,
        lang,
        true,
        parse,
        &mut Vec::new(),
    ) {
        (form, true)
    } else if let Some(form) = first(
        &expanded,
        lang.also,
        false,
        lang,
        false,
        parse,
        &mut attempts,
    ) {
        (form, false)
    } else if let Some(terminated) = terminate_lone_sequences(text)
        && let Ok(compiled) = compile(&terminated, lang, parse)
    {
        // `{ $$$B; }` read in place of `{ $$$B }`; `$$$B;` is the sequence.
        return Ok(compiled);
    } else {
        strict.extend(attempts);
        return Err(format!(
            "`{text}` is not valid code in this language: {}",
            strict.join("; ")
        ));
    };
    let mut forms = vec![primary];
    let add = |form: Form, forms: &mut Vec<Form>| {
        if forms.iter().all(|f| f.pattern != form.pattern) {
            forms.push(form);
        }
    };
    for context in lang.also {
        let attempt = read(&expanded, Some(context), lang.expando, false, parse)
            .or_else(|_| read(&expanded, Some(context), lang.expando, true, parse));
        if let Ok(form) = attempt {
            add(form, &mut forms);
        }
    }
    // Every context after the one it was read in, or all of them when it
    // read bare: the same text may mean something else there too.
    let after = match forms[0].context.as_deref() {
        None => Some(0),
        Some(c) => lang.contexts.iter().position(|k| *k == c).map(|i| i + 1),
    };
    if let Some(after) = after {
        for context in &lang.contexts[after..] {
            if let Ok(form) = read(&expanded, Some(context), lang.expando, tolerant, parse) {
                add(form, &mut forms);
            }
        }
    }
    Ok(Compiled { forms, tolerant })
}

/// `text` with a `;` after each sequence capture that is alone in braces
/// (`{ $$$B }` → `{ $$$B; }`) or ends a case (`case X: $$$B` →
/// `case X: $$$B;`), or `None` when it has none. C++ reads
/// `void f() { $$$B }` as a declaration with a brace initializer, and Java
/// a `case` needs a statement; with the `;` it is a function or a case, and
/// `$$$B;` is read as the sequence itself.
pub fn terminate_lone_sequences(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len() + 4);
    let mut rest = text;
    let mut changed = false;
    while let Some(at) = rest.find("$$$") {
        let (head, tail) = rest.split_at(at);
        out.push_str(head);
        let len = 3 + tail[3..]
            .find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
            .unwrap_or(tail.len() - 3);
        let (var, after) = tail.split_at(len);
        out.push_str(var);
        let before = out[..out.len() - var.len()].trim_end();
        let after_trimmed = after.trim_start();
        if (before.ends_with('{') && after_trimmed.starts_with('}'))
            || (before.ends_with(':') && after_trimmed.is_empty())
        {
            out.push(';');
            changed = true;
        }
        rest = after;
    }
    out.push_str(rest);
    changed.then_some(out)
}
