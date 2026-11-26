//! Markdown as a [`colab_syntax::Tree`], read by a parser of colab's own:
//! pure Rust, lossless, and shaped for what colab reads in Markdown
//! (headings, lists, pipe tables, fenced code, link destinations). Step 3
//! of the move off tree-sitter (docs/development-plan.md).
//!
//! Two trees, as before: [`parse`] gives a file's blocks (`block.rs`),
//! [`parse_inline`] the links in one span of inline text (`inline.rs`),
//! which the block tree holds as `inline` leaves. Kinds and fields are
//! named in `names.rs`.

mod block;
mod inline;
mod names;
mod tree;

use std::sync::Arc;

use colab_syntax::{BuildError, Grammar, SourceParser, Tree};

/// `source`'s blocks.
pub fn parse(source: &str) -> Result<Tree, BuildError> {
    let items = block::Blocks::new(source).document();
    tree::build(source, "document", &items)
}

/// The links, images, autolinks and code spans in a span of inline text.
pub fn parse_inline(text: &str) -> Result<Tree, BuildError> {
    let items = inline::items(text);
    tree::build(text, "inline", &items)
}

/// colab's Markdown parser (its block tree).
pub struct MarkdownParser;

/// The one parser.
pub static PARSER: MarkdownParser = MarkdownParser;

impl SourceParser for MarkdownParser {
    fn grammar(&self) -> Arc<Grammar> {
        tree::grammar()
    }

    fn parse(&self, text: &str) -> Tree {
        parse(text).unwrap_or_else(|_| {
            tree::build(text, "document", &[])
                .unwrap_or_else(|_| unreachable!("an empty document always builds"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "---\ntitle: x\n---\n\n# Title #\n\nText with [a](b.md \"t\") and ![i](p.png).\n\nSub\n---\n\n- one\n- two\n  - nested\n\n1. first\n\n| A | B |\n|---|:-:|\n| 1 | 2 |\n\n```rust title\ncode\n```\n\n[ref]: http://x \"T\"\n";

    fn kinds(tree: &Tree) -> Vec<(String, String)> {
        tree.nodes()
            .filter(|n| n.is_named() && n.id() != 0)
            .map(|n| (n.kind_name().to_string(), n.text().to_string()))
            .collect()
    }

    #[test]
    fn a_document_is_lossless_and_holds_its_blocks() {
        let tree = parse(SRC).unwrap();
        let text: String = tree.tokens().map(|t| t.text()).collect();
        assert_eq!(text, SRC);
        let k = kinds(&tree);
        let has = |kind: &str, text: &str| k.iter().any(|(a, b)| a == kind && b == text);
        assert!(has("minus_metadata", "---\ntitle: x\n---"), "{k:?}");
        assert!(has("inline", "Title #"));
        assert!(has("setext_h2_underline", "---"));
        assert!(has("list_marker_minus", "- "));
        assert!(has("list_item", "- nested\n\n"));
        assert!(has("pipe_table_cell", "A "));
        assert!(has("pipe_table_delimiter_cell", ":-:"));
        assert!(has("language", "rust"));
        assert!(has("code_fence_content", "code\n"));
        assert!(has("link_destination", "http://x"));
    }

    #[test]
    fn inline_links_and_images_are_read_and_code_is_not() {
        let text = "See [a](b.md \"t\"), ![i](p.png), `[x](y)` and [![b](i.svg)](l).";
        let tree = parse_inline(text).unwrap();
        let dests: Vec<&str> = tree
            .nodes()
            .filter(|n| n.kind_name() == "link_destination")
            .map(|n| n.text())
            .collect();
        assert_eq!(dests, ["b.md", "p.png", "i.svg", "l"]);
    }
}
