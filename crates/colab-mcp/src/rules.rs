//! Structured rules: the `rules` argument every script tool takes instead
//! of `script`, so an agent writes JSON it already knows rather than the
//! DSL's syntax and quoting.
//!
//! ```json
//! {"rules": [
//!   {"lang": "rust", "match": "old($A)", "replace": "new($A)",
//!    "where": [{"not_inside": "mod tests { $$$B }"}, {"var": "$A", "matches": "^cfg"}],
//!    "in": "src/**"}
//! ]}
//! ```
//!
//! Each rule is written as DSL text, every string quoted by
//! [`colab_dsl::ast::quote_double`] so any text reads back as itself, and
//! compiled by the one compiler: the two front ends cannot drift apart. The
//! generated script is returned beside the report, to save as a `.colab`
//! file once a rule works.

use std::fmt::Write as _;

use serde_json::{Map, Value};

use crate::ToolError;
use colab_dsl::ast::quote_double as q;

/// The script a tool runs: the `script` argument, or the DSL `rules` stand
/// for (`generated` then holds it, for the reply).
pub(crate) struct ScriptArg {
    pub(crate) text: String,
    pub(crate) generated: bool,
}

/// `script` or `rules` from a tool's arguments, exactly one of them.
pub(crate) fn script_arg(args: &Value) -> Result<ScriptArg, ToolError> {
    match (args.get("script"), args.get("rules")) {
        (Some(_), Some(_)) => Err(ToolError::protocol(
            "give `script` or `rules`, not both".to_string(),
        )),
        (Some(Value::String(text)), None) => Ok(ScriptArg {
            text: text.clone(),
            generated: false,
        }),
        (Some(_), None) => Err(ToolError::protocol("argument `script` must be a string")),
        (None, Some(Value::Array(rules))) if !rules.is_empty() => Ok(ScriptArg {
            text: script_of(rules)?,
            generated: true,
        }),
        (None, Some(_)) => Err(ToolError::protocol(
            "argument `rules` must be a non-empty array of rule objects".to_string(),
        )),
        (None, None) => Err(ToolError::protocol(
            "give `rules` (an array of rule objects, e.g. [{\"lang\": \"rust\", \"match\": \"old($A)\", \"replace\": \"new($A)\"}]) or `script`".to_string(),
        )),
    }
}

/// The keys a rule object takes.
const RULE_KEYS: &[&str] = &[
    "lang",
    "module",
    "match",
    "replace",
    "delete",
    "ensure",
    "replace_call",
    "set",
    "insert",
    "create",
    "in",
    "where",
    "let",
    "and_fences",
    "and_prose",
    "requires_import",
    "message",
    "severity",
    "id",
    "url",
];

/// The action keys, exactly one of which a rule has.
const ACTIONS: &[&str] = &[
    "replace",
    "delete",
    "ensure",
    "replace_call",
    "set",
    "insert",
    "create",
];

/// The DSL for a rules document (`colab refactor --rules-json`): an array
/// of rule objects, or `{"rules": [...]}` as the MCP tools take them.
pub fn script_from_rules_json(document: &Value) -> Result<String, String> {
    let rules = match document {
        Value::Array(rules) => rules,
        Value::Object(o) => match o.get("rules") {
            Some(Value::Array(rules)) => rules,
            _ => return Err("a rules document is an array of rules, or {\"rules\": [...]}".into()),
        },
        _ => return Err("a rules document is an array of rules, or {\"rules\": [...]}".into()),
    };
    if rules.is_empty() {
        return Err("the rules document has no rules".into());
    }
    let mut out = String::new();
    for (i, rule) in rules.iter().enumerate() {
        out.push_str(&rule_of(rule).map_err(|why| format!("rules[{i}]: {why}"))?);
        out.push('\n');
    }
    Ok(out)
}

/// The DSL for `rules`, one `match` per rule.
pub(crate) fn script_of(rules: &[Value]) -> Result<String, ToolError> {
    let mut out = String::new();
    for (i, rule) in rules.iter().enumerate() {
        let line =
            rule_of(rule).map_err(|why| ToolError::protocol(format!("rules[{i}]: {why}")))?;
        out.push_str(&line);
        out.push('\n');
    }
    Ok(out)
}

fn rule_of(rule: &Value) -> Result<String, String> {
    let Value::Object(rule) = rule else {
        return Err("a rule is an object, e.g. {\"lang\": \"go\", \"match\": \"errors.Wrap($E, $M)\", \"delete\": true}".into());
    };
    if let Some(unknown) = rule.keys().find(|k| !RULE_KEYS.contains(&k.as_str())) {
        return Err(format!(
            "unknown key `{unknown}`; a rule takes {}",
            RULE_KEYS.join(", ")
        ));
    }
    let lang = string(rule, "lang")?
        .ok_or("give `lang`: the language or format (`rust`, `go`, `toml`, `markdown`, …)")?;
    let module = string(rule, "module")?.unwrap_or("pattern");
    let target = string(rule, "match")?.ok_or(
        "give `match`: the code pattern (`old($A)`), or the module's target (an import path, a key)",
    )?;
    let mut text = format!("match {lang}::{module} {}", q(target));
    if let Some(glob) = string(rule, "in")? {
        let _ = write!(text, " in {}", q(glob));
    }
    if flag(rule, "and_fences")? {
        text.push_str(" and fences");
    }
    if flag(rule, "and_prose")? {
        text.push_str(" and prose");
    }
    if let Some(constraints) = rule.get("where") {
        let Value::Array(list) = constraints else {
            return Err("`where` is an array of constraints".into());
        };
        if !list.is_empty() {
            let all: Vec<String> = list.iter().map(constraint).collect::<Result<_, _>>()?;
            let _ = write!(text, " where {}", all.join(", "));
        }
    }
    text.push_str(" { ");
    if let Some(lets) = rule.get("let") {
        let Value::Array(list) = lets else {
            return Err("`let` is an array of DSL `let` clauses, e.g. \"$N = $F | snake\"".into());
        };
        for item in list {
            let Value::String(clause) = item else {
                return Err("each `let` is a string, e.g. \"$N = $F | snake\"".into());
            };
            let _ = write!(
                text,
                "let {} ",
                clause.trim().trim_start_matches("let ").trim()
            );
        }
    }
    text.push_str(&action(rule)?);
    if let Some(import) = string(rule, "requires_import")? {
        let _ = write!(text, " requires import {}", q(import));
    }
    if let Some(message) = string(rule, "message")? {
        let _ = write!(text, " message {}", q(message));
    }
    if let Some(severity) = string(rule, "severity")? {
        if !matches!(severity, "error" | "warning" | "info") {
            return Err(format!(
                "`severity` is error, warning or info, not `{severity}`"
            ));
        }
        let _ = write!(text, " severity {severity}");
    }
    if let Some(id) = string(rule, "id")? {
        let _ = write!(text, " id {}", q(id));
    }
    if let Some(url) = string(rule, "url")? {
        let _ = write!(text, " url {}", q(url));
    }
    text.push_str(" }");
    Ok(text)
}

/// The rule's one action.
fn action(rule: &Map<String, Value>) -> Result<String, String> {
    let present: Vec<&str> = ACTIONS
        .iter()
        .copied()
        .filter(|a| rule.contains_key(*a))
        .collect();
    let [name] = present.as_slice() else {
        return Err(format!(
            "give exactly one action of {}{}",
            ACTIONS.join(", "),
            if present.is_empty() {
                String::new()
            } else {
                format!(" (this rule has {})", present.join(", "))
            }
        ));
    };
    let value = &rule[*name];
    Ok(match (*name, value) {
        ("delete" | "ensure", Value::Bool(true)) => (*name).to_string(),
        ("delete" | "ensure", _) => return Err(format!("`{name}` takes `true`")),
        ("create", Value::Object(create)) => {
            let path = string(create, "path")?.ok_or("`create` needs `path`")?;
            let template = string(create, "template")?.ok_or("`create` needs `template`")?;
            format!("create {} {}", q(path), q(template))
        }
        ("create", _) => {
            return Err("`create` takes {\"path\": \"…\", \"template\": \"…\"}".into());
        }
        (_, Value::String(text)) => format!("{name} {}", q(text)),
        _ => return Err(format!("`{name}` takes a string")),
    })
}

/// The relations a structured constraint names a pattern with.
const RELATIONS: &[&str] = &["inside", "has", "follows", "precedes"];
/// The tests on a metavariable (`{"var": "$X", "matches": "…"}`).
const VAR_TESTS: &[&str] = &[
    "matches",
    "is",
    "in",
    "type",
    "resolves",
    "implements",
    "calls",
    "called_by",
    "count",
];

/// One `where` constraint: a DSL constraint string as it is, or an object.
fn constraint(value: &Value) -> Result<String, String> {
    let object = match value {
        Value::String(text) => return Ok(text.trim().to_string()),
        Value::Object(object) => object,
        _ => return Err(
            "a constraint is a string or an object, e.g. {\"not_inside\": \"mod tests { $$$B }\"}"
                .into(),
        ),
    };
    let mut not = object.get("not").and_then(Value::as_bool).unwrap_or(false);
    // `not_inside` / `not_has` / … as one key.
    let relation = RELATIONS.iter().find_map(|r| {
        if let Some(v) = object.get(*r) {
            Some((*r, v, false))
        } else {
            object.get(&format!("not_{r}")).map(|v| (*r, v, true))
        }
    });
    if let Some((relation, pattern, negated)) = relation {
        not |= negated;
        let Value::String(pattern) = pattern else {
            return Err(format!("`{relation}` takes a pattern string"));
        };
        let stop = match (object.get("directly"), object.get("until")) {
            (Some(Value::Bool(true)), _) => " directly".to_string(),
            (_, Some(Value::String(until))) => format!(" until {}", q(until)),
            _ => String::new(),
        };
        return Ok(format!(
            "{}{relation} {}{stop}",
            if not { "not " } else { "" },
            q(pattern)
        ));
    }
    if let Some(var) = object.get("var").and_then(Value::as_str) {
        let var = if var.starts_with('$') {
            var.to_string()
        } else {
            format!("${var}")
        };
        let Some((test, arg)) = VAR_TESTS
            .iter()
            .find_map(|t| object.get(*t).map(|v| (*t, v)))
            .or_else(|| {
                object
                    .get("not_matches")
                    .map(|v| ("matches", v))
                    .inspect(|_| not = true)
            })
        else {
            return Err(format!(
                "a `var` constraint takes one of {}",
                VAR_TESTS.join(", ")
            ));
        };
        let Value::String(arg) = arg else {
            return Err(format!("`{test}` takes a string"));
        };
        let arg = match test {
            // A kind, a set name, or a range are words, not strings.
            "in" | "count" => arg.clone(),
            _ => q(arg),
        };
        return Ok(format!(
            "{}{var} {test} {arg}",
            if not { "not " } else { "" }
        ));
    }
    for (key, inner) in [("any", "any"), ("all", "all")] {
        if let Some(Value::Array(list)) = object.get(key) {
            let all: Vec<String> = list.iter().map(constraint).collect::<Result<_, _>>()?;
            return Ok(format!(
                "{}{inner}({})",
                if not { "not " } else { "" },
                all.join(", ")
            ));
        }
    }
    if object.get("top") == Some(&Value::Bool(true)) {
        return Ok(format!("{}top", if not { "not " } else { "" }));
    }
    if let Some(n) = object.get("nth").and_then(Value::as_i64) {
        return Ok(format!("{}nth {n}", if not { "not " } else { "" }));
    }
    if let Some(Value::String(field)) = object.get("field") {
        return Ok(format!("{}field {field}", if not { "not " } else { "" }));
    }
    Err(format!(
        "unknown constraint {}; use {{\"inside\"|\"has\"|\"follows\"|\"precedes\": \"<pattern>\"}} \
         (or `not_inside`, …; `directly`: true), {{\"var\": \"$X\", \"matches\": \"<regex>\"}} \
         (or {}), {{\"any\"|\"all\": [...]}}, {{\"top\": true}}, {{\"nth\": n}}, {{\"field\": \"<name>\"}}, \
         or a DSL constraint string",
        Value::Object(object.clone()),
        VAR_TESTS.join(", ")
    ))
}

/// An optional string field.
fn string<'v>(object: &'v Map<String, Value>, key: &str) -> Result<Option<&'v str>, String> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(format!("`{key}` takes a string")),
    }
}

/// An optional boolean field.
fn flag(object: &Map<String, Value>, key: &str) -> Result<bool, String> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(format!("`{key}` takes true or false")),
    }
}

/// `colab.generalize`: one edit made by hand (`before` → `after`) as a rule
/// that makes it everywhere else, with that rule's other matches under
/// `paths` when they are given. Nothing is written: the rule goes to
/// `colab.preview` / `colab.apply` as `rules`.
pub(crate) fn generalize(
    args: &Value,
    backends: &colab_core::BackendRegistry,
) -> Result<Value, ToolError> {
    let lang = crate::arg_str(args, "lang")?;
    let before = crate::arg_str(args, "before")?;
    let after = crate::arg_str(args, "after")?;
    let backend = backends.get(&lang).ok_or_else(|| {
        ToolError::from_error(colab_core::Error::UnsupportedOperation(format!(
            "unknown language `{lang}`; {}",
            colab_core::suggest::candidates_note(&lang, "languages", &backends.languages())
        )))
    })?;
    let learned = backend
        .generalize(&before, &after)
        .map_err(ToolError::from_error)?;
    // One rule per change the edit made (an edit with context lines, or
    // two places changed, gives one for each).
    let rules: Vec<Value> = learned
        .iter()
        .map(|g| serde_json::json!({"lang": lang, "match": g.pattern, "replace": g.template}))
        .collect();
    let script = script_of(&rules)?;
    let bindings: Vec<Value> = learned
        .iter()
        .map(|g| {
            Value::Object(
                g.bindings
                    .iter()
                    .map(|(var, text)| (var.clone(), Value::String(text.clone())))
                    .collect(),
            )
        })
        .collect();
    let warnings: Vec<&String> = learned.iter().flat_map(|g| &g.warnings).collect();
    let mut out = serde_json::json!({
        "rules": rules,
        "rule": rules[0],
        "script": script,
        "bindings": bindings,
        "next": "check the rules' matches below (or pass `paths`), adjust `match` / `replace` if needed, then call colab.preview and colab.apply with `rules` as given here",
    });
    if !warnings.is_empty() {
        out["warnings"] = serde_json::json!(warnings);
    }
    if args.get("paths").is_some() {
        let mut find_args = args.clone();
        let object = find_args
            .as_object_mut()
            .ok_or_else(|| ToolError::protocol("arguments must be an object"))?;
        for key in ["lang", "before", "after"] {
            object.remove(key);
        }
        object.insert("rules".into(), Value::Array(rules));
        let found = crate::scripts::find(&find_args, backends)?;
        out["matches"] = found;
    }
    Ok(out)
}

/// The JSON Schema of `rules`, for the tools' `inputSchema`.
pub(crate) fn rules_schema() -> Value {
    serde_json::json!({
        "type": "array",
        "minItems": 1,
        "description": "Rules as JSON, instead of `script`: each {\"lang\", \"match\", one action, …} is one DSL `match`. Strings need no DSL quoting. The reply holds the equivalent `script`, to save as a .colab file.",
        "items": {
            "type": "object",
            "required": ["lang", "match"],
            "additionalProperties": false,
            "properties": {
                "lang": {"type": "string", "description": "Language or format: rust, go, python, js, ts, tsx, java, …, toml, json, markdown, text."},
                "module": {"type": "string", "description": "Default `pattern` (code with $METAVARS). Others: symbol, call, import (per language), key (toml/json), section/row/item/… (markdown), literal/word (text)."},
                "match": {"type": "string", "description": "The pattern, e.g. `errors.Wrap($E, $M)`, or the module's target (an import path, a key, a heading path)."},
                "replace": {"type": "string", "description": "Template using the pattern's $METAVARS."},
                "delete": {"const": true},
                "ensure": {"const": true},
                "replace_call": {"type": "string", "description": "For `call`: a template with $1, $2, $args, $func."},
                "set": {"type": "string", "description": "Config / Markdown value in the file's own syntax."},
                "insert": {"type": "string"},
                "create": {"type": "object", "required": ["path", "template"], "properties": {"path": {"type": "string"}, "template": {"type": "string"}}, "description": "A new file per match, path relative to the matched file."},
                "in": {"type": "string", "description": "A glob the rule is limited to, e.g. `src/**`."},
                "where": {
                    "type": "array",
                    "description": "Constraints, all of which must hold: {\"inside\"|\"has\"|\"follows\"|\"precedes\": pattern} (or not_inside, …; \"directly\": true), {\"var\": \"$X\", \"matches\": regex} (or is, type, resolves, implements, calls, called_by, count, in; \"not\": true), {\"any\"|\"all\": [...]}, {\"top\": true}, or a DSL constraint string.",
                    "items": {"type": ["object", "string"]}
                },
                "let": {"type": "array", "items": {"type": "string"}, "description": "DSL `let` clauses for `replace`, e.g. `$N = $F | snake`."},
                "and_fences": {"type": "boolean", "description": "Also Markdown code fences in this language."},
                "and_prose": {"type": "boolean", "description": "For `symbol`: also comments and strings."},
                "requires_import": {"type": "string"},
                "message": {"type": "string", "description": "Makes the rule a finding (for `fix --check`, CI)."},
                "severity": {"enum": ["error", "warning", "info"]},
                "id": {"type": "string"},
                "url": {"type": "string"}
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rules_become_dsl_that_compiles() {
        let script = script_of(&[
            json!({"lang": "rust", "match": "old($A)", "replace": "new(\"x\", $A)",
                   "where": [{"not_inside": "mod tests { $$$B }"}, {"var": "A", "matches": "^c\\d"}],
                   "in": "src/**"}),
            json!({"lang": "toml", "module": "key", "match": "package.edition", "set": "\"2024\""}),
            json!({"lang": "go", "match": "fmt.Println($$$A)", "delete": true,
                   "message": "no prints", "severity": "warning"}),
        ])
        .unwrap();
        assert!(script.contains(r#"match rust::pattern "old($A)" in "src/**" where not inside "mod tests { $$$B }", $A matches "^c\\d" { replace "new(\"x\", $A)" }"#), "{script}");
        let backends = colab_backends::registry();
        let compiled = colab_dsl::compile(&script, &backends).unwrap();
        assert_eq!(compiled.rules.len(), 3);
    }

    #[test]
    fn mistakes_name_the_rule_and_what_it_takes() {
        let err = |rule: Value| match script_of(&[
            json!({"lang": "go", "match": "f()", "delete": true}),
            rule,
        ]) {
            Err(e) => format!("{e:?}"),
            Ok(s) => panic!("{s}"),
        };
        assert!(
            err(json!({"lang": "go", "match": "f()"}))
                .contains("rules[1]: give exactly one action")
        );
        assert!(
            err(json!({"lang": "go", "match": "f()", "delete": true, "replace": "g()"}))
                .contains("this rule has replace, delete")
        );
        assert!(
            err(json!({"lang": "go", "match": "f()", "remove": true}))
                .contains("unknown key `remove`")
        );
        assert!(
            err(json!({"lang": "go", "match": "f()", "delete": true, "where": [{"near": "x"}]}))
                .contains("unknown constraint")
        );
        assert!(script_arg(&json!({"script": "x", "rules": []})).is_err());
    }
}
