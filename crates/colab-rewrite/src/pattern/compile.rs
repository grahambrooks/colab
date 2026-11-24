//! Compiling a pattern: the bare text or a context wrapper, every other
//! context and `also` wrapper it reads differently in, the qualified-call
//! form, the shape a match must fit, and the prefilter all forms share.

use super::*;

/// Replace each metavariable `$` with `expando`, leaving every other `$`
/// (a PHP variable, a `$` inside a string) alone.
pub(super) fn expand(query: &str, expando: char) -> String {
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
pub(super) fn is_lone_metavariable(text: &str) -> bool {
    let Some(name) = text.strip_prefix('$') else {
        return false;
    };
    !name.is_empty()
        && !name.starts_with('$')
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

pub(super) fn is_var_start(c: char) -> bool {
    c.is_ascii_uppercase() || c == '_'
}

/// Words that can stand before `(` without being a callee.
const NOT_CALLEES: &[&str] = &[
    "as", "async", "await", "box", "break", "const", "crate", "dyn", "else", "fn", "for", "if",
    "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self",
    "Self", "static", "super", "type", "unsafe", "use", "where", "while", "yield",
];

/// When `text` starts with a bare call (`f(`, or `f::<T>(` with `sep` =
/// `::`) or a struct literal of a type named bare (`FileChange {`: an
/// upper-case name, so a keyword's block never counts), the same text with
/// [`QUALIFIER`] and `sep` before it, and the callee's (or type's) name.
/// `None` for anything else: a metavariable callee, a path that is already
/// qualified, a macro, a keyword.
pub(super) fn qualify_call<'t>(text: &'t str, sep: &str) -> Option<(String, &'t str)> {
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
    let literal = rest.starts_with('{') && first.is_uppercase();
    (rest.starts_with('(') || turbofish || literal)
        .then(|| (format!("{lead}{QUALIFIER}{sep}{body}"), callee))
}

/// The fields of `text` written `name: $VAR` in a struct-literal place:
/// right after `{` or `,` and right before `,` or `}` (whitespace aside),
/// with `$VAR` a single metavariable the pattern names nowhere else. So
/// `let x: $T = …` and `fn f(a: $T)` never count. Each is `(the bytes
/// from the name to the end of $VAR, name, VAR)`.
pub(super) fn shorthand_fields(text: &str) -> Vec<(std::ops::Range<usize>, String, String)> {
    let bytes = text.as_bytes();
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        // A name starts after a `{` or `,` (skipping whitespace).
        if !(bytes[i].is_ascii_lowercase() || bytes[i] == b'_') || (i > 0 && ident(bytes[i - 1])) {
            i += 1;
            continue;
        }
        let before = text[..i].trim_end();
        let start = i;
        while i < bytes.len() && ident(bytes[i]) {
            i += 1;
        }
        let name = &text[start..i];
        if !(before.ends_with('{') || before.ends_with(',')) {
            continue;
        }
        let Some(value) = text[i..].trim_start().strip_prefix(':') else {
            continue;
        };
        if value.starts_with(':') {
            continue; // a path, `a::b`
        }
        let Some(var) = value.trim_start().strip_prefix('$') else {
            continue;
        };
        if var.starts_with('$') {
            continue; // `$$$` takes many nodes
        }
        let len = var
            .find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
            .unwrap_or(var.len());
        let var_name = &var[..len];
        let end = text.len() - var.len() + len;
        let next = text[end..].trim_start();
        let once = text.matches(&format!("${var_name}")).count() == 1;
        if !var_name.is_empty() && once && (next.starts_with(',') || next.starts_with('}')) {
            out.push((start..end, name.to_string(), var_name.to_string()));
        }
        i = end;
    }
    out
}

/// A template for a match of a shorthand form (see
/// [`shorthand_fields`]): each `$VAR` is the field's name, and a field
/// that ends up `name: name` is written `name`.
pub(super) fn shorthand_template(template: &str, fields: &[(String, String)]) -> String {
    let mut out = template.to_string();
    for (name, var) in fields {
        let token = format!("${var}");
        let mut at = 0;
        while let Some(found) = out[at..].find(&token) {
            let start = at + found;
            let end = start + token.len();
            let longer = out[end..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
            if longer || out[..start].ends_with('$') {
                at = end;
                continue;
            }
            out.replace_range(start..end, name);
            at = start + name.len();
        }
        out = out.replace(&format!("{name}: {name}"), name);
    }
    out
}

/// What the qualified form of a bare-call pattern needs to rewrite.
#[derive(Debug, Clone)]
pub(super) struct Qualified {
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
    pub(super) fn template(&self, template: &str) -> Option<String> {
        let (qualified, callee) = qualify_call(template, self.separator)?;
        let renamed = !template.contains(self.callee.as_str());
        (callee == self.callee || renamed).then_some(qualified)
    }
}

/// What the form of an item pattern compiled with the modifiers sentinel
/// (`@ColabMods void f()` for a pattern written `void f()`) needs.
#[derive(Clone)]
pub(super) struct Visible {
    /// The language, to read a template as code.
    language: PatternLanguage,
    modifiers: Modifiers,
    /// For [`Modifiers::several`]: `Some(k)` on a form that reads exactly
    /// `k` modifiers, one placeholder each (see [`modifier_forms`]); `None`
    /// on the one whose `$$$` capture reads them all.
    count: Option<usize>,
}

impl Visible {
    /// The template to fill for a match of this form: a template that is
    /// an item which spells no modifiers — it parses with the sentinel in
    /// front, as the modifiers — gets the matched ones back, so a rewrite
    /// keeps a `public static`. Any other template (`$$$B`, which unwraps
    /// the item; one that spells its own; one that is not an item) is used
    /// as written.
    pub(super) fn template(&self, template: &str) -> Option<String> {
        let trimmed = template.trim_start();
        if trimmed.starts_with("$$$") || self.leads_with_modifiers(trimmed) {
            return None;
        }
        let prefixed = format!("{} {trimmed}", self.modifiers.sentinel);
        let sigil = if self.modifiers.several { "$$$" } else { "$" };
        self.takes_modifiers(&prefixed)
            .then(|| format!("{sigil}{MODIFIERS} {trimmed}"))
    }

    /// Whether `text`, read as a pattern, starts with modifiers.
    fn leads_with_modifiers(&self, text: &str) -> bool {
        StructuralPattern::compile_one(&self.language, text, true)
            .is_ok_and(|p| leads_with(&p.tree, self.modifiers.kinds))
    }

    /// Whether `text`, written with the sentinel in front, reads with the
    /// sentinel as its modifiers: the [`MODIFIERS`] metavariable, first.
    fn takes_modifiers(&self, text: &str) -> bool {
        StructuralPattern::compile_one(&self.language, text, true)
            .is_ok_and(|p| self.reads_as_modifiers(&p))
    }

    /// Whether a compiled form starts with the sentinel as its modifiers.
    pub(super) fn reads_as_modifiers(&self, form: &StructuralPattern) -> bool {
        leads_with(&form.tree, self.modifiers.kinds)
            && form.pattern.defined_vars().contains(MODIFIERS)
    }

    /// `false` for a match of this form that binds no modifiers: the
    /// sentinel's `$$$` capture also matches none, and the pattern as
    /// written matches those items already. A form that reads exactly `k`
    /// keeps a match only when the item's leading modifiers are those `k`,
    /// and binds them as [`MODIFIERS`] for the template.
    pub(super) fn bound<D: ast_grep_core::Doc>(&self, m: &mut NodeMatch<'_, D>) -> bool {
        if !self.modifiers.several {
            return true;
        }
        let Some(k) = self.count else {
            return !m.get_env().get_multiple_matches(MODIFIERS).is_empty();
        };
        let leading: Vec<_> = m
            .get_node()
            .children()
            .filter(|c| c.is_named() && !c.is_extra())
            .take_while(|c| self.modifiers.kinds.contains(&c.kind().as_ref()))
            .collect();
        leading.len() == k && m.get_env_mut().insert_multi(MODIFIERS, leading).is_some()
    }
}

/// The most modifiers a form reads one placeholder each for (`[Obsolete]
/// public static async`): an item with more is matched only by a pattern
/// that spells some of them.
const MOST_MODIFIERS: usize = 4;

/// The form or forms that read [`Modifiers::several`] modifiers, from
/// `alt`, compiled with the sentinel as a `$$$` [`MODIFIERS`] capture.
///
/// ast-grep ends an ellipsis at the first later goal a candidate child
/// matches, after skipping the goal's anonymous tokens. When the goal is a
/// keyword (PHP's `$$$M function $F() {}`), marking it named makes it the
/// goal: it matches only the candidate's own `function`, so the capture
/// takes every modifier before it, and one form does. When the goal is a
/// metavariable (C#'s `$$$M $T $F() {}`), it matches the first modifier,
/// so no capture can end in the right place: then the form is `k` named
/// placeholders, for each `k` up to [`MOST_MODIFIERS`], and
/// [`Visible::bound`] keeps a match only when they are the item's `k`
/// modifiers.
fn modifier_forms(mut alt: StructuralPattern, visible: &Visible) -> Vec<StructuralPattern> {
    use ast_grep_core::matcher::PatternNode;
    let Some(children) = modifiers_parent(&mut alt.pattern.node) else {
        return Vec::new();
    };
    let next = children[1..].iter().find(|c| !c.is_trivial());
    if !matches!(next, Some(PatternNode::MetaVar { .. })) {
        for child in children.iter_mut().skip(1) {
            match child {
                PatternNode::Terminal { is_named, .. } if !*is_named => *is_named = true,
                _ => break,
            }
        }
        alt.visible = Some(visible.clone());
        return vec![alt];
    }
    (1..=MOST_MODIFIERS)
        .map(|k| {
            let mut form = alt.clone_form();
            if let Some(children) = modifiers_parent(&mut form.pattern.node) {
                let placeholder = PatternNode::MetaVar {
                    meta_var: MetaVariable::Dropped(true),
                };
                children.splice(0..1, std::iter::repeat_n(placeholder, k));
            }
            form.visible = Some(Visible {
                count: Some(k),
                ..visible.clone()
            });
            form
        })
        .collect()
}

/// The most leading modifiers a form for
/// [`PatternLanguage::modifier_tokens`] reads (`private static async *`).
const MOST_LED: usize = 4;

/// For an item pattern of a language whose modifiers no sentinel can
/// stand for ([`PatternLanguage::modifier_tokens`]), the forms that read
/// `k` leading modifiers first, `k` = 1..=[`MOST_LED`]: `k` placeholders
/// that match any child, each kept only when they are the item's
/// modifiers (`led`). `None` when `form` is not an item that leaves its
/// modifiers out: its first child is spelled (`static`, `private`,
/// `function`), or it is not a member or a function.
///
/// ast-grep skips a candidate's anonymous tokens before a literal, but not
/// before a metavariable, and never a named child: without these forms
/// `$M() {}` misses `static m() {}`, and `m() {}` misses `private m() {}`.
fn led_forms(
    form: &StructuralPattern,
    tokens: &'static [&'static str],
) -> Option<Vec<StructuralPattern>> {
    use ast_grep_core::matcher::PatternNode;
    let root = form.tree.lines().next().unwrap_or("");
    let item = ["method", "field", "function", "property", "signature"]
        .iter()
        .any(|w| root.contains(w));
    let PatternNode::Internal { children, .. } = &form.pattern.node else {
        return None;
    };
    // The very first child: a spelled `static` is an unnamed terminal,
    // which ast-grep counts as trivial.
    let first = children.first()?;
    let leaves_them_out = match first {
        PatternNode::MetaVar { .. } => true,
        PatternNode::Terminal { is_named, .. } => *is_named,
        PatternNode::Internal { .. } => false,
    };
    if !item || !leaves_them_out {
        return None;
    }
    let shape = match &form.shape {
        Shape::Node(goals) => Shape::Node(
            std::iter::once(Shape::Modifiers(tokens))
                .chain(goals.iter().cloned())
                .collect(),
        ),
        other => other.clone(),
    };
    Some(
        (1..=MOST_LED)
            .map(|k| {
                let mut pattern = form.pattern.clone();
                if let PatternNode::Internal { children, .. } = &mut pattern.node {
                    let placeholder = PatternNode::MetaVar {
                        meta_var: MetaVariable::Dropped(false),
                    };
                    children.splice(0..0, std::iter::repeat_n(placeholder, k));
                }
                StructuralPattern {
                    lang: form.lang.clone(),
                    pattern,
                    text: form.text.clone(),
                    prefilter: form.prefilter.clone(),
                    context: form.context.clone(),
                    tree: form.tree.clone(),
                    checks: Vec::new(),
                    constraints: Vec::new(),
                    lets: Vec::new(),
                    also: Vec::new(),
                    qualified: None,
                    visible: None,
                    shorthand: form.shorthand.clone(),
                    loose_semicolon: form.loose_semicolon,
                    shape: shape.clone(),
                    led: Some(k),
                }
            })
            .collect(),
    )
}

/// How many of `node`'s leading children are modifiers, by `tokens`
/// (anonymous keywords and named kinds alike).
pub(super) fn leading_modifiers<D: ast_grep_core::Doc>(
    node: &ast_grep_core::Node<'_, D>,
    tokens: &[&str],
) -> usize {
    node.children()
        .filter(|c| !c.is_extra())
        .take_while(|c| tokens.contains(&c.kind().as_ref()))
        .count()
}

/// The children of the pattern node whose first child is the `$$$`
/// [`MODIFIERS`] capture.
fn modifiers_parent(
    node: &mut ast_grep_core::matcher::PatternNode,
) -> Option<&mut Vec<ast_grep_core::matcher::PatternNode>> {
    use ast_grep_core::matcher::PatternNode;
    let PatternNode::Internal { children, .. } = node else {
        return None;
    };
    let leads = matches!(
        children.first(),
        Some(PatternNode::MetaVar { meta_var: MetaVariable::MultiCapture(name) }) if name == MODIFIERS
    );
    if leads {
        return Some(children);
    }
    children.iter_mut().find_map(modifiers_parent)
}

/// Whether the item a printed pattern tree ([`print_tree`]) starts with
/// has a first child of one of `kinds`: its modifiers.
fn leads_with(tree: &str, kinds: &[&str]) -> bool {
    tree.lines()
        .nth(1)
        .and_then(|line| line.split_whitespace().find(|w| !w.ends_with(':')))
        .is_some_and(|kind| kinds.contains(&kind))
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
pub(super) enum Shape {
    /// A metavariable or a literal leaf: anything ast-grep accepted.
    Any,
    /// A `$$$` sequence.
    Many,
    /// The modifiers sentinel as a `$$$` sequence ([`Modifiers::several`]):
    /// it absorbs a candidate's leading children of these kinds, and no
    /// others, so the rest of the item is held to the pattern still.
    Modifiers(&'static [&'static str]),
    /// A modifiers node the pattern spells (`public` in `public void
    /// f()`), with this many tokens: its keywords are anonymous, which
    /// ast-grep lets trail, so without the count `public` would match
    /// `public static`.
    Tokens(usize),
    /// An inner node and its named, non-comment children.
    Node(Vec<Shape>),
}

impl Shape {
    fn of(node: Node<'_>, source: &str, lang: &TsLang) -> Self {
        let expando = lang.expando;
        let text = node.utf8_text(source.as_bytes()).unwrap_or("").trim();
        if let Some(modifiers) = lang.modifiers
            && text == modifiers.sentinel
        {
            return if modifiers.several {
                Self::Modifiers(modifiers.kinds)
            } else {
                Self::Any
            };
        }
        if let Some(modifiers) = lang.modifiers
            && !modifiers.several
            && modifiers.kinds.contains(&node.kind())
            && node.child_count() > 0
        {
            let mut cursor = node.walk();
            return Self::Tokens(node.children(&mut cursor).filter(|c| !c.is_extra()).count());
        }
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
                .map(|c| Self::of(c, source, lang))
                .collect(),
        )
    }

    /// `true` unless `candidate`, which ast-grep matched against this
    /// shape, has named children the pattern leaves out.
    pub(super) fn fits<D: ast_grep_core::Doc>(
        &self,
        candidate: &ast_grep_core::Node<'_, D>,
    ) -> bool {
        let goals = match self {
            Self::Node(goals) => goals,
            Self::Tokens(n) => return candidate.children().filter(|c| !c.is_extra()).count() == *n,
            _ => return true,
        };
        let (goals, absorbed) = match goals.split_first() {
            Some((Self::Modifiers(kinds), rest)) => (rest, *kinds),
            _ => (goals.as_slice(), &[][..]),
        };
        if goals.contains(&Self::Many) {
            return true;
        }
        let mut named: Vec<_> = candidate
            .children()
            .filter(|c| c.is_named() && !c.is_extra())
            .collect();
        let modifiers = named
            .iter()
            .take_while(|c| absorbed.contains(&c.kind().as_ref()))
            .count();
        named.drain(..modifiers);
        if named.len() != goals.len() {
            return named.len() < goals.len();
        }
        goals.iter().zip(&named).all(|(g, c)| g.fits(c))
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
        // A trailing `;` that is not part of the node: a TypeScript class
        // field's `;` is its sibling, so `count: number = $V;` is no single
        // node in a class. When the pattern is refused with it, or reads the
        // same without it, it is compiled without it (every form too), and
        // still shown as written.
        let read = Self::readings(language, text);
        if let Some(stripped) = text.trim_end().strip_suffix(';')
            && !stripped.trim().is_empty()
            && !stripped.trim_end().ends_with(';')
        {
            let same = match &read {
                Err(_) => true,
                Ok(with) => Self::readings(language, stripped).is_ok_and(|w| w.tree == with.tree),
            };
            if same && let Ok(mut loose) = Self::compile(language, stripped) {
                loose.text = text.to_string();
                // The match no longer holds the `;`, so neither may the
                // template: `x: T = $V;` → `x: T = f($V);` keeps one.
                loose.loose_semicolon = true;
                for form in &mut loose.also {
                    form.loose_semicolon = true;
                }
                return Ok(loose);
            }
        }
        let mut primary = read?;
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
        // `void $F() {}` also as `@ColabMods void $F() {}`, the sentinel
        // reading as the modifiers and binding them: an item's modifiers
        // are named children, which a pattern without them never matches,
        // so `fn f` would silently miss `pub fn f`. A pattern that spells
        // modifiers stays exact (its tree leads with them, and the sentinel
        // joins them rather than standing for them); one that is not an
        // item does not read the sentinel as modifiers and gets no form.
        // Each reading of the sentinel text that takes it as modifiers is a
        // form (C#: a local function, and a method in a class).
        if let Some(modifiers) = language.modifiers
            && !leads_with(&primary.tree, modifiers.kinds)
        {
            let visible = Visible {
                language: language.clone(),
                modifiers,
                count: None,
            };
            let prefixed = format!("{} {}", modifiers.sentinel, text.trim_start());
            if let Ok(mut first) = Self::readings(language, &prefixed) {
                let others = std::mem::take(&mut first.also);
                for mut alt in std::iter::once(first).chain(others) {
                    if visible.reads_as_modifiers(&alt) {
                        // The form's literals are the pattern's (the
                        // sentinel is a metavariable), so the pattern's
                        // prefilter holds for it.
                        alt.prefilter.clone_from(&primary.prefilter);
                        if modifiers.several {
                            primary.also.extend(modifier_forms(alt, &visible));
                        } else {
                            alt.visible = Some(visible.clone());
                            primary.also.push(alt);
                        }
                    }
                }
            }
        }
        // `$M(): void {}` also as `• $M(): void {}`, `• • $M(): void {}`, …
        // where a modifier is a token the grammar leaves anonymous
        // (JavaScript's `static`) or a named one no text can stand for
        // (TypeScript's `private`), so no sentinel form reads them.
        if !language.modifier_tokens.is_empty() {
            let mut added = Vec::new();
            if let Some(led) = led_forms(&primary, language.modifier_tokens) {
                primary.led = Some(0);
                added.extend(led);
            }
            for alt in &mut primary.also {
                if alt.visible.is_none()
                    && alt.qualified.is_none()
                    && let Some(led) = led_forms(alt, language.modifier_tokens)
                {
                    alt.led = Some(0);
                    added.extend(led);
                }
            }
            primary.also.extend(added);
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
        // `S { text: $T }` also as `S { text }`, the shorthand for a field
        // set from a variable of its name: every subset of the fields
        // written `name: $VAR` (up to three; all of them beyond), each a
        // form only when it parses as a different tree.
        if language.shorthand_fields {
            let fields = shorthand_fields(text);
            let subsets: Vec<Vec<usize>> = if fields.len() <= 3 {
                (1..1usize << fields.len())
                    .map(|bits| (0..fields.len()).filter(|i| bits & (1 << i) != 0).collect())
                    .collect()
            } else {
                vec![(0..fields.len()).collect()]
            };
            for subset in subsets {
                let mut alt_text = text.to_string();
                for &i in subset.iter().rev() {
                    let (range, name, _) = &fields[i];
                    alt_text.replace_range(range.clone(), name);
                }
                if let Ok(mut alt) = Self::compile_one(language, &alt_text, true)
                    && alt.tree != primary.tree
                {
                    // Only `: $VAR`s were taken out, so the pattern's
                    // literals are all still there.
                    alt.prefilter.clone_from(&primary.prefilter);
                    alt.shorthand = Some(
                        subset
                            .iter()
                            .map(|&i| (fields[i].1.clone(), fields[i].2.clone()))
                            .collect(),
                    );
                    primary.also.push(alt);
                }
            }
        }
        primary.prefilter = shared_prefilter(&primary);
        Ok(primary)
    }

    /// A copy of a form as compiled, before any `where` constraint or
    /// `also` form is added (those are compiled for each form after).
    fn clone_form(&self) -> Self {
        debug_assert!(self.checks.is_empty() && self.lets.is_empty() && self.also.is_empty());
        Self {
            lang: self.lang.clone(),
            pattern: self.pattern.clone(),
            text: self.text.clone(),
            prefilter: self.prefilter.clone(),
            context: self.context.clone(),
            tree: self.tree.clone(),
            checks: Vec::new(),
            constraints: Vec::new(),
            lets: Vec::new(),
            also: Vec::new(),
            qualified: self.qualified.clone(),
            visible: self.visible.clone(),
            shorthand: self.shorthand.clone(),
            loose_semicolon: self.loose_semicolon,
            shape: self.shape.clone(),
            led: None,
        }
    }

    /// `text` compiled in the first context it parses in, with every other
    /// context that reads it as a different tree among its `also` forms.
    fn readings(language: &PatternLanguage, text: &str) -> Result<Self, String> {
        let alt_in = |context: &'static &'static str| {
            let only = PatternLanguage {
                contexts: std::slice::from_ref(context),
                also: &[],
                ..language.clone()
            };
            Self::compile_one(&only, text, false)
        };
        // Text that is code only in an `also` context (a Python keyword
        // argument, a dict entry) compiles there. `tolerant`: nothing read
        // it cleanly, and it compiled with a placeholder-only error (a lone
        // `$$$B` body, which C# and Java keep in an `ERROR` node).
        let compiled = Self::compile_with(language, text, true, false)
            .map(|p| (p, false))
            .or_else(|strict| {
                Self::compile_with(language, text, true, true)
                    .map(|p| (p, true))
                    .map_err(|_| strict)
            });
        let (mut primary, tolerant) = match compiled {
            Ok(compiled) => compiled,
            Err(e) => language
                .also
                .iter()
                .find_map(|context| alt_in(context).ok())
                .map(|p| (p, false))
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
        // A pattern that compiled bare (C#'s top-level statements take
        // `void $F() {}` as a local function) is tried in every context.
        // One that only compiled tolerantly is tried tolerantly in them too
        // (no context reads it cleanly), or `void $F() { $$$B }` would be a
        // local function only and never match a method.
        let after = match primary.context.as_deref() {
            None => Some(0),
            Some(c) => language
                .contexts
                .iter()
                .position(|k| *k == c)
                .map(|index| index + 1),
        };
        if let Some(after) = after {
            for context in &language.contexts[after..] {
                let only = PatternLanguage {
                    contexts: std::slice::from_ref(context),
                    also: &[],
                    ..language.clone()
                };
                if let Ok(alt) = Self::compile_with(&only, text, false, tolerant)
                    && alt.tree != primary.tree
                    && primary.also.iter().all(|a| a.tree != alt.tree)
                {
                    primary.also.push(alt);
                }
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
            leading: language.leading,
            macro_contents: language.macro_contents,
            modifiers: language.modifiers,
            modifier_tokens: language.modifier_tokens,
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
                    let shape = Shape::of(root, &expanded, &lang);
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
            let Some(node) = deepest_covering(tree.root_node(), &filled_expanded, trimmed.clone())
            else {
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
                    let shape = Shape::of(node, &filled_expanded, &lang);
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
            visible: None,
            shorthand: None,
            loose_semicolon: false,
            shape,
            led: None,
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
        let mut also = Vec::new();
        for alt in std::mem::take(&mut self.also) {
            let shorthand = alt.shorthand.is_some();
            match alt.constrain(language, constraints) {
                Ok(alt) => also.push(alt),
                // A shorthand form binds no `$VAR` for the field it writes
                // bare, so a constraint on it cannot be judged there: the
                // form is left out, and the rule matches as written.
                Err(_) if shorthand => {}
                Err(e) => return Err(e),
            }
        }
        self.also = also;
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
}

/// A literal every form of `pattern` needs, so the rule's prefilter stays
/// sound however many forms it matches in: each form's literal must be in
/// a file for that form to match, so the one contained in all the others
/// is in every file any form matches. `None` when no form's literal is in
/// all the others.
pub(super) fn shared_prefilter(pattern: &StructuralPattern) -> Option<String> {
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
