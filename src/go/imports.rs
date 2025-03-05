use crate::config::GoModule;

pub(crate) fn rename(replacement: &GoModule, source_code: &String) -> String {
    let go_language = tree_sitter_go::LANGUAGE.into();
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&go_language)
        .expect("Error loading Go grammar");

    let tree = parser
        .parse(&source_code, None)
        .expect("Failed to parse file");

    let mut cursor = tree.walk();
    let mut edits = Vec::new();

    // Recursive depth-first traversal
    fn visit_node(
        cursor: &mut tree_sitter::TreeCursor,
        source_code: &String,
        replacement: &GoModule,
        edits: &mut Vec<(usize, usize, String)>,
    ) {
        let node = cursor.node();
        if node.is_named() && node.kind() == "import_spec" {
            let module_name = node
                .utf8_text(source_code.as_bytes())
                .expect("Failed to get text");
            if module_name.contains(&replacement.from) {
                let new_module_name = module_name.replace(&replacement.from, &replacement.to);
                edits.push((node.start_byte(), node.end_byte(), new_module_name));
            }
        }

        // Traverse child nodes using depth-first traversal
        if cursor.goto_first_child() {
            loop {
                visit_node(cursor, source_code, replacement, edits);
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
            cursor.goto_parent(); // Return to the parent node
        }
    }

    // Start recursion from the root
    visit_node(&mut cursor, &source_code, replacement, &mut edits);

    // Apply the edits in reverse order to avoid invalidating ranges
    let mut new_source_code = source_code.clone();
    for (start, end, replacement) in edits.iter().rev() {
        new_source_code.replace_range(*start..*end, replacement);
    }
    new_source_code
}

#[cfg(test)]
mod test {
    use tree_sitter::Parser;
    use super::*;

    #[test]
    fn test_rename_import() {
        // Load the tree-sitter Go language
        let mut parser = Parser::new();
        let go_language = tree_sitter_go::LANGUAGE.into();
        parser
            .set_language(&go_language)
            .expect("Error loading Go language");

        // Go source code input
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

        // Define the replacement
        let replacement = GoModule {
            from: "some.module".to_string(),
            to: "new.module".to_string(),
        };

        // Call the function to refactor the Go import
        let refactored_code = rename(&replacement, &go_code.to_string());

        // Expected output
        let expected_code = r#"
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

        // Verify that the refactored code matches the expected output
        assert_eq!(refactored_code, expected_code);
    }

    // Test to replace mumltiple imports
    #[test]
    fn test_rename_multiple_imports() {
        // Load the tree-sitter Go language
        let mut parser = Parser::new();
        let go_language = tree_sitter_go::LANGUAGE.into();
        parser
            .set_language(&go_language)
            .expect("Error loading Go language");

        // Go source code input
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

        // Define the replacement
        let replacement = GoModule {
            from: "some.module".to_string(),
            to: "new.module".to_string(),
        };

        // Call the function to refactor the Go import
        let refactored_code = rename(&replacement, &go_code.to_string());

        let refactored_code = rename(
            &GoModule {
                from: "another.module".to_string(),
                to: "yet.another.module".to_string(),
            },
            &refactored_code.to_string(),
        );

        // Expected output
        let expected_code = r#"
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

        // Verify that the refactored code matches the expected output
        assert_eq!(refactored_code, expected_code);
    }
}
