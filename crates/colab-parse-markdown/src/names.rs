//! The names of the kinds and fields a Markdown tree holds.
//!
//! The tree is shaped for what colab reads in Markdown (headings and the
//! sections they start, lists and items, pipe tables, fenced code, link
//! destinations), not for rendering. Its kinds keep tree-sitter-md's names
//! for those constructs, so the Markdown backend's modules and users'
//! rules read them as before, and none of its artefacts (`section` nodes,
//! `block_continuation` tokens, punctuation inside `inline`). The parser
//! names nothing outside this file.

/// The named kinds of the block tree.
pub(crate) const NAMED: &[&str] = &[
    "document",
    "minus_metadata",
    "plus_metadata",
    "atx_heading",
    "atx_h1_marker",
    "atx_h2_marker",
    "atx_h3_marker",
    "atx_h4_marker",
    "atx_h5_marker",
    "atx_h6_marker",
    "setext_heading",
    "setext_h1_underline",
    "setext_h2_underline",
    "paragraph",
    "inline",
    "list",
    "list_item",
    "list_marker_minus",
    "list_marker_plus",
    "list_marker_star",
    "list_marker_dot",
    "list_marker_parenthesis",
    "task_list_marker_checked",
    "task_list_marker_unchecked",
    "block_quote",
    "block_quote_marker",
    "fenced_code_block",
    "fenced_code_block_delimiter",
    "info_string",
    "language",
    "code_fence_content",
    "indented_code_block",
    "html_block",
    "thematic_break",
    "pipe_table",
    "pipe_table_header",
    "pipe_table_delimiter_row",
    "pipe_table_delimiter_cell",
    "pipe_table_row",
    "pipe_table_cell",
    "link_reference_definition",
    "link_label",
    "link_destination",
    "link_title",
    // The inline tree's.
    "inline_link",
    "image",
    "link_text",
    "image_description",
    "code_span",
    "uri_autolink",
    "email_autolink",
];

/// The anonymous kinds: punctuation, named by its text.
pub(crate) const ANONYMOUS: &[&str] = &["|", ":", "[", "]", "(", ")", "!", "<", ">"];

/// The field names; slot 0 is no field.
pub(crate) const FIELDS: &[&str] = &["", "heading_content", "info_string"];
