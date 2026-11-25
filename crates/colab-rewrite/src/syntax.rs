//! The engine's bridge to `colab_syntax`, colab's own tree and matcher,
//! while the languages still parse with tree-sitter: a grammar's names
//! (built once per grammar), a source text as a colab tree, and a
//! pattern compiled with a backend's [`PatternLanguage`].
//!
//! Nothing in the engine matches through this yet; it is here so the new
//! matcher can be run beside the ast-grep one on the same patterns
//! (`colab-backends/tests/properties.rs`) until it can take over.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use colab_syntax::fragment::{self, Compiled, Fragments};
use colab_syntax::{Grammar, Tree};

use crate::pattern::PatternLanguage;

/// `language`'s kind and field names, built on first use.
pub fn grammar_of(language: &tree_sitter::Language) -> Arc<Grammar> {
    static GRAMMARS: OnceLock<Mutex<HashMap<tree_sitter::Language, Arc<Grammar>>>> =
        OnceLock::new();
    let mut grammars = GRAMMARS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    grammars
        .entry(language.clone())
        .or_insert_with(|| {
            let name = language.name().unwrap_or("?").to_string();
            Arc::new(Grammar::from_tree_sitter(&name, language))
        })
        .clone()
}

/// `source` as a colab tree, parsed through the shared parse cache.
pub fn tree(language: &tree_sitter::Language, source: &str) -> Option<Tree> {
    let parsed = crate::parse_cache::parse(language, source)?;
    Tree::from_tree_sitter(&parsed, source, grammar_of(language)).ok()
}

/// `text` compiled as a pattern for `language` by colab's own matcher:
/// bare or in the language's contexts, with every other reading as a form.
pub fn compile_fragment(language: &PatternLanguage, text: &str) -> Result<Compiled, String> {
    let lang = Fragments {
        expando: language.expando,
        contexts: language.contexts,
        also: language.also,
    };
    // A fragment's parse is never reused, so it skips the shared cache.
    let parse = |filled: &str| {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&language.grammar).ok()?;
        let parsed = parser.parse(filled, None)?;
        Tree::from_tree_sitter(&parsed, filled, grammar_of(&language.grammar)).ok()
    };
    fragment::compile(text, &lang, &parse)
}
