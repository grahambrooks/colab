//! One parse per text, shared by the rules that read it (M10.6).
//!
//! Most of a rule's cost is the parse: on 55 MB of Rust, tree-sitter took
//! 5.0 s of the 6.4-8.1 s a `pattern` rule needed. A script's rules run
//! one after another over a file, each parsing the text it is given; when
//! a rule changes nothing, the next is given the same text and parses it
//! again. This cache hands back the tree instead, keyed by the grammar and
//! the exact text, so the rules still see exactly what they saw before
//! (each one the previous one's output): a rule that changes the text
//! makes the next parse a miss. Nothing about what a rule matches or
//! writes depends on it.
//!
//! The cache is per thread, as the walker gives each file to one thread,
//! and holds the last few texts only.

use std::cell::RefCell;
use std::collections::VecDeque;

use tree_sitter::{Language, Parser, Tree};

/// Texts kept: a file's chain of rules moves through one at a time, and a
/// rule's checks may parse its input and its output.
const KEPT: usize = 4;

struct Entry {
    grammar: Language,
    text: String,
    tree: Tree,
}

thread_local! {
    static CACHE: RefCell<VecDeque<Entry>> = const { RefCell::new(VecDeque::new()) };
    static PARSER: RefCell<Option<(Language, Parser)>> = const { RefCell::new(None) };
}

/// `source` parsed with `grammar`, from the cache when this thread parsed
/// the same text with the same grammar lately.
pub fn parse(grammar: &Language, source: &str) -> Option<Tree> {
    let hit = CACHE.with_borrow(|cache| {
        cache
            .iter()
            .find(|e| e.text.len() == source.len() && e.grammar == *grammar && e.text == source)
            .map(|e| e.tree.clone())
    });
    if hit.is_some() {
        return hit;
    }
    let tree = PARSER.with_borrow_mut(|slot| {
        if slot.as_ref().is_none_or(|(g, _)| g != grammar) {
            let mut parser = Parser::new();
            parser.set_language(grammar).ok()?;
            *slot = Some((grammar.clone(), parser));
        }
        slot.as_mut()?.1.parse(source, None)
    })?;
    CACHE.with_borrow_mut(|cache| {
        if cache.len() == KEPT {
            cache.pop_front();
        }
        cache.push_back(Entry {
            grammar: grammar.clone(),
            text: source.to_string(),
            tree: tree.clone(),
        });
    });
    Some(tree)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_text_is_parsed_once_and_a_changed_one_again() {
        let rust: Language = tree_sitter_rust::LANGUAGE.into();
        let go: Language = tree_sitter_go::LANGUAGE.into();
        let a = parse(&rust, "fn a() {}").unwrap();
        let again = parse(&rust, "fn a() {}").unwrap();
        // The cached tree is the one parsed: same root, same text.
        assert_eq!(a.root_node().to_sexp(), again.root_node().to_sexp());
        let b = parse(&rust, "fn b() {}").unwrap();
        assert_eq!(b.root_node().to_sexp(), a.root_node().to_sexp());
        // A different grammar on the same text is its own parse.
        let as_go = parse(&go, "fn a() {}").unwrap();
        assert!(as_go.root_node().has_error());
        assert!(!parse(&rust, "fn a() {}").unwrap().root_node().has_error());
    }
}
