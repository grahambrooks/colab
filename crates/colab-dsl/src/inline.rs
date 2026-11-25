//! Bare rules: `match …` items with no `refactor "…" { … }` around them.
//!
//! The grammar takes them anywhere a script is read (`-e`, `--script`,
//! stdin, an `include`, the MCP tools), as
//! one block named [`NAME`]; the compiler names a bare script file after
//! the file instead. Parse errors point into the text as written, since
//! nothing is wrapped around it.

/// The name of a block of bare rules that did not come from a file.
pub const NAME: &str = "inline";

/// Whether `text` is bare items: its first token (past whitespace and
/// `//` comments) is not `refactor`.
pub fn is_bare(text: &str) -> bool {
    let mut rest = text.trim_start();
    while let Some(comment) = rest.strip_prefix("//") {
        rest = comment
            .split_once('\n')
            .map_or("", |(_, after)| after)
            .trim_start();
    }
    !rest
        .strip_prefix("refactor")
        .is_some_and(|after| !after.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use colab_core::Error;

    #[test]
    fn a_script_is_bare_unless_it_opens_a_refactor_block() {
        assert!(is_bare("match rust::pattern 'f($A)' { delete }"));
        assert!(!is_bare("refactor \"x\" { }"));
        assert!(!is_bare("  // why\n// more\n refactor \"x\" { }"));
        assert!(is_bare("refactoring"), "a longer word is not the keyword");
    }

    #[test]
    fn bare_items_parse_as_one_inline_block() {
        let command = crate::parse(
            "pattern p = 'g($X)'\nmatch rust::pattern 'f($A)' { delete }\nthen { match rust::pattern 'h()' { delete } }",
        )
        .unwrap();
        assert_eq!(command.refactor_name, NAME);
        assert_eq!(command.items.len(), 3);
    }

    #[test]
    fn a_parse_error_points_into_the_bare_text() {
        let rules = "match rust::pattern 'f($A)' { delete }\nmatch rust::pattern 'f($A)' { nope }";
        let Error::Parse(detail) = crate::parse(rules).unwrap_err() else {
            panic!("a parse error");
        };
        assert_eq!(detail.line, 2);
        assert_eq!(&rules[detail.offset..detail.offset + 4], "nope");
    }
}
