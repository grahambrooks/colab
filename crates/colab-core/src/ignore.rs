//! `colab:ignore`: a comment that keeps colab's hands off a site.
//!
//! A `colab:ignore` comment that trails code suppresses changes on that
//! line; one on a line of its own suppresses changes on the line below.
//! In any comment syntax:
//!
//! ```text
//! return errors.Wrap(err, "keep") // colab:ignore
//! # colab:ignore pkg-errors-to-fmt
//! <!-- colab:ignore markdown::row -->
//! ```
//!
//! With no names it applies to every rule. Names after it (separated by
//! spaces or commas) limit it to rules whose script name, or whose
//! `<lang>::<module>`, is one of them. The check is textual and needs no
//! parser, so it works for every backend alike; a `colab:ignore` inside a
//! string literal counts too.

use std::collections::HashSet;

/// The marker.
pub const MARKER: &str = "colab:ignore";

/// What a rule is called, for matching the names after the marker.
#[derive(Debug, Clone, Copy)]
pub struct RuleNames<'a> {
    /// The `refactor "<name>"` the rule came from.
    pub script: &'a str,
    /// `go::pattern` and the like.
    pub namespace: &'a str,
}

/// The names after `colab:ignore` on `line`: `Some(vec![])` for a bare
/// marker, `None` when the line has none.
fn marker_names(line: &str) -> Option<Vec<&str>> {
    let at = line.find(MARKER)?;
    let rest = &line[at + MARKER.len()..];
    // `colab:ignored` is not the marker.
    if rest.starts_with(|c: char| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let rest = rest.trim_end_matches(['\n', '\r']);
    let rest = rest
        .split("-->")
        .next()
        .unwrap_or(rest)
        .split("*/")
        .next()
        .unwrap_or(rest);
    Some(
        rest.split([' ', ',', '\t'])
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .collect(),
    )
}

/// `true` when `line` holds nothing but a comment.
fn comment_only(line: &str) -> bool {
    let t = line.trim_start();
    ["//", "#", "/*", "<!--", "--", ";", "%", "*"]
        .iter()
        .any(|opener| t.starts_with(opener))
}

/// The 0-based lines of `source` a rule named `names` must leave alone.
pub fn ignored_lines(source: &str, names: RuleNames<'_>) -> HashSet<usize> {
    let mut out = HashSet::new();
    if !source.contains(MARKER) {
        return out;
    }
    for (i, line) in source.lines().enumerate() {
        let Some(list) = marker_names(line) else {
            continue;
        };
        if list.is_empty()
            || list
                .iter()
                .any(|n| *n == names.script || *n == names.namespace)
        {
            out.insert(i);
            // A marker on a line of its own covers the line below; one that
            // trails code covers only that code.
            if comment_only(line) {
                out.insert(i + 1);
            }
        }
    }
    out
}

/// `after` with every change that touches an ignored line of `before`
/// put back. A change touches a line it replaces or deletes, or, for a
/// pure insertion, the line it is inserted after.
pub fn keep_ignored(before: &str, after: &str, names: RuleNames<'_>) -> String {
    let ignored = ignored_lines(before, names);
    if ignored.is_empty() || before == after {
        return after.to_string();
    }
    let old: Vec<&str> = before.split_inclusive('\n').collect();
    let new: Vec<&str> = after.split_inclusive('\n').collect();
    let diff = similar::TextDiff::from_slices(&old, &new);
    let mut out = String::with_capacity(after.len());
    for op in diff.ops() {
        let o = op.old_range();
        let n = op.new_range();
        let touched = if o.is_empty() {
            o.start.checked_sub(1).is_some_and(|l| ignored.contains(&l))
        } else {
            o.clone().any(|l| ignored.contains(&l))
        };
        // A block that replaces as many lines as it removes is decided line
        // by line, so one ignored line does not hold back its neighbours.
        if matches!(op.tag(), similar::DiffTag::Replace) && o.len() == n.len() {
            for (ol, nl) in o.zip(n) {
                out.push_str(if ignored.contains(&ol) {
                    old[ol]
                } else {
                    new[nl]
                });
            }
            continue;
        }
        let keep_old = !matches!(op.tag(), similar::DiffTag::Equal) && touched;
        let lines = if keep_old { &old[o] } else { &new[n] };
        for line in lines {
            out.push_str(line);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMES: RuleNames<'static> = RuleNames {
        script: "fix",
        namespace: "go::pattern",
    };

    #[test]
    fn a_marker_keeps_its_line_and_the_next() {
        let before = "a()\n// colab:ignore\na()\na() // colab:ignore\na()\n";
        let after = before.replace("a()", "b()");
        assert_eq!(
            keep_ignored(before, &after, NAMES),
            "b()\n// colab:ignore\na()\na() // colab:ignore\nb()\n",
            "a trailing marker covers its own line only"
        );
    }

    #[test]
    fn neighbours_of_an_ignored_line_still_change() {
        let before = "a()
a() // colab:ignore
";
        let after = before.replace("a()", "b()");
        assert_eq!(
            keep_ignored(before, &after, NAMES),
            "b()
a() // colab:ignore
"
        );
        let next = "a()
a()
a()
// colab:ignore
a()
";
        assert_eq!(
            keep_ignored(next, &next.replace("a()", "b()"), NAMES),
            "b()
b()
b()
// colab:ignore
a()
"
        );
    }

    #[test]
    fn names_limit_the_marker() {
        let before = "# colab:ignore other-script\na()\n# colab:ignore go::pattern, x\na()\n";
        let after = before.replace("a()", "b()");
        assert_eq!(
            keep_ignored(before, &after, NAMES),
            "# colab:ignore other-script\nb()\n# colab:ignore go::pattern, x\na()\n"
        );
        let html = "<!-- colab:ignore fix -->\na()\n";
        assert_eq!(keep_ignored(html, &html.replace("a()", "b()"), NAMES), html);
        assert!(ignored_lines("colab:ignored\n", NAMES).is_empty());
    }
}
