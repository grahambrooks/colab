//! The one place every colab language backend is registered.
//!
//! Before this crate existed the backend list was enumerated three times
//! — in the binary's `default_backends`, in the corpus harness's
//! `registry()`, and in the MCP server's tests — plus four manifests.
//! Adding seven languages meant edits in eleven sites, and forgetting one
//! produced a confusing "no backend for language `csharp`" from the
//! corpus rather than a compile error.
//!
//! Everything that needs a populated registry now calls [`registry`]:
//! `colab-cli` as a runtime dependency, and `colab-dsl` / `colab-mcp` as
//! a **dev-dependency only**. That last part matters — those two crates
//! must not depend on a backend at runtime, so the CI matrix can build
//! them while a backend is broken. Depending on this crate for tests does
//! not weaken that: it is the same relationship they already had with the
//! individual backend crates, expressed once.

use colab_core::BackendRegistry;

/// Every backend compiled into colab, in a stable order.
///
/// Order is alphabetical by namespace and is what `colab schema` and
/// `colab list-languages` report. `BackendRegistry` resolves by name, so
/// order affects presentation only — but keeping it stable keeps the
/// discovery output diffable.
pub fn registry() -> BackendRegistry {
    let mut registry = BackendRegistry::new();
    registry.register(Box::new(colab_lang_c::CBackend));
    registry.register(Box::new(colab_lang_cpp::CppBackend));
    registry.register(Box::new(colab_lang_csharp::CSharpBackend));
    registry.register(Box::new(colab_lang_go::GoBackend));
    registry.register(Box::new(colab_lang_java::JavaBackend));
    registry.register(Box::new(colab_lang_js::JsBackend));
    registry.register(Box::new(colab_lang_json::JsonBackend));
    registry.register(Box::new(colab_lang_kotlin::KotlinBackend));
    registry.register(Box::new(colab_lang_markdown::MarkdownBackend));
    registry.register(Box::new(colab_lang_php::PhpBackend));
    registry.register(Box::new(colab_lang_python::PythonBackend));
    registry.register(Box::new(colab_lang_ruby::RubyBackend));
    registry.register(Box::new(colab_lang_rust::RustBackend));
    registry.register(Box::new(colab_lang_swift::SwiftBackend));
    registry.register(Box::new(colab_lang_toml::TomlBackend));
    registry
}

/// The number of backends [`registry`] registers.
///
/// Exposed so a test can assert the registry has not silently lost one.
pub const BACKEND_COUNT: usize = 15;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_every_backend() {
        assert_eq!(registry().languages().len(), BACKEND_COUNT);
    }

    #[test]
    fn language_names_are_unique_and_sorted() {
        let langs = registry().languages();
        let mut sorted = langs.clone();
        sorted.sort_unstable();
        assert_eq!(langs, sorted, "keep registration alphabetical");
        sorted.dedup();
        assert_eq!(sorted.len(), langs.len(), "duplicate language name");
    }

    /// The registry must match what the workspace actually builds.
    ///
    /// This is the guard for the failure that motivated the crate: a new
    /// `colab-lang-*` crate that nobody registered. Reading the workspace
    /// manifest means the test cannot drift from reality the way a
    /// hand-maintained count would.
    #[test]
    fn every_colab_lang_crate_is_registered() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("workspace root above crates/colab-backends")
            .join("Cargo.toml");
        let text = std::fs::read_to_string(&manifest).expect("read workspace manifest");

        let members: Vec<&str> = text
            .lines()
            .filter_map(|l| l.trim().strip_prefix('"')?.strip_suffix("\","))
            .filter_map(|m| m.strip_prefix("crates/colab-lang-"))
            .collect();

        assert_eq!(
            members.len(),
            BACKEND_COUNT,
            "workspace has {} colab-lang-* crates but {} are registered: {:?}",
            members.len(),
            BACKEND_COUNT,
            members
        );
    }
}
