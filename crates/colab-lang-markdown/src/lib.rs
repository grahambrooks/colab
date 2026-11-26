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
pub mod column;
pub mod frontmatter;
pub mod link;
pub mod list;
pub mod moving;
pub mod outline;
pub mod section;
pub mod table;
pub mod text;

/// Every node of `tree` that `keep` accepts, in document order.
pub(crate) fn nodes<'t>(
    tree: &'t colab_syntax::Tree,
    mut keep: impl FnMut(colab_syntax::Node<'t>) -> bool,
) -> Vec<colab_syntax::Node<'t>> {
    let mut out = Vec::new();
    for node in tree.nodes() {
        if keep(node) {
            out.push(node);
        }
    }
    out
}

/// Every fenced code block in `source`: its language (the info string's
/// first word, lowercased), the lines between its fences, and the opening
/// fence's indentation. The `colab_core::FenceLocator` behind `in fences`.
pub fn fences(source: &str) -> Vec<colab_core::Fence> {
    let Some(tree) = parse(source) else {
        return Vec::new();
    };
    let line_start = |at: usize| source[..at].rfind('\n').map_or(0, |i| i + 1);
    let line_end = |at: usize| source[at..].find('\n').map_or(source.len(), |i| at + i + 1);
    let mut out = Vec::new();
    for node in nodes(&tree, |n| n.kind_name() == "fenced_code_block") {
        let children: Vec<_> = node.children().collect();
        let delimiters: Vec<_> = children
            .iter()
            .filter(|n| n.kind_name() == "fenced_code_block_delimiter")
            .collect();
        let Some(open) = delimiters.first() else {
            continue;
        };
        let lang = children
            .iter()
            .find(|n| n.kind_name() == "info_string")
            .and_then(|info| {
                source[info.range()]
                    .split_whitespace()
                    .next()
                    .map(|w| w.trim_matches(['{', '}', '.']).to_lowercase())
            })
            .unwrap_or_default();
        let start = line_end(open.start());
        let end = match delimiters.get(1) {
            Some(close) => line_start(close.start()),
            None => node.end(),
        }
        .max(start);
        let indent = source[line_start(open.start())..open.start()].to_string();
        out.push(colab_core::Fence {
            lang,
            content: start..end,
            indent,
        });
    }
    out
}

/// Parse one inline span (a paragraph's or a heading's text) for its
/// links, images, autolinks and code spans.
pub(crate) fn parse_inline(text: &str) -> Option<colab_syntax::Tree> {
    colab_parse_markdown::parse_inline(text).ok()
}

/// `.md` and `.markdown` files.
pub(crate) fn is_markdown(path: &std::path::Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("md" | "markdown")
    )
}

/// `source`'s blocks (headings, tables, lists, fences, front matter), by
/// colab's own Markdown parser.
pub(crate) fn parse(source: &str) -> Option<colab_syntax::Tree> {
    colab_parse_markdown::parse(source).ok()
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
        module: "column",
        description: "A column of a pipe table in a section's own text, by its header: `\"Matrix[create]\"`, compared as a reader sees it; `@2` picks the second table. A new column goes last, first with `^`, or before another with `\"Matrix[create] < [Applies to]\"`. Lines are spliced at their pipes, so other cells keep their spacing.",
        actions: &[
            ActionCapability {
                name: "insert",
                description: "Add the column, with this cell in every row, when no column has the header.",
            },
            ActionCapability {
                name: "delete",
                description: "Remove the column from every line of the table.",
            },
            ActionCapability {
                name: "replace",
                description: "Give the column this header.",
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
        module: "text",
        description: "A phrase in a section's own text (not in code fences): `\"Section[the old words]\"`, matched word for word with any whitespace between, so a phrase wrapped across lines matches. Every occurrence changes.",
        actions: &[
            ActionCapability {
                name: "replace",
                description: "Replace each occurrence with this text.",
            },
            ActionCapability {
                name: "delete",
                description: "Remove each occurrence, and one space beside it.",
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

    /// Markdown fences inside Markdown are not supported.
    fn fence_names(&self) -> Vec<&'static str> {
        Vec::new()
    }

    /// `markdown::text … in fences` / `and fences`: the phrase inside the
    /// section's code fences too. No other module edits code.
    fn build_rule_in_fences(
        &self,
        module: &str,
        spec: RuleSpec,
        also: bool,
    ) -> Option<Result<Box<dyn Operation>>> {
        if module != "text" {
            return None;
        }
        let fences = if also {
            text::Fences::Also
        } else {
            text::Fences::Only
        };
        Some(match spec {
            RuleSpec::Replace {
                target,
                replacement,
            } => text::TextEdit::replace(&target, &replacement)
                .map(|op| Box::new(op.with_fences(fences)) as Box<dyn Operation>),
            RuleSpec::Delete { target } => text::TextEdit::delete(&target)
                .map(|op| Box::new(op.with_fences(fences)) as Box<dyn Operation>),
            other => Err(self.unsupported(module, &other)),
        })
    }

    fn fence_locator(&self) -> Option<colab_core::FenceLocator> {
        Some(fences)
    }

    fn syntax_tree(&self, input: colab_core::AstInput<'_>) -> Result<colab_core::SyntaxTree> {
        let colab_core::AstInput::Source(text) = input else {
            return Err(colab_core::Error::UnsupportedOperation(
                "markdown has no `pattern` module; pass a snippet with --stdin".into(),
            ));
        };
        let parsed = parse(text).ok_or_else(|| {
            colab_core::Error::UnsupportedOperation("markdown: the text does not parse".into())
        })?;
        let (tree, has_error) = colab_rewrite::pattern::print_tree_of(&parsed);
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
            ("column", RuleSpec::Insert { target, value }) => {
                Ok(Box::new(column::ColumnEdit::insert(&target, &value)?))
            }
            ("column", RuleSpec::Delete { target }) => {
                Ok(Box::new(column::ColumnEdit::delete(&target)?))
            }
            (
                "column",
                RuleSpec::Replace {
                    target,
                    replacement,
                },
            ) => Ok(Box::new(column::ColumnEdit::rename(&target, &replacement)?)),
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
            (
                "text",
                RuleSpec::Replace {
                    target,
                    replacement,
                },
            ) => Ok(Box::new(text::TextEdit::replace(&target, &replacement)?)),
            ("text", RuleSpec::Delete { target }) => Ok(Box::new(text::TextEdit::delete(&target)?)),
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

#[cfg(test)]
mod fence_tests {
    #[test]
    fn fences_report_language_body_and_indent() {
        let doc = "# T\n\n```Go title=x\nfoo()\n```\n\n- item\n\n  ```rust\n  let x = 1;\n  ```\n\n````\nbare\n````\n";
        let found = super::fences(doc);
        let shown: Vec<(&str, &str, &str)> = found
            .iter()
            .map(|f| (f.lang.as_str(), &doc[f.content.clone()], f.indent.as_str()))
            .collect();
        assert_eq!(
            shown,
            [
                ("go", "foo()\n", ""),
                ("rust", "  let x = 1;\n", "  "),
                ("", "bare\n", "")
            ]
        );
    }
}
