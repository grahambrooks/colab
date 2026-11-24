//! The Go backend.

use colab_core::RuleSpec;

// Kept: plain comments are about the code around them.
pub fn build(module: &str, spec: RuleSpec) -> Op {
    match (module, spec) {
        ("import", RuleSpec::Delete { target }) => delete(target),
        (_, spec) => other(spec),
    }
}
