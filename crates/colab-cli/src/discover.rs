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
    let backend = backends.get(lang).ok_or_else(|| {
        Error::UnsupportedOperation(format!(
            "unknown language `{}`; {}",
            lang,
            colab_core::suggest::candidates_note(lang, "languages", &backends.languages())
        ))
    })?;
    Ok(language_capabilities(backend))
}

/// JSON IR for a parsed script (`colab explain`).
pub fn explain(script_path: &Path) -> Result<Value> {
    let script = fs::read_to_string(script_path).map_err(|e| Error::io_at(script_path, e))?;
    let command = colab_dsl::parse(&script)?;
    Ok(explain_command(&command))
}

fn explain_command(cmd: &ast::Command) -> Value {
    let items: Vec<Value> = cmd
        .items
        .iter()
        .map(|item| match item {
            ast::Item::Match(m) => {
                // One shape for every action: a name plus an optional
                // value. The alternative — a bare string for `delete` and
                // an object for `replace` — forces every consumer to
                // handle two shapes for one field.
                let (action, value) = match &m.action {
                    ast::Action::Replace(s) => ("replace", Some(s)),
                    ast::Action::Delete => ("delete", None),
                    ast::Action::Ensure => ("ensure", None),
                    ast::Action::ReplaceCall(t) => ("replace_call", Some(t)),
                    ast::Action::Set(v) => ("set", Some(v)),
                    ast::Action::Insert(v) => ("insert", Some(v)),
                };
                json!({
                    "kind": "match",
                    "namespace": format!("{}::{}", m.namespace.lang, m.namespace.module),
                    "match": m.match_string,
                    "scope": m.scope,
                    "action": action,
                    "value": value,
                })
            }
            ast::Item::Include(path) => json!({
                "kind": "include",
                "path": path,
            }),
        })
        .collect();
    json!({ "name": cmd.refactor_name, "items": items })
}

#[cfg(test)]
mod tests {
    use super::*;
    use colab_core::BackendRegistry;

    fn registry() -> BackendRegistry {
        colab_backends::registry()
    }

    #[test]
    fn schema_lists_go_import_replace() {
        let value = schema(&registry());
        // Look up by name — registration order changes whenever a
        // backend is added.
        let go = value["languages"]
            .as_array()
            .expect("languages")
            .iter()
            .find(|l| l["name"] == "go")
            .expect("go registered");
        let import = go["modules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["name"] == "import")
            .expect("go::import");
        let actions: Vec<&str> = import["actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["name"].as_str().unwrap())
            .collect();
        assert!(actions.contains(&"replace"), "got: {actions:?}");
    }

    #[test]
    fn list_languages_only_carries_top_level() {
        let value = list_languages(&registry());
        let langs = value["languages"].as_array().expect("languages");
        assert!(langs.iter().any(|l| l["name"] == "go"));
        for lang in langs {
            assert!(
                lang.get("modules").is_none(),
                "list-languages must stay the cheap call: {lang}"
            );
        }
    }

    #[test]
    fn list_rules_for_a_registered_lang_succeeds() {
        let value = list_rules(&registry(), "rust").expect("rust is registered");
        assert_eq!(value["name"], "rust");
    }

    #[test]
    fn list_rules_for_unknown_lang_is_unsupported() {
        let err = list_rules(&registry(), "klingon").unwrap_err();
        assert_eq!(err.exit_code(), 3);
    }
}
