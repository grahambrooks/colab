//! Workflow-shaped tools: `colab.move_import` and `colab.set_config_key`.
//!
//! Each builds the one-rule script an agent would otherwise have to write,
//! runs it through the same path as `colab.preview` / `colab.apply`, and
//! returns that report with the generated `script` beside it. A first call
//! needs no DSL, and the answer teaches it.

use colab_core::BackendRegistry;
use serde_json::Value;

use crate::{ToolError, arg_str, suggest};

/// `s` as a DSL string. DSL strings have no escapes, so a value is quoted
/// with whichever quote it does not contain.
pub(crate) fn dsl_string(s: &str, what: &str) -> Result<String, ToolError> {
    if !s.contains('"') {
        Ok(format!("\"{s}\""))
    } else if !s.contains('\'') {
        Ok(format!("'{s}'"))
    } else {
        Err(ToolError::protocol(format!(
            "`{what}` contains both `'` and `\"`, which no DSL string can hold; \
             use colab.preview with a script that captures the value instead"
        )))
    }
}

fn unknown_language(lang: &str, backends: &BackendRegistry) -> ToolError {
    ToolError::from_error(colab_core::Error::UnsupportedOperation(format!(
        "unknown language `{lang}`; {}",
        suggest::candidates_note(lang, "languages", &backends.languages())
    )))
}

/// `match <lang>::<import module> "<from>" { replace "<to>" }`, or
/// `{ delete }` when there is no `to`.
pub(crate) fn move_import_script(
    args: &Value,
    backends: &BackendRegistry,
) -> Result<String, ToolError> {
    let lang = arg_str(args, "lang")?;
    let backend = backends
        .get(&lang)
        .ok_or_else(|| unknown_language(&lang, backends))?;
    let module = backend.import_module().ok_or_else(|| {
        ToolError::from_error(colab_core::Error::UnsupportedOperation(format!(
            "`{lang}` has no import module; colab.move_import works on source languages"
        )))
    })?;
    let from = dsl_string(&arg_str(args, "from")?, "from")?;
    let action = match args.get("to").and_then(Value::as_str) {
        Some(to) => format!("replace {}", dsl_string(to, "to")?),
        None => "delete".to_string(),
    };
    Ok(format!(
        "refactor \"move_import\" {{\n    match {lang}::{module} {from} {{ {action} }}\n}}\n"
    ))
}

/// `match <json|toml>::key "<key>" { set '<value>' }` (or `insert` /
/// `delete`), or `markdown::block` for a Markdown managed block. The
/// language comes from `lang`, else from the first path's extension.
pub(crate) fn config_key_script(args: &Value) -> Result<String, ToolError> {
    let lang = match args.get("lang").and_then(Value::as_str) {
        Some(lang) => lang.to_string(),
        None => {
            let first = args
                .get("paths")
                .and_then(Value::as_array)
                .and_then(|p| p.first())
                .and_then(Value::as_str)
                .unwrap_or("");
            match std::path::Path::new(first)
                .extension()
                .and_then(|e| e.to_str())
            {
                Some("json") => "json".into(),
                Some("toml") => "toml".into(),
                Some("md" | "markdown") => "markdown".into(),
                _ => {
                    return Err(ToolError::protocol(
                        "cannot tell the format from the first path; pass `lang`: `json`, `toml` or `markdown`",
                    ));
                }
            }
        }
    };
    let module = match lang.as_str() {
        "json" | "toml" => "key",
        "markdown" => "block",
        other => {
            return Err(ToolError::protocol(format!(
                "colab.set_config_key edits `json`, `toml` or `markdown`, not `{other}`"
            )));
        }
    };
    let key = dsl_string(&arg_str(args, "key")?, "key")?;
    let delete = args.get("delete").and_then(Value::as_bool).unwrap_or(false);
    let action = if delete {
        "delete".to_string()
    } else {
        let value = dsl_string(&arg_str(args, "value")?, "value")?;
        let only_if_absent = args
            .get("only_if_absent")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        format!("{} {value}", if only_if_absent { "insert" } else { "set" })
    };
    Ok(format!(
        "refactor \"set_config_key\" {{\n    match {lang}::{module} {key} {{ {action} }}\n}}\n"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn values_are_quoted_with_the_quote_they_lack() {
        assert_eq!(dsl_string("a/b", "x").unwrap(), "\"a/b\"");
        assert_eq!(dsl_string("\"1.2\"", "x").unwrap(), "'\"1.2\"'");
        assert!(dsl_string("it's \"x\"", "x").is_err());
    }

    #[test]
    fn config_scripts_follow_the_file_format() {
        let toml =
            json!({"paths": ["Cargo.toml"], "key": "dependencies.regex", "value": "\"1.10\""});
        assert_eq!(
            config_key_script(&toml).unwrap(),
            "refactor \"set_config_key\" {\n    match toml::key \"dependencies.regex\" { set '\"1.10\"' }\n}\n"
        );
        let absent =
            json!({"paths": ["a.json"], "key": "/a", "value": "1", "only_if_absent": true});
        assert!(
            config_key_script(&absent)
                .unwrap()
                .contains("json::key \"/a\" { insert \"1\" }")
        );
        let gone = json!({"paths": ["a.json"], "key": "/a", "delete": true});
        assert!(config_key_script(&gone).unwrap().contains("{ delete }"));
        assert!(
            config_key_script(&json!({"paths": ["a.yaml"], "key": "k", "value": "1"})).is_err()
        );
    }
}
