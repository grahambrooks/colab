//! Rename Go `import` paths using a tree-sitter parse.
//!
//! The traversal walks every `import_spec` node and rewrites the bytes in
//! place when the path matches. Edits are applied in reverse byte order so
//! earlier offsets remain valid as later ones are replaced.

use tree_sitter::{Parser, TreeCursor};

/// Replace every Go import whose path contains `from` with the same string
/// substituted for `to`, returning the rewritten source.
///
/// `source_code` is left untouched if no imports match.
pub(crate) fn rename(from: &str, to: &str, source_code: &str) -> String {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_go::LANGUAGE.into())
        .expect("failed to load tree-sitter Go grammar");

    let tree = match parser.parse(source_code, None) {
        Some(tree) => tree,
        None => return source_code.to_string(),
    };

    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut cursor = tree.walk();
    collect_import_edits(&mut cursor, source_code, from, to, &mut edits);

    if edits.is_empty() {
        return source_code.to_string();
    }

    let mut rewritten = source_code.to_string();
    for (start, end, replacement) in edits.iter().rev() {
        rewritten.replace_range(*start..*end, replacement);
    }
    rewritten
}

fn collect_import_edits(
    cursor: &mut TreeCursor,
    source: &str,
    from: &str,
    to: &str,
    edits: &mut Vec<(usize, usize, String)>,
) {
    let node = cursor.node();
    if node.is_named()
        && node.kind() == "import_spec"
        && let Ok(text) = node.utf8_text(source.as_bytes())
        && text.contains(from)
    {
        edits.push((node.start_byte(), node.end_byte(), text.replace(from, to)));
    }

    if cursor.goto_first_child() {
        loop {
            collect_import_edits(cursor, source, from, to, edits);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_a_single_import() {
        let go_code = r#"
    package main

    import (
        "fmt"
        "some.module"
        "another.module"
    )

    func main() {
        fmt.Println("Hello, world!")
    }
    "#;

        let refactored = rename("some.module", "new.module", go_code);

        let expected = r#"
    package main

    import (
        "fmt"
        "new.module"
        "another.module"
    )

    func main() {
        fmt.Println("Hello, world!")
    }
    "#;

        assert_eq!(refactored, expected);
    }

    #[test]
    fn renames_repeated_and_distinct_imports() {
        let go_code = r#"
    package main

    import (
        "fmt"
        "some.module"
        "another.module"
        "some.module"
    )

    func main() {
        fmt.Println("Hello, world!")
    }
    "#;

        let after_first = rename("some.module", "new.module", go_code);
        let after_second = rename("another.module", "yet.another.module", &after_first);

        let expected = r#"
    package main

    import (
        "fmt"
        "new.module"
        "yet.another.module"
        "new.module"
    )

    func main() {
        fmt.Println("Hello, world!")
    }
    "#;

        assert_eq!(after_second, expected);
    }

    #[test]
    fn returns_input_unchanged_when_no_match() {
        let go_code = "package main\n";
        assert_eq!(rename("missing", "replacement", go_code), go_code);
    }
}
