//! Printing a pattern's or snippet's syntax tree (`colab ast`), and the
//! report that explains how a pattern parsed. Printed from colab's own
//! tree, whichever parser made it.

use super::*;
use colab_syntax::Node;

/// The most lines [`syntax_tree`] prints before eliding the rest.
pub const MAX_TREE_LINES: usize = 2000;

/// Parse `source` and print its named-node tree, for debugging a pattern
/// against real code. Returns the printed tree and whether the parse hit
/// a syntax error.
pub fn syntax_tree(language: &PatternLanguage, source: &str) -> Result<(String, bool), String> {
    printed(crate::syntax::fragment_tree(language, source), source)
}

/// [`syntax_tree`] of a tree a backend parsed itself (Markdown's).
pub fn print_tree_of(tree: &colab_syntax::Tree) -> (String, bool) {
    (
        print_tree(tree.root(), tree.text(), '$', true),
        tree.has_error(),
    )
}

fn printed(tree: Option<colab_syntax::Tree>, source: &str) -> Result<(String, bool), String> {
    let tree =
        tree.ok_or_else(|| "the parser gave up (or the grammar failed to load)".to_string())?;
    Ok((print_tree(tree.root(), source, '$', true), tree.has_error()))
}

/// One named node per line, indented by depth:
/// `field: kind  L:C` plus the text for leaves (positions only when
/// `positions` — a pattern's would be relative to a hidden context). Unnamed tokens
/// (punctuation, keywords) are left out — they are not what a pattern's
/// structure is compared on. `expando` is shown as `$`.
pub(super) fn print_tree(node: Node<'_>, source: &str, expando: char, positions: bool) -> String {
    let mut out = String::new();
    let mut lines = 0usize;
    let style = Style { expando, positions };
    // Depth first with an explicit stack: (node, depth).
    let mut stack = vec![(node, 0usize)];
    while let Some((node, depth)) = stack.pop() {
        let mut child_depth = depth;
        if node.is_named() || node.is_error() || node.is_missing() {
            lines += 1;
            if lines <= MAX_TREE_LINES {
                print_line(node, source, style, depth, &mut out);
            }
            child_depth += 1;
        }
        let children: Vec<Node<'_>> = node.children().filter(|c| !c.is_trivia()).collect();
        stack.extend(children.into_iter().rev().map(|c| (c, child_depth)));
    }
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

fn print_line(node: Node<'_>, source: &str, style: Style, depth: usize, out: &mut String) {
    let field = node.field_name().map_or_default(|f| format!("{f}: "));
    let kind = if node.is_missing() {
        format!("MISSING {}", node.kind_name())
    } else {
        node.kind_name().to_string()
    };
    let _ = write!(out, "{}{}{}", "  ".repeat(depth), field, kind);
    if style.positions {
        let before = &source[..node.start()];
        let row = before.matches('\n').count();
        let column = before.len() - before.rfind('\n').map_or(0, |p| p + 1);
        let _ = write!(out, "  {}:{}", row + 1, column + 1);
    }
    if node.named_children().next().is_none() {
        let shown: String = node.text().replace(style.expando, "$");
        let shown = if shown.len() > 80 {
            format!("{}…", shown.chars().take(80).collect::<String>())
        } else {
            shown
        };
        let _ = write!(out, "  {shown:?}");
    }
    out.push('\n');
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

/// The printed tree of the node `text` (a pattern) spans when read in
/// `context` (bare when `None`), metavariables shown with `$`, as the
/// first form compiled it; without a `;` its node does not hold, as a
/// loose form compiled it. `None` for a lone metavariable.
fn pattern_tree(language: &PatternLanguage, text: &str, context: Option<&str>) -> Option<String> {
    let read = |text: &str| -> Option<String> {
        let expanded = colab_syntax::pattern::expand(text, language.expando);
        let (filled, slot) = match context {
            Some(c) => (
                c.replacen(PATTERN_SLOT, &expanded, 1),
                c.find(PATTERN_SLOT)?,
            ),
            None => (expanded.clone(), 0),
        };
        let tree = crate::syntax::fragment_tree(language, &filled)?;
        let span = trim_span(&filled, slot..slot + expanded.len());
        let node = deepest_covering(tree.root(), &filled, span)?;
        Some(print_tree(node, &filled, language.expando, false))
    };
    read(text).or_else(|| read(text.trim_end().strip_suffix(';')?))
}

/// The tree for a snippet (`is_pattern == false`) or for `text` compiled
/// as a pattern exactly as a `<lang>::pattern` rule would compile it.
pub fn describe(
    language: &PatternLanguage,
    text: &str,
    is_pattern: bool,
) -> Result<TreeReport, String> {
    if is_pattern {
        let forms = crate::syntax::compile_forms(language, text)?;
        let form = &forms.forms[0];
        Ok(TreeReport {
            tree: pattern_tree(language, text, form.context.as_deref())
                .unwrap_or_else(|| "(any node)".to_string()),
            has_error: false,
            context: form.context.clone(),
            prefilter: crate::syntax::prefilter(&forms),
        })
    } else {
        let (tree, has_error) = syntax_tree(language, text)?;
        Ok(TreeReport {
            tree,
            has_error,
            context: None,
            prefilter: None,
        })
    }
}
