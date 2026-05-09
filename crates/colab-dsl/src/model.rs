//! Runtime intermediate representation of a compiled codemod script.
//!
//! [`Refactoring`] is the value [`crate::compile`] produces. It holds a
//! list of [`Operation`]s sourced from registered language backends and
//! implements [`CodeTransformer`] so the [`colab_core::walker`] can
//! apply it without knowing about any specific language.

use std::fmt;
use std::path::Path;

use colab_core::{CodeTransformer, Operation};

/// The compiled, executable form of a codemod script.
pub struct Refactoring {
    pub name: String,
    /// Operations applied in source order. Each rule must be a no-op
    /// for files it does not care about so composition is safe across
    /// languages.
    pub rules: Vec<Box<dyn Operation>>,
}

impl Refactoring {
    /// Returns `true` if the script contains no rules.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Number of rules in the script.
    pub fn len(&self) -> usize {
        self.rules.len()
    }
}

impl fmt::Debug for Refactoring {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Refactoring")
            .field("name", &self.name)
            .field("rules", &self.rules)
            .finish()
    }
}

impl fmt::Display for Refactoring {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Refactoring '{}': [", self.name)?;
        for (i, rule) in self.rules.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{}", rule)?;
        }
        write!(f, "]")
    }
}

impl CodeTransformer for Refactoring {
    fn is_file_relevant(&self, path: &Path) -> bool {
        self.rules.iter().any(|r| r.is_file_relevant(path))
    }

    fn apply(&self, source_code: &str) -> String {
        let mut current = source_code.to_string();
        for rule in &self.rules {
            current = rule.apply(&current);
        }
        current
    }
}
