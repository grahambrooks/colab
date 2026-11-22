//! Printing a pattern's or snippet's syntax tree (`colab ast`), and the
//! report that explains how a pattern parsed.

use super::*;

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
pub(super) fn print_tree(node: Node<'_>, source: &str, expando: char, positions: bool) -> String {
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
pub(super) struct Style {
    expando: char,
    positions: bool,
}

pub(super) fn print_node(
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
            let field = cursor.field_name().map_or_default(|f| format!("{f}: "));
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
