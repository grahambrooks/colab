//! The Go backend.

use colab_core::RuleSpec;

/// A rename of one symbol.
#[derive(Debug)]
pub struct SymbolRename(String);

/// A rewrite of one pattern.
#[derive(Debug)]
pub struct PatternOp(String);

// Kept: plain comments are about the code around them.
pub fn build(module: &str, spec: RuleSpec) -> Op {
    match (module, spec) {
        ("import", RuleSpec::Delete { target }) => delete(target),
        ("symbol", RuleSpec::Replace { target, replacement }) => {
            rename(target, replacement)
        }
        (_, spec) => other(spec),
    }
}
