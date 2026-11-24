//! Bare rules: `match …` items with no `refactor "…" { … }` around them.
//!
//! `colab refactor -e '<match …>'` has always accepted them; the MCP
//! tools take the same text, so both surfaces wrap it here, and both
//! report a parse error against the text the caller wrote, not the
//! wrapper.

use colab_core::{Error, ParseDetail};

/// The wrapper [`wrap`] puts before bare rules.
pub const PREFIX: &str = "refactor \"inline\" { ";

/// `rules` inside a `refactor` block, unless its first token (past
/// whitespace and `//` comments) already opens one.
pub fn wrap(rules: &str) -> Option<String> {
    (!opens_refactor(rules)).then(|| format!("{PREFIX}{rules}\n}}\n"))
}

/// Re-anchor a parse error in a script [`wrap`] produced onto `rules`,
/// the text that was wrapped. Other errors pass through.
pub fn relocate(rules: &str, err: Error) -> Error {
    match err {
        Error::Parse(detail) => Error::Parse(ParseDetail::at_offset(
            rules,
            detail.offset.saturating_sub(PREFIX.len()),
            detail.message,
            detail.expected,
        )),
        other => other,
    }
}

fn opens_refactor(text: &str) -> bool {
    let mut rest = text.trim_start();
    while let Some(comment) = rest.strip_prefix("//") {
        rest = comment
            .split_once('\n')
            .map_or("", |(_, after)| after)
            .trim_start();
    }
    rest.strip_prefix("refactor")
        .is_some_and(|after| !after.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_bare_rules_are_wrapped() {
        assert_eq!(
            wrap("match rust::pattern 'f($A)' { delete }").as_deref(),
            Some("refactor \"inline\" { match rust::pattern 'f($A)' { delete }\n}\n")
        );
        assert!(wrap("refactor \"x\" { }").is_none());
        assert!(wrap("  // why\n// more\n refactor \"x\" { }").is_none());
        assert!(
            wrap("refactoring").is_some(),
            "a longer word is not the keyword"
        );
    }

    #[test]
    fn a_parse_error_points_into_the_bare_text() {
        let rules = "match rust::pattern 'f($A)' { nope }";
        let wrapped = wrap(rules).unwrap();
        let err = crate::parse(&wrapped).unwrap_err();
        let Error::Parse(detail) = relocate(rules, err) else {
            panic!("a parse error");
        };
        assert_eq!(detail.line, 1);
        assert_eq!(&rules[detail.offset..detail.offset + 4], "nope");
    }
}
