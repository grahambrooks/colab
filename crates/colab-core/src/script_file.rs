//! The file extension of a colab script.
//!
//! Scripts are `.colab` files. `.codemod`, the extension colab used
//! first, is still read everywhere a script is looked for, with a
//! warning, so existing scripts keep working until it is dropped.

use std::path::Path;

/// The extension of a colab script, without the dot.
pub const EXTENSION: &str = "colab";

/// The extension colab scripts used before `.colab`, still accepted.
pub const LEGACY_EXTENSION: &str = "codemod";

/// `true` when `path` names a colab script: `.colab`, or the legacy
/// `.codemod`.
pub fn is_script(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e == EXTENSION || e == LEGACY_EXTENSION)
}

/// `true` when `path` has the legacy `.codemod` extension.
pub fn is_legacy(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == LEGACY_EXTENSION)
}

/// Log a warning when `path` still uses the legacy extension.
pub fn warn_if_legacy(path: &Path) {
    if is_legacy(path) {
        log::warn!(
            "{}: the `.{LEGACY_EXTENSION}` extension is deprecated; rename the script to `.{EXTENSION}`",
            path.display()
        );
    }
}

/// `name` without a trailing `.colab` or `.codemod`.
pub fn strip_extension(name: &str) -> &str {
    name.strip_suffix(".colab")
        .or_else(|| name.strip_suffix(".codemod"))
        .unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_extensions_are_scripts() {
        assert!(is_script(Path::new("a/b.colab")));
        assert!(is_script(Path::new("a/b.codemod")));
        assert!(!is_script(Path::new("a/b.toml")));
        assert!(!is_script(Path::new(".colab")));
    }

    #[test]
    fn only_codemod_is_legacy() {
        assert!(is_legacy(Path::new("x.codemod")));
        assert!(!is_legacy(Path::new("x.colab")));
    }

    #[test]
    fn strips_either_extension() {
        assert_eq!(strip_extension("go/x.colab"), "go/x");
        assert_eq!(strip_extension("go/x.codemod"), "go/x");
        assert_eq!(strip_extension("go/x"), "go/x");
    }
}
