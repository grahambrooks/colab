//! Parsing patterns: trying each context wrapper, trimming the match to the
//! pattern's own node, and the named-leaf literal used as a prefilter.

use super::*;

/// Parse `text`, failing if the tree contains a syntax error.
/// `source` as ast-grep's tree, parsed once per text (`parse_cache`).
pub(super) fn parsed(lang: &TsLang, source: &str) -> ast_grep_core::AstGrep<StrDoc<TsLang>> {
    match crate::parse_cache::parse(&lang.grammar, source) {
        Some(tree) => ast_grep_core::AstGrep::doc(StrDoc {
            src: source.to_string(),
            lang: lang.clone(),
            tree,
        }),
        None => lang.ast_grep(source),
    }
}

/// Every match of `pattern` in `grep` (the tree of `source`), in pre-order:
/// what `grep.root().find_all(pattern)` gives. When the tree is searched
/// again (a second rule, another form of the pattern), the parse cache's
/// kind index lets it visit only the nodes the pattern can start at,
/// instead of walking the whole tree once per rule.
pub(super) fn candidates<'t>(
    grep: &'t ast_grep_core::AstGrep<StrDoc<TsLang>>,
    source: &str,
    pattern: &Pattern,
) -> Vec<NodeMatch<'t, StrDoc<TsLang>>> {
    use ast_grep_core::matcher::{Matcher, MatcherExt};
    let root = grep.root();
    let ts_root = root.get_inner_node();
    if let Some(kinds) = pattern.potential_kinds()
        && let Some(index) = crate::parse_cache::kind_index(source, ts_root)
    {
        let mut cursor = ts_root.walk();
        return index
            .positions(kinds.iter())
            .into_iter()
            .filter_map(|position| {
                cursor.goto_descendant(position as usize);
                pattern.match_node(grep.adopt(cursor.node()))
            })
            .collect();
    }
    root.find_all(pattern).collect()
}

/// Line and column (1-based, the column in characters) of offsets met in
/// ascending order, counting each line break once rather than from the
/// start of the text for every offset.
pub(super) struct Lines<'s> {
    source: &'s str,
    at: usize,
    line: usize,
    line_start: usize,
}

impl<'s> Lines<'s> {
    pub(super) fn new(source: &'s str) -> Self {
        Self {
            source,
            at: 0,
            line: 1,
            line_start: 0,
        }
    }

    pub(super) fn at(&mut self, offset: usize) -> (usize, usize) {
        if offset < self.at {
            return line_column(self.source, offset);
        }
        let skipped = &self.source[self.at..offset];
        if let Some(last) = skipped.rfind('\n') {
            self.line += skipped.matches('\n').count();
            self.line_start = self.at + last + 1;
        }
        self.at = offset;
        (
            self.line,
            self.source[self.line_start..offset].chars().count() + 1,
        )
    }
}

pub(super) fn parse_clean(
    grammar: &tree_sitter::Language,
    text: &str,
) -> Result<tree_sitter::Tree, String> {
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
pub(super) fn parse_pattern(
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
            // A bare `$$$` (three expandos, no name) is the anonymous
            // sequence, as much a placeholder as `$$$B`.
            let anonymous = name.is_empty() && t.chars().count() == 3;
            anonymous
                || (!name.is_empty() && name.chars().all(|c| is_var_start(c) || c.is_ascii_digit()))
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
pub(super) fn trim_span(text: &str, span: std::ops::Range<usize>) -> std::ops::Range<usize> {
    let slice = &text[span.clone()];
    let start = span.start + (slice.len() - slice.trim_start().len());
    let end = span.end - (slice.len() - slice.trim_end().len());
    start..end
}

/// The deepest named node whose byte range is exactly `span`.
pub(super) fn deepest_covering<'t>(
    root: Node<'t>,
    text: &str,
    span: std::ops::Range<usize>,
) -> Option<Node<'t>> {
    let mut node = root.named_descendant_for_byte_range(span.start, span.end)?;
    // A node may take the whitespace after it into its range (Go's
    // `expression_case` ends at the newline that terminates it); past the
    // pattern there must be nothing else.
    let trailing_space = node.start_byte() == span.start
        && node.end_byte() > span.end
        && text
            .get(span.end..node.end_byte())
            .is_some_and(|t| t.trim().is_empty());
    if node.byte_range() != span && !trailing_space {
        return None;
    }
    let span = node.byte_range();
    // Descend through single-child wrappers that cover the same span
    // (`expression_statement` → `call_expression`).
    loop {
        let child = node
            .named_children(&mut node.walk())
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
pub(super) fn single_root(mut node: Node<'_>) -> Node<'_> {
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
pub(super) fn first_of_kind<'t>(root: Node<'t>, kind: &str) -> Option<Node<'t>> {
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
pub(super) fn longest_literal(node: Node<'_>, source: &str, expando: char) -> Option<String> {
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

pub(super) fn line_column(source: &str, offset: usize) -> (usize, usize) {
    let before = &source[..offset];
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    (line, source[line_start..offset].chars().count() + 1)
}
