//! What the tests that parse with the backends' parsers share.

/// The tree-sitter grammar colab's own parser of each language is compared
/// against (no backend reads with tree-sitter).
#[allow(dead_code)]
pub fn grammar(lang: &str) -> tree_sitter::Language {
    match lang {
        "c" => tree_sitter_c::LANGUAGE.into(),
        "cpp" => tree_sitter_cpp::LANGUAGE.into(),
        "csharp" => tree_sitter_c_sharp::LANGUAGE.into(),
        "go" => tree_sitter_go::LANGUAGE.into(),
        "java" => tree_sitter_java::LANGUAGE.into(),
        "js" => tree_sitter_javascript::LANGUAGE.into(),
        "kotlin" => tree_sitter_kotlin_ng::LANGUAGE.into(),
        "php" => tree_sitter_php::LANGUAGE_PHP.into(),
        "python" => tree_sitter_python::LANGUAGE.into(),
        "swift" => tree_sitter_swift::LANGUAGE.into(),
        "ts" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
        other => panic!("no tree-sitter grammar for {other}"),
    }
}

/// `text` as colab's tree, by the parser `lang`'s backend reads it with:
/// rust-analyzer's for Rust, colab's own for the rest.
#[allow(dead_code)] // not every test that shares this module parses
pub fn tree(lang: &str, text: &str) -> colab_syntax::Tree {
    match lang {
        "rust" => colab_parse_rust::parse(text).expect("rust-analyzer's tree is lossless"),
        "go" => colab_parse_go::parse(text).expect("colab's Go tree builds"),
        "python" => colab_parse_python::parse(text).expect("colab's Python tree builds"),
        "java" => colab_parse_java::parse(text).expect("colab's Java tree builds"),
        "js" => colab_parse_js::parse(text).expect("colab's JavaScript tree builds"),
        "ts" => colab_parse_js::parse_as(text, colab_parse_js::Dialect::Ts)
            .expect("colab's TypeScript tree builds"),
        "tsx" => colab_parse_js::parse_as(text, colab_parse_js::Dialect::Tsx)
            .expect("colab's TSX tree builds"),
        "csharp" => colab_parse_csharp::parse(text).expect("colab's C# tree builds"),
        "cpp" => colab_parse_c::parse(text).expect("colab's C++ tree builds"),
        "php" => colab_parse_php::parse(text).expect("colab's PHP tree builds"),
        "kotlin" => colab_parse_kotlin::parse(text).expect("colab's Kotlin tree builds"),
        "swift" => colab_parse_swift::parse(text).expect("colab's Swift tree builds"),
        "c" => {
            colab_parse_c::parse_as(text, colab_parse_c::Dialect::C).expect("colab's C tree builds")
        }
        other => panic!("no colab parser for {other}"),
    }
}

/// `text` as tree-sitter reads it: what colab's own parsers are compared
/// against.
#[allow(dead_code)]
pub fn tree_sitter_tree(lang: &str, text: &str) -> colab_syntax::Tree {
    colab_syntax::Tree::parse_tree_sitter(&grammar(lang), text).expect("tree-sitter parses")
}

/// Holds `differ`, each repo file's first difference from tree-sitter's
/// tree, to the reviewed baseline `tests/parsers/<name>.diffs`;
/// `COLAB_BLESS=1` rewrites it. tree-sitter is a reference, not the
/// target: colab's parsers read each language's own grammar where
/// tree-sitter misreads it, and such a change is a reviewed baseline diff.
#[allow(dead_code)]
pub fn baseline(name: &str, differ: &[String]) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/parsers")
        .join(format!("{name}.diffs"));
    let mut lines = differ.to_vec();
    lines.sort();
    let mut text = format!(
        "# The repo's {name} files whose colab tree differs from tree-sitter's: the first\n\
         # differing node of each. Reviewed, not a target; rewrite with\n\
         # COLAB_BLESS=1 cargo test -p colab-backends --test <lang>_parser\n"
    );
    for line in &lines {
        text.push_str(line);
        text.push('\n');
    }
    if std::env::var_os("COLAB_BLESS").is_some() {
        std::fs::write(&path, &text).expect("the baseline is writable");
        return;
    }
    let blessed = std::fs::read_to_string(&path).unwrap_or_default();
    if blessed != text {
        let diff = similar::TextDiff::from_lines(&blessed, &text);
        panic!(
            "{name}: the trees' differences from tree-sitter changed; review and bless \
             (COLAB_BLESS=1):\n{}",
            diff.unified_diff().context_radius(1)
        );
    }
}
