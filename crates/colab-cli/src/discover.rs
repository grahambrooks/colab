//! Capability and IR discovery commands: `schema`, `list-languages`,
//! `list-rules`, `explain`. All emit pretty-printed JSON to stdout.

use std::fs;
use std::path::Path;

use colab_core::{BackendRegistry, Error, Result};
use colab_dsl::ast;
use serde_json::{Value, json};

/// Build a JSON document describing every backend and its capabilities.
pub fn schema(backends: &BackendRegistry) -> Value {
    let langs: Vec<Value> = backends
        .languages()
        .iter()
        .filter_map(|lang| backends.get(lang).map(language_capabilities))
        .collect();
    json!({ "languages": langs })
}

/// JSON for one backend (used by `schema` and `list-rules`).
fn language_capabilities(backend: &dyn colab_core::LanguageBackend) -> Value {
    let modules: Vec<Value> = backend
        .capabilities()
        .iter()
        .map(|cap| {
            let actions: Vec<Value> = cap
                .actions
                .iter()
                .map(|act| {
                    json!({
                        "name": act.name,
                        "description": act.description,
                    })
                })
                .collect();
            json!({
                "name": cap.module,
                "description": cap.description,
                "actions": actions,
            })
        })
        .collect();
    json!({
        "name": backend.lang(),
        "description": backend.description(),
        "modules": modules,
    })
}

/// JSON for `colab list-languages`.
pub fn list_languages(backends: &BackendRegistry) -> Value {
    let names: Vec<Value> = backends
        .languages()
        .iter()
        .filter_map(|lang| backends.get(lang).map(language_capabilities))
        .map(|v| {
            // Strip module-level detail; users wanting modules call list-rules.
            json!({
                "name": v["name"].clone(),
                "description": v["description"].clone(),
            })
        })
        .collect();
    json!({ "languages": names })
}

/// JSON for `colab list-rules <lang>`. Errors when the lang is not
/// registered; this maps to exit code 3 (unsupported operation).
pub fn list_rules(backends: &BackendRegistry, lang: &str) -> Result<Value> {
    let backend = backends
        .get(lang)
        .ok_or_else(|| Error::UnsupportedOperation(format!("unknown language: {}", lang)))?;
    Ok(language_capabilities(backend))
}

/// JSON IR for a parsed script (`colab explain`).
pub fn explain(script_path: &Path) -> Result<Value> {
    let script = fs::read_to_string(script_path).map_err(|e| Error::io_at(script_path, e))?;
    let command = colab_dsl::parse(&script)?;
    Ok(explain_command(&command))
}

fn explain_command(cmd: &ast::Command) -> Value {
    let rules: Vec<Value> = cmd
        .matches
        .iter()
        .map(|m| {
            let action = match &m.action {
                ast::Action::Replace(s) => json!({ "replace": s }),
                ast::Action::Delete => json!("delete"),
                ast::Action::Ensure => json!("ensure"),
                ast::Action::ReplaceCall(t) => json!({ "replace_call": t }),
            };
            json!({
                "namespace": format!("{}::{}", m.namespace.lang, m.namespace.module),
                "match": m.match_string,
                "action": action,
            })
        })
        .collect();
    json!({ "name": cmd.refactor_name, "rules": rules })
}

#[cfg(test)]
mod tests {
    use super::*;
    use colab_core::BackendRegistry;

    fn registry() -> BackendRegistry {
        let mut r = BackendRegistry::new();
        r.register(Box::new(colab_lang_go::GoBackend));
        r
    }

    #[test]
    fn schema_lists_go_import_replace() {
        let value = schema(&registry());
        let go = &value["languages"][0];
        assert_eq!(go["name"], "go");
        let import = &go["modules"][0];
        assert_eq!(import["name"], "import");
        assert_eq!(import["actions"][0]["name"], "replace");
    }

    #[test]
    fn list_languages_only_carries_top_level() {
        let value = list_languages(&registry());
        assert_eq!(value["languages"][0]["name"], "go");
        assert!(value["languages"][0].get("modules").is_none());
    }

    #[test]
    fn list_rules_for_unknown_lang_is_unsupported() {
        let err = list_rules(&registry(), "rust").unwrap_err();
        assert_eq!(err.exit_code(), 3);
    }
}
