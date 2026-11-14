//! Markdown backend for colab, addressed by structure.
//!
//! - `markdown::section "Parent > Child"` (or `"#anchor"`): a heading and
//!   everything under it, subsections included. See [`outline`].
//! - `markdown::row "Section[Col=value]"`, `markdown::item
//!   "Section[text]"`: a table row or list item in a section's own text.
//! - `markdown::link "<destination>"`: link destinations.
//! - `markdown::frontmatter "<key>"`: a top-level key of the YAML front
//!   matter.
//! - [`check`]: broken links, for the run-level check that keeps a run
//!   from breaking any.
//! - `markdown::block "<name>"`: a managed block, below.
//!
//! A managed block is the text between two HTML-comment marker lines, which
//! Markdown renderers hide:
//!
//! ```text
//! <!-- tool:begin — anything after the name is kept -->
//! …managed text…
//! <!-- tool:end -->
//! ```
//!
//! `markdown::block "tool"` targets it by name. Everything outside the
//! markers belongs to the file's author and is never touched.

pub mod block;
pub mod check;
pub mod frontmatter;
pub mod link;
pub mod list;
pub mod outline;
pub mod section;
pub mod table;

use std::cell::RefCell;

thread_local! {
    static PARSER: RefCell<tree_sitter::Parser> = RefCell::new({
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_md::LANGUAGE.into())
            .expect("the built-in markdown grammar loads");
        parser
    });
    static INLINE: RefCell<tree_sitter::Parser> = RefCell::new({
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_md::INLINE_LANGUAGE.into())
            .expect("the built-in markdown inline grammar loads");
        parser
    });
}

/// Every node of `tree` that `keep` accepts, in document order, borrowed
/// for as long as the tree (unlike `colab_rewrite::visit_all`'s callback).
pub(crate) fn nodes<'t>(
    tree: &'t tree_sitter::Tree,
    mut keep: impl FnMut(tree_sitter::Node<'t>) -> bool,
) -> Vec<tree_sitter::Node<'t>> {
    let mut out = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if keep(node) {
            out.push(node);
        }
        let mut cursor = node.walk();
        let children: Vec<_> = node.children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
    out
}

/// Parse one inline span (a paragraph's or a heading's text) with the
/// inline grammar: links, images, code spans.
pub(crate) fn parse_inline(text: &str) -> Option<tree_sitter::Tree> {
    INLINE.with(|p| p.borrow_mut().parse(text, None))
}

/// `.md` and `.markdown` files.
pub(crate) fn is_markdown(path: &std::path::Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("md" | "markdown")
    )
}

/// Parse `source` with the block grammar (headings, sections, tables,
/// lists, fences, front matter), reusing this thread's parser.
pub(crate) fn parse(source: &str) -> Option<tree_sitter::Tree> {
    PARSER.with(|p| p.borrow_mut().parse(source, None))
}

use colab_core::{ActionCapability, Capability, LanguageBackend, Operation, Result, RuleSpec};

/// Plug-in entry point for the `markdown` namespace.
pub struct MarkdownBackend;

const CAPABILITIES: &[Capability] = &[
    Capability {
        module: "section",
        description: "A heading and everything under it (subsections included), addressed by heading path `\"Parent > Child\"` (text as a reader sees it; outer segments need not be direct parents) or by anchor `\"#slug\"`. An ambiguous address changes nothing and the report says which headings it matched.",
        actions: &[
            ActionCapability {
                name: "set",
                description: "Make everything under the heading exactly this text, adding the section as `insert` does when it is missing; a no-op when it already is, ignoring trailing spaces and blank lines around it.",
            },
            ActionCapability {
                name: "insert",
                description: "Add the section with this body at the end of its parent (the path minus its last segment) when it is missing. The level is one below the parent's.",
            },
            ActionCapability {
                name: "delete",
                description: "Remove the heading and everything under it.",
            },
            ActionCapability {
                name: "replace",
                description: "Give the heading this text, keeping its level. Links in the file to the old anchor are reported as conflicts.",
            },
        ],
    },
    Capability {
        module: "row",
        description: "A row of a pipe table in a section's own text (before its first subsection): `\"Log[Date=2026-11-14]\"` selects by cell values, compared as a reader sees them; `@2` picks the second table; `^` inserts at the top. The value is a whole `| a | b |` row or `Col=value` lines naming some cells.",
        actions: &[
            ActionCapability {
                name: "set",
                description: "Change the one matching row (only the named cells, given `Col=value` lines), or add it when none matches.",
            },
            ActionCapability {
                name: "insert",
                description: "Add the row when none matches the selector.",
            },
            ActionCapability {
                name: "delete",
                description: "Remove every matching row.",
            },
        ],
    },
    Capability {
        module: "item",
        description: "A top-level item of a list in a section's own text, named by the start of its text: `\"Notes[Raw strings]\"`; `@2` picks the second list.",
        actions: &[
            ActionCapability {
                name: "ensure",
                description: "Add `- <text>` in the list's marker style when no item starts with it (creating the list if the section has none).",
            },
            ActionCapability {
                name: "delete",
                description: "Remove the item, nested lines included.",
            },
            ActionCapability {
                name: "replace",
                description: "Give the item this text, keeping its marker and nested list.",
            },
        ],
    },
    Capability {
        module: "link",
        description: "Link destinations (inline links, images, reference definitions; not code): `#anchor` matches a same-file anchor, `guide.md` the path's trailing segments (keeping any anchor), `guide.md#usage` that path with that anchor.",
        actions: &[
            ActionCapability {
                name: "replace",
                description: "Rewrite the matched part of each destination.",
            },
            ActionCapability {
                name: "delete",
                description: "Unwrap an inline link to its text; remove an image or a reference definition.",
            },
        ],
    },
    Capability {
        module: "frontmatter",
        description: "A top-level key of the YAML front matter (`---` block at the top). The value is YAML, compared by meaning (quotes and spacing ignored); a multi-line value (a list) starts with a newline.",
        actions: &[
            ActionCapability {
                name: "set",
                description: "Give the key this value, adding it (and the front matter) when missing.",
            },
            ActionCapability {
                name: "insert",
                description: "Add the key with this value only when it is missing.",
            },
            ActionCapability {
                name: "delete",
                description: "Remove the key and any lines under it.",
            },
        ],
    },
    Capability {
        module: "block",
        description: "Edit the managed block between `<!-- <name>:begin … -->` and `<!-- <name>:end -->` lines; text outside the markers is never touched. A file with the markers repeated or unbalanced is left alone.",
        actions: &[
            ActionCapability {
                name: "delete",
                description: "Remove the block and its markers.",
            },
            ActionCapability {
                name: "set",
                description: "Make the block's content exactly this text, appending the block at the end of the file when it is missing.",
            },
            ActionCapability {
                name: "insert",
                description: "Append the block with this text only when it is missing.",
            },
        ],
    },
];

impl LanguageBackend for MarkdownBackend {
    fn lang(&self) -> &'static str {
        "markdown"
    }

    fn description(&self) -> &'static str {
        "Markdown by structure: sections by heading path, front-matter keys, and managed blocks."
    }

    fn capabilities(&self) -> &'static [Capability] {
        CAPABILITIES
    }

    fn syntax_tree(&self, input: colab_core::AstInput<'_>) -> Result<colab_core::SyntaxTree> {
        let colab_core::AstInput::Source(text) = input else {
            return Err(colab_core::Error::UnsupportedOperation(
                "markdown has no `pattern` module; pass a snippet with --stdin".into(),
            ));
        };
        let (tree, has_error) =
            colab_rewrite::pattern::syntax_tree(&tree_sitter_md::LANGUAGE.into(), text)
                .map_err(|e| colab_core::Error::UnsupportedOperation(format!("markdown: {e}")))?;
        Ok(colab_core::SyntaxTree {
            tree,
            has_error,
            context: None,
            prefilter: None,
        })
    }

    fn build_rule(&self, module: &str, spec: RuleSpec) -> Result<Box<dyn Operation>> {
        match (module, spec) {
            ("block", RuleSpec::Set { target, value }) => {
                Ok(Box::new(block::BlockEdit::set(&target, &value)?))
            }
            ("block", RuleSpec::Insert { target, value }) => {
                Ok(Box::new(block::BlockEdit::insert(&target, &value)?))
            }
            ("block", RuleSpec::Delete { target }) => {
                Ok(Box::new(block::BlockEdit::delete(&target)?))
            }
            ("section", RuleSpec::Set { target, value }) => {
                Ok(Box::new(section::SectionEdit::set(&target, &value)?))
            }
            ("section", RuleSpec::Insert { target, value }) => {
                Ok(Box::new(section::SectionEdit::insert(&target, &value)?))
            }
            ("section", RuleSpec::Delete { target }) => {
                Ok(Box::new(section::SectionEdit::delete(&target)?))
            }
            (
                "section",
                RuleSpec::Replace {
                    target,
                    replacement,
                },
            ) => Ok(Box::new(section::SectionEdit::rename(
                &target,
                &replacement,
            )?)),
            ("row", RuleSpec::Set { target, value }) => {
                Ok(Box::new(table::RowEdit::set(&target, &value)?))
            }
            ("row", RuleSpec::Insert { target, value }) => {
                Ok(Box::new(table::RowEdit::insert(&target, &value)?))
            }
            ("row", RuleSpec::Delete { target }) => Ok(Box::new(table::RowEdit::delete(&target)?)),
            ("item", RuleSpec::Ensure { target }) => Ok(Box::new(list::ItemEdit::ensure(&target)?)),
            ("item", RuleSpec::Delete { target }) => Ok(Box::new(list::ItemEdit::delete(&target)?)),
            (
                "item",
                RuleSpec::Replace {
                    target,
                    replacement,
                },
            ) => Ok(Box::new(list::ItemEdit::replace(&target, &replacement)?)),
            (
                "link",
                RuleSpec::Replace {
                    target,
                    replacement,
                },
            ) => Ok(Box::new(link::LinkEdit::replace(&target, &replacement)?)),
            ("link", RuleSpec::Delete { target }) => Ok(Box::new(link::LinkEdit::delete(&target)?)),
            ("frontmatter", RuleSpec::Set { target, value }) => Ok(Box::new(
                frontmatter::FrontmatterEdit::set(&target, &value)?,
            )),
            ("frontmatter", RuleSpec::Insert { target, value }) => Ok(Box::new(
                frontmatter::FrontmatterEdit::insert(&target, &value)?,
            )),
            ("frontmatter", RuleSpec::Delete { target }) => {
                Ok(Box::new(frontmatter::FrontmatterEdit::delete(&target)?))
            }
            (other, spec) => Err(self.unsupported(other, &spec)),
        }
    }
}
