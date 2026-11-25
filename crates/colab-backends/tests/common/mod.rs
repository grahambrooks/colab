//! What the tests that parse with the backends' grammars share.

/// The tree-sitter grammar each tree-sitter language's files are parsed
/// with, as its backend does. Rust has none: rust-analyzer's parser reads
/// it ([`tree`]).
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
        "ruby" => tree_sitter_ruby::LANGUAGE.into(),
        "swift" => tree_sitter_swift::LANGUAGE.into(),
        "ts" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
        other => panic!("no tree-sitter grammar for {other}"),
    }
}

/// `text` as colab's tree, by the parser `lang`'s backend reads it with.
#[allow(dead_code)] // not every test that shares this module parses
pub fn tree(lang: &str, text: &str) -> colab_syntax::Tree {
    if lang == "rust" {
        return colab_parse_rust::parse(text).expect("rust-analyzer's tree is lossless");
    }
    colab_rewrite::syntax::tree(&grammar(lang), text).expect("tree-sitter parses")
}
