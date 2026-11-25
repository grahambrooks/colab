//! Parsing helpers: a clean tree-sitter parse (the qualifier check), and
//! the node a pattern's text spans in its context (`colab ast`'s printed
//! tree, on colab's own tree).

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

/// Narrow `span` to exclude surrounding whitespace.
pub(super) fn trim_span(text: &str, span: std::ops::Range<usize>) -> std::ops::Range<usize> {
    let slice = &text[span.clone()];
    let start = span.start + (slice.len() - slice.trim_start().len());
    let end = span.end - (slice.len() - slice.trim_end().len());
    start..end
}

/// The deepest named node whose byte range is exactly `span`.
pub(super) fn deepest_covering<'t>(
    root: colab_syntax::Node<'t>,
    text: &str,
    span: std::ops::Range<usize>,
) -> Option<colab_syntax::Node<'t>> {
    let mut node = root.covering(span.clone())?;
    while !node.is_named() {
        node = node.parent()?;
    }
    // A node may take the whitespace after it into its range (Go's
    // `expression_case` ends at the newline that terminates it); past the
    // pattern there must be nothing else.
    let trailing_space = node.start() == span.start
        && node.end() > span.end
        && text
            .get(span.end..node.end())
            .is_some_and(|t| t.trim().is_empty());
    if node.range() != span && !trailing_space {
        return None;
    }
    let span = node.range();
    // Descend through single-child wrappers that cover the same span
    // (`expression_statement` → `call_expression`).
    loop {
        let child = node.named_children().find(|c| c.range() == span);
        match child {
            Some(c) => node = c,
            None => return Some(node),
        }
    }
}
